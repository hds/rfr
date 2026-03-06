use std::{
    cell::RefCell,
    collections::VecDeque,
    error, fmt, fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{self, AtomicBool},
    },
    time::{Duration, Instant},
};

use jiff::{Timestamp, Zoned, tz::TimeZone};

use crate::chunked::{
    AbsTimestampSecs, ChunkedCallsitesWriter, ChunkedMeta, SeqId, current_software_version,
    sequence::SeqChunkWriteError,
};
use crate::{
    AbsTimestamp, Callsite,
    chunked::{ChunkHeader, ChunkInterval, SeqChunkBuffer},
};

#[derive(Debug)]
pub struct ChunkedWriter {
    root_dir: PathBuf,
    base_time: AbsTimestampSecs,

    /// The length of time a chunk is "responsible" for. This value must either be a multiple of
    /// seconds (multiple of 1_000_000) or a divisor of a whole second (divisor of 1_000_000).
    chunk_period_micros: u32,

    /// Configuration for how many chunks to keep.
    storage_quota: StorageQuota,

    closed: AtomicBool,

    chunks: Mutex<Chunks>,
    callsites_writer: Mutex<ChunkedCallsitesWriter<fs::File>>,
    notifiers: Mutex<Vec<ChunkWriteNotifier>>,
}

#[derive(Debug, Default)]
struct Chunks {
    written_chunks: VecDeque<WrittenChunk>,
    chunk_buffers: Vec<ChunkBuffer>,
}

/// Configuration for how many chunks to keep when performing cleanup.
///
/// A recording can take up a lot of space on hard disk and for longer running, it is usually
/// necessary to remove older chunks. The chunked recording format is designed for this.
///
/// This configuration determines which older chunks are cleaned up.
///
/// The configuration specifies soft and hard limits for the maximum number of chunks and maximum
/// total chunk size (in bytes). There is also a minimum number of chunks and minimum size.
///
/// The rules work in the following way:
/// - Chunks will be kept up to the soft maximum limits (max chunks and max size)
/// - If the minimum limits have not been reached, then the soft maximum limits will be breached
/// - The hard maximum limits will not be breached.
///
/// For example, consider the following configuration:
///
/// ```
/// use rfr::chunked::StorageQuota;
///
/// let quota = StorageQuota {
///     min_chunks: Some(300),
///     min_bytes: None,
///
///     max_chunks_soft: Some(900),
///     max_bytes_soft: Some(100 * 1024 * 1024),
///
///     max_chunks_hard: None,
///     max_bytes_hard: Some(200 * 1024 * 1024),
/// };
/// # _ = quota;
/// ```
///
/// Chunks will be cleaned up if there are either more than 900 chunks or their combined disk usage
/// is over 100 MiB. However, if there are fewer than 300 chunks, then they won't be cleaned up,
/// even if they occupy over 100 MiB. This will apply up to the hard size limit of 200 MiB, at
/// which point chunks will be cleaned up, even if there are fewer than 300.
#[derive(Debug, Default)]
pub struct StorageQuota {
    pub min_chunks: Option<usize>,
    pub min_bytes: Option<usize>,

    pub max_chunks_soft: Option<usize>,
    pub max_bytes_soft: Option<usize>,

    pub max_chunks_hard: Option<usize>,
    pub max_bytes_hard: Option<usize>,
}

impl StorageQuota {
    /// Whether the provided number of chunks and total size in bytes above any hard limits
    ///
    /// If no limit is set, then any value will NOT be considered above the limit.
    fn is_above_any_hard_max(&self, chunks: usize, total_size_bytes: usize) -> bool {
        self.max_chunks_hard
            .map(|max| chunks > max)
            .unwrap_or(false)
            || self
                .max_bytes_hard
                .map(|max| total_size_bytes > max)
                .unwrap_or(false)
    }

    /// Whether the provided number of chunks and total size in bytes above any soft limits
    ///
    /// If no limit is set, then any value will NOT be considered above the limit.
    fn is_above_any_soft_max(&self, chunks: usize, total_size_bytes: usize) -> bool {
        self.max_chunks_soft
            .map(|max| chunks > max)
            .unwrap_or(false)
            || self
                .max_bytes_soft
                .map(|max| total_size_bytes > max)
                .unwrap_or(false)
    }

    /// Whether the provided number of chunks and total size in bytes are both above the minimum
    /// limit.
    ///
    /// If no limit is set, then the minimum limit will be considered to be zero and any value will
    /// be considered to be above the limit.
    fn is_above_all_min(&self, chunks: usize, total_size_bytes: usize) -> bool {
        Some(chunks) > self.min_chunks && Some(total_size_bytes) > self.min_bytes
    }
}

#[derive(Debug)]
struct WrittenChunk {
    path: PathBuf,
    size_bytes: usize,
}

impl ChunkedWriter {
    pub fn try_new<P>(root_dir: P) -> Result<Self, NewChunkedWriterError>
    where
        P: AsRef<Path>,
    {
        Self::try_new_with_config(root_dir, Default::default())
    }

    pub fn try_new_with_config<P>(
        root_dir: P,
        storage_quota: StorageQuota,
    ) -> Result<Self, NewChunkedWriterError>
    where
        P: AsRef<Path>,
    {
        let root_dir = root_dir.as_ref();

        let timestamp = AbsTimestamp::now();
        let base_time = AbsTimestampSecs::from(timestamp.clone());
        let meta = ChunkedMeta::new(vec![current_software_version()]);

        if let Ok(true) = root_dir.try_exists() {
            return Err(NewChunkedWriterError::already_exists());
        }

        fs::create_dir_all(root_dir).map_err(NewChunkedWriterError::create_recording_dir_failed)?;
        Self::write_meta(root_dir, &meta)?;

        let callsites_path = Path::new(&root_dir).join("callsites.rfr");
        let callsites_file = fs::File::create(callsites_path)
            .map_err(|err| NewChunkedWriterError::write_callsites_failed(WriteError::Io(err)))?;
        let callsites_writer = ChunkedCallsitesWriter::try_new(callsites_file)
            .map_err(NewChunkedWriterError::write_callsites_failed)?;

        // By default, chunks contain 1 second of execution time.
        let chunk_period_micros = 1_000_000;
        let writer = Self {
            root_dir: root_dir.to_owned(),
            base_time,
            chunk_period_micros,
            chunks: Mutex::new(Default::default()),
            storage_quota,
            closed: false.into(),
            callsites_writer: Mutex::new(callsites_writer),
            notifiers: Mutex::new(Vec::new()),
        };

        let base_time = writer.base_time;
        writer
            .ensure_dir(&base_time)
            .map_err(NewChunkedWriterError::create_recording_dir_failed)?;

        Ok(writer)
    }

    pub fn chunk_period_micros(&self) -> u32 {
        self.chunk_period_micros
    }

    pub fn close(&self) {
        self.closed.store(true, atomic::Ordering::SeqCst);

        let mut notifiers = self
            .notifiers
            .lock()
            .expect("cannot notify closed. notifiers poisoned");
        while let Some(notifier) = notifiers.pop() {
            notifier.notify(WaitForWrite::Closed);
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(atomic::Ordering::SeqCst)
    }

    fn write_meta(base_dir: &Path, meta: &ChunkedMeta) -> Result<(), NewMetaError> {
        let path = base_dir.join("meta.rfr");
        {
            let mut file = fs::File::create_new(path).map_err(|err| match err.kind() {
                io::ErrorKind::AlreadyExists => NewMetaError::AlreadyExists,
                _ => NewMetaError::WriteFailed(WriteError::Io(err)),
            })?;

            postcard::to_io(meta, &mut file)
                .map_err(|err| NewMetaError::WriteFailed(WriteError::Serialization(err)))?;
        }

        Ok(())
    }

    fn ensure_dir(&self, time: &AbsTimestampSecs) -> Result<(), io::Error> {
        fs::create_dir_all(self.dir_path(time)?)
    }

    fn dir_path(&self, time: &AbsTimestampSecs) -> Result<PathBuf, io::Error> {
        let ts = Timestamp::from_second(time.secs as i64).map_err(io::Error::other)?;
        let ts_utc = ts.to_zoned(TimeZone::UTC);
        Ok(self.dir_path_from_utc(&ts_utc))
    }

    fn dir_path_from_utc(&self, ts_utc: &Zoned) -> PathBuf {
        Path::new(&self.root_dir)
            .join(format!("{}", ts_utc.strftime("%Y-%m")))
            .join(format!("{}", ts_utc.strftime("%d-%H")))
    }

    fn chunk_path(&self, chunk: &ChunkBuffer) -> Result<PathBuf, io::Error> {
        let time = &chunk.header.interval.base_time;
        let ts = Timestamp::from_second(time.secs as i64).map_err(io::Error::other)?;
        let ts_utc = ts.to_zoned(TimeZone::UTC);

        Ok(self
            .dir_path_from_utc(&ts_utc)
            .join(format!("chunk-{}.rfr", ts_utc.strftime("%M-%S"))))
    }

    pub fn register_callsite(&self, callsite: Callsite) {
        let mut callsites_writer = self
            .callsites_writer
            .lock()
            .expect("callsite writer lock poisoned");
        // TODO(hds): Should we try to avoid building a `Callsite` if it's going to be a duplicate?
        callsites_writer.push_callsite(callsite);
    }

    pub fn with_seq_chunk_buffer<F>(&self, timestamp: AbsTimestamp, f: F)
    where
        F: FnOnce(&SeqChunkBuffer),
    {
        thread_local! {
            pub static SEQ_CHUNK_BUFFER: RefCell<Option<Arc<SeqChunkBuffer>>>
                = const { RefCell::new(None) };
        }

        SEQ_CHUNK_BUFFER.with_borrow_mut(|seq_chunk_buffer| {
            let current_buffer = self.current_seq_chunk_buffer(seq_chunk_buffer, timestamp.clone());
            f(current_buffer);
        });
    }

    fn current_seq_chunk_buffer<'a>(
        &self,
        local_buffer: &'a mut Option<Arc<SeqChunkBuffer>>,
        timestamp: AbsTimestamp,
    ) -> &'a Arc<SeqChunkBuffer> {
        let interval = ChunkInterval::from_timestamp_and_period(
            timestamp.clone(),
            self.chunk_period_micros as u64,
        );
        let seq_chunk_buffer =
            local_buffer.get_or_insert_with(|| self.create_seq_chunk_buffer(interval.clone()));

        if seq_chunk_buffer.interval() != &interval {
            // Stored sequence chunk is not for this interval, create a new sequence chunk.
            *seq_chunk_buffer = self.create_seq_chunk_buffer(interval);
        }

        seq_chunk_buffer
    }

    fn create_seq_chunk_buffer(&self, interval: ChunkInterval) -> Arc<SeqChunkBuffer> {
        let mut chunks = self.chunks.lock().expect("poisoned");
        let chunk_buffer = chunks
            .chunk_buffers
            .iter_mut()
            .find(|cb| cb.header.interval == interval);
        match chunk_buffer {
            Some(chunk_buffer) => chunk_buffer.new_seq_chunk_buffer(),
            None => {
                let mut new_chunk_buffer = ChunkBuffer::new(interval.clone());
                let seq_chunk_buffer = new_chunk_buffer.new_seq_chunk_buffer();
                chunks.chunk_buffers.push(new_chunk_buffer);
                seq_chunk_buffer
            }
        }
    }

    /// Write all the completed chunks out to disk.
    ///
    /// A buffer period between now and the end of each chunk's interval is put in place to give
    /// other threads time to finish writing to the sequence chunks. The buffer is on the order of
    /// 100 milliseconds.
    ///
    /// Once each chunk is written to disk, it is discarded.
    ///
    /// This method is still not race-condition safe, despite the buffer. If a thread is taking a
    /// very long time to prepare an event before calling [`with_seq_chunk_buffer`], then a record
    /// may get lost.
    ///
    /// For this reason, [`with_seq_chunk_buffer`] should be called with a timestamp that is close
    /// to the current time.
    pub fn write_completed_chunks(&self) -> Result<Duration, WriteChunksError> {
        let mut chunks = self.chunks.lock().expect("poisoned");
        let write_time_buffer = Duration::from_millis(150);
        // Tell the caller to check back an extra 50 milliseconds after we would be ready to write
        // the next interval.
        let next_write_buffer = write_time_buffer + Duration::from_millis(50);

        self.flush_callsites();

        let mut written_chunks: VecDeque<WrittenChunk> = VecDeque::new();

        let mut idx = 0;
        let write_result = loop {
            let Some(chunk_buffer) = chunks.chunk_buffers.get(idx) else {
                break Ok(());
            };

            let end_time = chunk_buffer.header.interval.abs_end_time();
            let since_completion = AbsTimestamp::now()
                .as_duration_since_epoch()
                .saturating_sub(end_time.as_duration_since_epoch());

            if since_completion > write_time_buffer {
                let mut writer = match self.writer_for_chunk(chunk_buffer) {
                    Ok(writer) => writer,
                    Err(err) => break Err(err),
                };
                chunk_buffer
                    .write(&mut writer)
                    .map_err(|inner| WriteChunksError::write(chunk_buffer, inner))?;
                written_chunks.push_front(writer.into());

                self.notifiers
                    .lock()
                    .expect("cannot notify written. notifiers poisoned")
                    .retain(|notifier| {
                        // Only keep notifiers which we are too early for.
                        notifier.notify_after_ts(&end_time) == NotifyAfterTs::TooEarly
                    });

                // TODO(hds): Perhaps retain the completed sequence chunks to avoid allocating again?
                chunks.chunk_buffers.remove(idx);
            } else {
                idx += 1;
            }
        };

        // TODO(hds): We will end up in an inconsistent state if this thread panics between
        // removing chunk bufferes and adding written chunks. Ideally, we need to make sure that
        // nothing will panic here.
        //
        // On the other hand, if the lock around chunks **is** poisoned then we will panic next
        // time we try to access it, so the inconsistent state won't be externally visible.
        for written_chunk in written_chunks {
            chunks.written_chunks.push_front(written_chunk);
        }

        // Once written chunks has been updated, we can safely return early if there was an error.
        write_result?;

        // TODO(hds): Flush the callsites again afterwards to ensure consistency?

        let now = AbsTimestamp::now();
        let interval =
            ChunkInterval::from_timestamp_and_period(now.clone(), self.chunk_period_micros as u64);

        let next_write_in = (interval.abs_end_time().as_duration_since_epoch() + next_write_buffer)
            .saturating_sub(now.as_duration_since_epoch());
        Ok(next_write_in)
    }

    /// Write all stored chunks to disk.
    ///
    /// The chunks are not discarded after being written. If further records are written to the
    /// contained sequence chunks, then they can be written to disk at a later time with subsequent
    /// calls to [`write_completed_chunks`] or [`write_all_chunks`].
    ///
    /// Note that chunks written in this way aren't considered for the purposes of the storage
    /// quota when calling [`cleanup_outdated_chunks`].
    pub fn write_all_chunks(&self) -> Result<(), WriteChunksError> {
        // Flush the callsites first
        self.flush_callsites();

        let chunks = self.chunks.lock().expect("poisoned");

        for chunk_buffer in &chunks.chunk_buffers {
            let mut writer = self.writer_for_chunk(chunk_buffer)?;
            chunk_buffer
                .write(&mut writer)
                .map_err(|inner| WriteChunksError::write(chunk_buffer, inner))?;
        }

        // TODO(hds): Flush the callsites again afterwards to ensure consistency?

        Ok(())
    }

    /// Wait for the current active chunk to be written to disk.
    ///
    /// Chunks are normally written to disk following a short delay after their completion time.
    /// This method takes this delay into account, it takes the time this method is called and
    /// waits until the chunk where a record is buffered at that time is written to disk, not just
    /// the next chunk (which may not contain the hypothetical record) is written.
    pub fn wait_for_write_timeout(&self, timeout_dur: Duration) -> Result<(), WaitForWriteError> {
        let now = AbsTimestamp::now();
        let notifier = ChunkWriteNotifier::new(now);
        self.notifiers
            .lock()
            .expect("cannot wait for write. notifiers poisoned")
            .push(notifier.clone());

        match notifier.wait_for_write_timeout(timeout_dur) {
            WaitForWrite::Written => Ok(()),
            WaitForWrite::Timeout => Err(WaitForWriteError::Timeout),
            WaitForWrite::Closed => Err(WaitForWriteError::Closed),
        }
    }

    fn writer_for_chunk(&self, chunk: &ChunkBuffer) -> Result<ChunkWriter, WriteChunksError> {
        let path = self
            .chunk_path(chunk)
            .map_err(|inner| WriteChunksError::chunk_path(chunk, inner))?;
        let file = {
            match fs::File::create(&path) {
                Ok(file) => Ok(file),
                Err(_err) => {
                    self.ensure_dir(&chunk.header.interval.base_time)
                        .map_err(|inner| WriteChunksError::chunk_dir(chunk, inner))?;
                    fs::File::create(&path)
                }
            }
            .map_err(|inner| WriteChunksError::file_open(chunk, inner))?
        };

        Ok(ChunkWriter { file, path, len: 0 })
    }

    fn flush_callsites(&self) {
        let mut callsites_writer = self
            .callsites_writer
            .lock()
            .expect("callsites writer mutex poisoned");

        if let Err(flush_error) = callsites_writer.flush() {
            eprintln!("Failed to flush callsites. Recording may be inconsistent: {flush_error}");
        }
    }

    pub fn clean_outdated_chunks(&self) -> Result<(), CleanChunksError> {
        let mut chunks_to_keep = 0;
        let mut total_size = 0;

        let mut chunks = self.chunks.lock().expect("poisoned");

        // First we iterate through written chunks in order newest to oldest until we go over the
        // keep quota.
        for written_chunk in &chunks.written_chunks {
            total_size += written_chunk.size_bytes;
            let chunk_count = chunks_to_keep + 1;

            if self
                .storage_quota
                .is_above_any_hard_max(chunk_count, total_size)
                || (self
                    .storage_quota
                    .is_above_any_soft_max(chunk_count, total_size)
                    && self.storage_quota.is_above_all_min(chunk_count, total_size))
            {
                break;
            }

            chunks_to_keep = chunk_count;
        }

        // Now we know how many chunks to keep, we pop off chunks from the oldest end until we only
        // have that many left.
        let chunks_to_delete = chunks.written_chunks.len() - chunks_to_keep;
        while chunks.written_chunks.len() > chunks_to_keep {
            let Some(written_chunk) = chunks.written_chunks.pop_back() else {
                // We have somehow runout of written chunks, break from the loop.
                break;
            };

            fs::remove_file(&written_chunk.path).map_err(|io_err| {
                CleanChunksError::RemoveChunkFailed {
                    chunk_path: written_chunk.path,
                    chunks_to_delete,
                    chunks_deleted: chunks_to_delete
                        - (chunks.written_chunks.len() - chunks_to_keep),
                    inner: io_err,
                }
            })?;
        }

        Ok(())
    }
}

struct ChunkWriter {
    file: fs::File,
    path: PathBuf,
    len: usize,
}

impl io::Write for &mut ChunkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.file.write(buf) {
            Ok(len) => {
                self.len += len;
                Ok(len)
            }
            Err(e) => Err(e),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl From<ChunkWriter> for WrittenChunk {
    fn from(writer: ChunkWriter) -> Self {
        WrittenChunk {
            path: writer.path,
            size_bytes: writer.len,
        }
    }
}

/// Error waiting for a chunk to be written
#[derive(Debug, Clone, Copy)]
pub enum WaitForWriteError {
    /// The provided timeout was reached before the chunk was written.
    Timeout,
    /// The chunk writer was closed before the chunk was written.
    Closed,
}

impl fmt::Display for WaitForWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Timeout => "timeout was reached before chunk was written",
                Self::Closed => "chunk writer was closed before chunk was written",
            }
        )
    }
}

impl error::Error for WaitForWriteError {}

#[derive(Debug, Clone)]
struct ChunkWriteNotifier {
    ts: AbsTimestamp,
    pair: Arc<(Mutex<Option<WaitForWrite>>, Condvar)>,
}

impl ChunkWriteNotifier {
    fn new(ts: AbsTimestamp) -> Self {
        Self {
            ts,
            pair: Arc::new((Mutex::new(None), Condvar::new())),
        }
    }

    fn notify_after_ts(&self, chunk_end_time: &AbsTimestamp) -> NotifyAfterTs {
        if &self.ts > chunk_end_time {
            return NotifyAfterTs::TooEarly;
        }

        self.notify(WaitForWrite::Written);
        NotifyAfterTs::Notified
    }

    fn notify(&self, val: WaitForWrite) {
        let (lock, cvar) = &*self.pair;
        let mut written = lock
            .lock()
            .expect("can't notify. chunk writer notifier poisoned");

        *written = Some(val);
        cvar.notify_one();
    }

    fn wait_for_write_timeout(&self, timeout_dur: Duration) -> WaitForWrite {
        let (lock, cvar) = &*self.pair;
        let mut written = lock
            .lock()
            .expect("can't wait. chunk writer notifier poisoned");
        let wait_until = Instant::now() + timeout_dur;
        loop {
            let timeout_dur = wait_until.saturating_duration_since(Instant::now());
            if timeout_dur.is_zero() {
                break WaitForWrite::Timeout;
            }
            let (guard, timeout) = cvar
                .wait_timeout(written, timeout_dur)
                .expect("can't wait. chunk writer notifier poisoned");
            written = guard;
            if let Some(val) = *written {
                // Even if we have timed out, if we have a value, we return that.
                break val;
            } else if timeout.timed_out() {
                break WaitForWrite::Timeout;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum NotifyAfterTs {
    Notified,
    TooEarly,
}

#[derive(Debug, Clone, Copy)]
enum WaitForWrite {
    Written,
    Closed,
    Timeout,
}

/// An error occuring when creating a new [`ChunkedWriter`].
#[derive(Debug)]
pub struct NewChunkedWriterError {
    kind: NewChunkedWriterErrorKind,
}

impl NewChunkedWriterError {
    fn already_exists() -> Self {
        Self {
            kind: NewChunkedWriterErrorKind::AlreadyExists,
        }
    }

    fn create_recording_dir_failed(inner: io::Error) -> Self {
        Self {
            kind: NewChunkedWriterErrorKind::CreateRecordingDirFailed(inner),
        }
    }

    fn write_meta_failed(inner: WriteError) -> Self {
        Self {
            kind: NewChunkedWriterErrorKind::WriteMetaFailed(inner),
        }
    }

    fn write_callsites_failed(inner: WriteError) -> Self {
        Self {
            kind: NewChunkedWriterErrorKind::WriteCallsitesFailed(inner),
        }
    }
}

#[derive(Debug)]
pub enum NewChunkedWriterErrorKind {
    /// There is already a chunked recording at this location
    AlreadyExists,
    /// Could not create the directory for the chunked recording
    CreateRecordingDirFailed(io::Error),
    /// There was a failure writing the meta file
    WriteMetaFailed(WriteError),
    /// There was a failure writing the callsites file
    WriteCallsitesFailed(WriteError),
}

impl fmt::Display for NewChunkedWriterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            NewChunkedWriterErrorKind::AlreadyExists => {
                write!(f, "a chunked recording already exists at this location")
            }
            NewChunkedWriterErrorKind::CreateRecordingDirFailed(inner) => {
                write!(f, "parent directory could not be created: {inner}")
            }
            NewChunkedWriterErrorKind::WriteMetaFailed(inner) => {
                write!(f, "failed to write `meta.rfr`: {inner}")
            }
            NewChunkedWriterErrorKind::WriteCallsitesFailed(inner) => {
                write!(f, "failed to write `callsites.rfr` file: {inner}")
            }
        }
    }
}
impl error::Error for NewChunkedWriterError {}

impl From<NewMetaError> for NewChunkedWriterError {
    fn from(value: NewMetaError) -> Self {
        match value {
            NewMetaError::AlreadyExists => NewChunkedWriterError::already_exists(),
            NewMetaError::WriteFailed(inner) => NewChunkedWriterError::write_meta_failed(inner),
        }
    }
}

#[derive(Debug)]
enum NewMetaError {
    AlreadyExists,
    WriteFailed(WriteError),
}

impl fmt::Display for NewMetaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists => {
                write!(
                    f,
                    "cannot write `meta.rfr`: file already exists. \
                    There is probably already a chunked recording at this location"
                )
            }
            Self::WriteFailed(inner) => inner.fmt(f),
        }
    }
}

impl error::Error for NewMetaError {}

/// Error occuring when writing a serialized file that is part of a chunked recording.
#[derive(Debug)]
pub enum WriteError {
    /// An IO error occurred when creating the file.
    Io(io::Error),
    /// An error occurred when writing the serialized contents of the file.
    Serialization(postcard::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(inner) => write!(f, "IO error: {}", inner),
            Self::Serialization(inner) => {
                write!(f, "serialization error: {}", inner)
            }
        }
    }
}

impl error::Error for WriteError {}

// Error writing chunks out to storage
#[derive(Debug)]
pub struct WriteChunksError {
    chunk_interval: ChunkInterval,
    kind: WriteChunksErrorKind,
}

impl WriteChunksError {
    fn chunk_path(chunk: &ChunkBuffer, inner: io::Error) -> Self {
        Self {
            chunk_interval: chunk.header.interval.clone(),
            kind: WriteChunksErrorKind::ChunkPath { inner },
        }
    }

    fn chunk_dir(chunk: &ChunkBuffer, inner: io::Error) -> Self {
        Self {
            chunk_interval: chunk.header.interval.clone(),
            kind: WriteChunksErrorKind::ChunkDir { inner },
        }
    }

    fn file_open(chunk: &ChunkBuffer, inner: io::Error) -> Self {
        Self {
            chunk_interval: chunk.header.interval.clone(),
            kind: WriteChunksErrorKind::FileOpen { inner },
        }
    }

    fn write(chunk: &ChunkBuffer, inner: ChunkWriteError) -> Self {
        Self {
            chunk_interval: chunk.header.interval.clone(),
            kind: WriteChunksErrorKind::Write { inner },
        }
    }
}

#[derive(Debug)]
enum WriteChunksErrorKind {
    ChunkPath { inner: io::Error },
    ChunkDir { inner: io::Error },
    FileOpen { inner: io::Error },
    Write { inner: ChunkWriteError },
}

impl fmt::Display for WriteChunksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let chunk_interval = &self.chunk_interval;
        let (kind, inner) = match &self.kind {
            WriteChunksErrorKind::ChunkPath { inner } => ("chunk path", inner),
            WriteChunksErrorKind::ChunkDir { inner } => ("chunk dir", inner),
            WriteChunksErrorKind::FileOpen { inner } => ("file open", inner),
            WriteChunksErrorKind::Write { inner } => {
                return write!(
                    f,
                    "failed to write serialized data to chunk `{chunk_interval}`: {inner}"
                );
            }
        };

        write!(
            f,
            "failed to write chunk `{chunk_interval}`, {kind}: {inner}"
        )
    }
}

impl error::Error for WriteChunksError {}

#[non_exhaustive]
#[derive(Debug)]
pub enum CleanChunksError {
    Unknown,
    RemoveChunkFailed {
        chunk_path: PathBuf,
        chunks_to_delete: usize,
        chunks_deleted: usize,
        inner: io::Error,
    },
}

impl fmt::Display for CleanChunksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanChunksError::Unknown => write!(f, "{self:?}"),
            CleanChunksError::RemoveChunkFailed {
                chunk_path,
                chunks_to_delete,
                chunks_deleted,
                inner,
            } => {
                write!(
                    f,
                    "Failed to delete chunk at path: {chunk_path} \
                    (chunks deleted: {chunks_deleted}, chunks to delete: {chunks_to_delete}): \
                    {inner}",
                    chunk_path = chunk_path.as_path().to_string_lossy(),
                )
            }
        }
    }
}

impl error::Error for CleanChunksError {}

#[derive(Debug)]
pub struct ChunkBuffer {
    header: ChunkHeader,

    seq_chunks: Vec<Arc<SeqChunkBuffer>>,
}

impl ChunkBuffer {
    fn new(interval: ChunkInterval) -> Self {
        Self {
            header: ChunkHeader::new(interval),
            seq_chunks: Vec::new(),
        }
    }

    fn new_seq_chunk_buffer(&mut self) -> Arc<SeqChunkBuffer> {
        let seq_chunk_buffer = Arc::new(SeqChunkBuffer::new(self.header.interval.clone()));
        self.seq_chunks.push(Arc::clone(&seq_chunk_buffer));
        seq_chunk_buffer
    }

    fn write(&self, writer: impl io::Write) -> Result<(), ChunkWriteError> {
        let mut writer = writer;

        postcard::to_io(&current_software_version(), &mut writer)
            .map_err(ChunkWriteError::identifier)?;

        let (earliest_timestamp, latest_timestamp) = self
            .seq_chunks
            .iter()
            .map(|seq_chunk| (seq_chunk.earliest_timestamp(), seq_chunk.latest_timestamp()))
            .fold(
                (self.header.earliest_timestamp, self.header.latest_timestamp),
                |(acc_earliest, acc_latest), (earliest, latest)| {
                    (acc_earliest.min(earliest), acc_latest.max(latest))
                },
            );
        let header = ChunkHeader {
            interval: self.header.interval.clone(),
            earliest_timestamp,
            latest_timestamp,
        };
        postcard::to_io(&header, &mut writer).map_err(ChunkWriteError::header)?;

        postcard::to_io(&self.seq_chunks.len(), &mut writer)
            .map_err(ChunkWriteError::seq_length)?;
        for seq_chunk in &self.seq_chunks {
            seq_chunk
                .write(&mut writer)
                .map_err(|inner| ChunkWriteError::seq_chunk(seq_chunk.seq_id(), inner))?;
        }

        Ok(())
    }
}

/// Error writing a single chunk
#[derive(Debug)]
pub struct ChunkWriteError {
    kind: ChunkWriteErrorKind,
}

impl ChunkWriteError {
    fn identifier(inner: postcard::Error) -> Self {
        Self {
            kind: ChunkWriteErrorKind::Identifier { inner },
        }
    }

    fn header(inner: postcard::Error) -> Self {
        Self {
            kind: ChunkWriteErrorKind::Header { inner },
        }
    }

    fn seq_length(inner: postcard::Error) -> Self {
        Self {
            kind: ChunkWriteErrorKind::SeqLength { inner },
        }
    }

    fn seq_chunk(seq_id: SeqId, inner: SeqChunkWriteError) -> Self {
        Self {
            kind: ChunkWriteErrorKind::SeqChunk { seq_id, inner },
        }
    }
}

impl fmt::Display for ChunkWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, inner) = match &self.kind {
            ChunkWriteErrorKind::SeqChunk { seq_id, inner } => {
                return write!(
                    f,
                    "failed to write sequence chunk `{seq_id}`: {inner}",
                    seq_id = seq_id.as_u64()
                );
            }
            ChunkWriteErrorKind::Identifier { inner } => ("identifier", inner),
            ChunkWriteErrorKind::Header { inner } => ("header", inner),
            ChunkWriteErrorKind::SeqLength { inner } => ("sequence length", inner),
        };

        write!(f, "Failed to write chunk {kind}: {inner}")
    }
}

impl error::Error for ChunkWriteError {}

#[derive(Debug)]
enum ChunkWriteErrorKind {
    Identifier {
        inner: postcard::Error,
    },
    Header {
        inner: postcard::Error,
    },
    SeqLength {
        inner: postcard::Error,
    },
    SeqChunk {
        seq_id: SeqId,
        inner: SeqChunkWriteError,
    },
}
