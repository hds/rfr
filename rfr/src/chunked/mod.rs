use std::{
    cell::RefCell,
    error, fmt, fs,
    io::{self, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{self, AtomicBool},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};

use jiff::{tz::TimeZone, Timestamp, Zoned};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::{
    common::{Span, Task},
    rec::{self, AbsTimestamp},
    FormatIdentifier, FormatVariant,
};

mod callsite;
mod meta;
mod record;
mod sequence;

pub use callsite::{
    Callsite, CallsiteId, ChunkedCallsites, ChunkedCallsitesWriter, FlushCallsitesError,
};
pub use meta::{ChunkedMeta, ChunkedMetaHeader, MetaTryFromIoError};
pub use record::{Meta, Record, RecordData};
pub use sequence::{SeqChunk, SeqChunkBuffer};

fn current_software_version() -> FormatIdentifier {
    FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 0,
        minor: 0,
        patch: 2,
    }
}

#[derive(Debug)]
pub struct ChunkedWriter {
    root_dir: PathBuf,
    base_time: AbsTimestampSecs,

    /// The length of time a chunk is "responsible" for. This value must either be a multiple of
    /// seconds (multiple of 1_000_000) or a divisor of a whole second (divisor of 1_000_000).
    chunk_period_micros: u32,

    closed: AtomicBool,

    callsites_writer: Mutex<ChunkedCallsitesWriter<fs::File>>,
    chunk_buffers: Mutex<Vec<ChunkBuffer>>,
    notifiers: Mutex<Vec<ChunkWriteNotifier>>,
}

impl ChunkedWriter {
    pub fn try_new<P>(root_dir: P) -> Result<Self, NewChunkedWriterError>
    where
        P: AsRef<Path>,
    {
        let root_dir = root_dir.as_ref();

        let timestamp = rec::AbsTimestamp::now();
        let base_time = AbsTimestampSecs::from(timestamp.clone());
        let meta = ChunkedMeta::new(vec![current_software_version()]);

        if let Ok(true) = root_dir.try_exists() {
            return Err(NewChunkedWriterError::AlreadyExists);
        }

        fs::create_dir_all(root_dir).map_err(NewChunkedWriterError::CreateRecordingDirFailed)?;
        Self::write_meta(root_dir, &meta)?;

        let callsites_path = Path::new(&root_dir).join("callsites.rfr");
        let callsites_file = fs::File::create(callsites_path)
            .map_err(|err| NewChunkedWriterError::WriteCallsitesFailed(WriteError::Io(err)))?;
        let callsites_writer = ChunkedCallsitesWriter::try_new(callsites_file)
            .map_err(NewChunkedWriterError::WriteCallsitesFailed)?;

        // By default, chunks contain 1 second of execution time.
        let chunk_period_micros = 1_000_000;
        let writer = Self {
            root_dir: root_dir.to_owned(),
            base_time,
            chunk_period_micros,
            closed: false.into(),
            callsites_writer: Mutex::new(callsites_writer),
            chunk_buffers: Mutex::new(Vec::new()),
            notifiers: Mutex::new(Vec::new()),
        };

        let base_time = writer.base_time;
        writer.ensure_dir(&base_time);

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

    fn ensure_dir(&self, time: &AbsTimestampSecs) {
        fs::create_dir_all(self.dir_path(time)).unwrap();
    }

    fn dir_path(&self, time: &AbsTimestampSecs) -> PathBuf {
        let ts = Timestamp::from_second(time.secs as i64).unwrap();
        let ts_utc = ts.to_zoned(TimeZone::UTC);
        self.dir_path_from_utc(&ts_utc)
    }

    fn dir_path_from_utc(&self, ts_utc: &Zoned) -> PathBuf {
        Path::new(&self.root_dir)
            .join(format!("{}", ts_utc.strftime("%Y-%m")))
            .join(format!("{}", ts_utc.strftime("%d-%H")))
    }

    fn chunk_path(&self, time: &AbsTimestampSecs) -> PathBuf {
        let ts = Timestamp::from_second(time.secs as i64).unwrap();
        let ts_utc = ts.to_zoned(TimeZone::UTC);

        self.dir_path_from_utc(&ts_utc)
            .join(format!("chunk-{}.rfr", ts_utc.strftime("%M-%S")))
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
        timestamp: rec::AbsTimestamp,
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
        let mut chunk_buffers = self.chunk_buffers.lock().expect("poisoned");
        let chunk_buffer = chunk_buffers
            .iter_mut()
            .find(|cb| cb.header.interval == interval);
        match chunk_buffer {
            Some(chunk_buffer) => chunk_buffer.new_seq_chunk_buffer(),
            None => {
                let mut new_chunk_buffer = ChunkBuffer::new(interval.clone());
                let seq_chunk_buffer = new_chunk_buffer.new_seq_chunk_buffer();
                chunk_buffers.push(new_chunk_buffer);
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
    /// very long time to prepare an even before calling [`with_seq_chunk_buffer`], then a record
    /// may get lost.
    ///
    /// For this reason, [`with_seq_chunk_buffer`] should be called with a timestamp that is close
    /// to the current time.
    pub fn write_completed_chunks(&self) -> Result<Duration, WriteChunksError> {
        let mut chunk_buffers = self.chunk_buffers.lock().expect("poisoned");
        let write_time_buffer = Duration::from_millis(150);
        // Tell the caller to check back an extra 50 milliseconds after we would be ready to write
        // the next interval.
        let next_write_buffer = write_time_buffer + Duration::from_millis(50);

        self.flush_callsites();

        chunk_buffers.retain(|chunk_buffer| {
            let end_time = chunk_buffer.header.interval.abs_end_time();
            let since_completion = AbsTimestamp::now()
                .as_duration_since_epoch()
                .saturating_sub(end_time.as_duration_since_epoch());
            if since_completion > write_time_buffer {
                let writer = self.writer_for_chunk(chunk_buffer);
                // TODO(hds): Check for errors
                chunk_buffer.write(writer);

                self.notifiers
                    .lock()
                    .expect("cannot notify written. notifiers poisoned")
                    .retain(|notifier| {
                        // Only keep notifiers which we are too early for.
                        notifier.notify_after_ts(&end_time) == NotifyAfterTs::TooEarly
                    });

                // TODO(hds): Perhaps retain the completed sequence chunks to avoid allocating again?
                false
            } else {
                true
            }
        });

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
    pub fn write_all_chunks(&self) {
        // Flush the callsites first
        self.flush_callsites();

        let chunk_buffers = self.chunk_buffers.lock().expect("poisoned");

        chunk_buffers.iter().for_each(|chunk_buffer| {
            let writer = self.writer_for_chunk(chunk_buffer);
            chunk_buffer.write(writer);
        });

        // TODO(hds): Flush the callsites again afterwards to ensure consistency?
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

    fn writer_for_chunk(&self, chunk: &ChunkBuffer) -> impl io::Write {
        fs::File::create(self.chunk_path(&chunk.header.interval.base_time)).unwrap()
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
pub enum NewChunkedWriterError {
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
        match self {
            Self::AlreadyExists => write!(f, "a chunked recording already exists at this location"),
            Self::CreateRecordingDirFailed(inner) => {
                write!(f, "parent directory could not be created: {inner}")
            }
            Self::WriteMetaFailed(inner) => write!(f, "failed to write `meta.rfr`: {inner}"),
            Self::WriteCallsitesFailed(inner) => {
                write!(f, "failed to write `callsites.rfr` file: {inner}")
            }
        }
    }
}
impl error::Error for NewChunkedWriterError {}

impl From<NewMetaError> for NewChunkedWriterError {
    fn from(value: NewMetaError) -> Self {
        match value {
            NewMetaError::AlreadyExists => NewChunkedWriterError::AlreadyExists,
            NewMetaError::WriteFailed(inner) => NewChunkedWriterError::WriteMetaFailed(inner),
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

#[non_exhaustive]
#[derive(Debug)]
pub enum WriteChunksError {
    FileOpenFailed,
}

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

    fn write(&self, writer: impl io::Write) {
        let mut writer = writer;

        postcard::to_io(&current_software_version(), &mut writer).unwrap();

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
        postcard::to_io(&header, &mut writer).unwrap();

        postcard::to_io(&self.seq_chunks.len(), &mut writer).unwrap();
        for seq_chunk in &self.seq_chunks {
            seq_chunk.write(&mut writer);
        }
    }
}

/// A timestamp measured from the [`UNIX_EPOCH`].
///
/// This timestamp is absoluteand only contains the whole seconds. No sub-second component is
/// stored.
#[derive(Debug, Clone, Copy, Hash, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct AbsTimestampSecs {
    /// Whole seconds component of the timestamp, measured from the [`UNIX_EPOCH`].
    pub secs: u64,
}

impl From<rec::AbsTimestamp> for AbsTimestampSecs {
    fn from(value: rec::AbsTimestamp) -> Self {
        Self { secs: value.secs }
    }
}

impl AbsTimestampSecs {
    pub const ZERO: Self = Self { secs: 0 };

    pub fn as_micros(&self) -> u64 {
        self.secs * 1_000_000
    }
}

// A timestamp within a chunk.
//
// A chunk timestamp represents the time of a record with respect to the chunk's base time. It is
// stored as the number of microseconds since the base time. All records within a chunk must occur
// at the base time or afterwards.
#[derive(Clone, Copy, Debug, Hash, PartialEq, PartialOrd, Ord, Eq, Deserialize, Serialize)]
pub struct ChunkTimestamp {
    /// Microseconds since the chunk's base time
    pub micros: u64,
}

impl ChunkTimestamp {
    const ZERO: ChunkTimestamp = ChunkTimestamp { micros: 0 };

    pub fn new(micros: u64) -> Self {
        Self { micros }
    }

    /// Create a new chunk timestamp from a base time and an absolute timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{chunked::{AbsTimestampSecs, ChunkTimestamp}, rec::AbsTimestamp};
    /// #
    /// let now = AbsTimestamp::now();
    /// let base_time = AbsTimestampSecs::from(now.clone());
    /// let chunk_ts = ChunkTimestamp::from_base_and_timestamp(base_time, &now);
    ///
    /// # let also_now = chunk_ts.to_abs_timestamp(base_time);
    /// # assert_eq!(now, also_now);
    /// ```
    pub fn from_base_and_timestamp(base_time: AbsTimestampSecs, timestamp: &AbsTimestamp) -> Self {
        let secs = timestamp.secs.saturating_sub(base_time.secs);
        let micros = (secs * 1_000_000) + timestamp.subsec_micros as u64;
        Self::new(micros)
    }

    /// Convert to an absolute timestamp, given the base timestamp for this chunk.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{chunked::{AbsTimestampSecs, ChunkTimestamp}, rec::AbsTimestamp};
    /// #
    /// # let now = AbsTimestamp::now();
    /// # let base_time = AbsTimestampSecs::from(now.clone());
    /// let chunk_ts = ChunkTimestamp::from_base_and_timestamp(base_time, &now);
    ///
    /// let also_now = chunk_ts.to_abs_timestamp(base_time);
    /// # assert_eq!(now, also_now);
    /// ```
    pub fn to_abs_timestamp(&self, base_time: AbsTimestampSecs) -> AbsTimestamp {
        abs_timestamp(base_time, self)
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum Object {
    Span(Span),
    Task(Task),
}

#[derive(Debug)]
pub struct Recording {
    meta: ChunkedMeta,
    chunks: Vec<ChunkLoader>,
}

impl Recording {
    pub fn load_all_chunks(&mut self) {
        for chunk_loader in &mut self.chunks {
            chunk_loader.ensure_chunk();
        }
    }

    pub fn meta(&self) -> &ChunkedMeta {
        &self.meta
    }

    pub fn chunks_lossy(&mut self) -> impl DoubleEndedIterator<Item = Option<&Chunk>> {
        self.chunks.iter_mut().map(|loader| {
            loader.ensure_chunk();
            match &loader.state {
                ChunkLoaderState::Chunk(chunk) => Some(chunk),
                _ => None,
            }
        })
    }

    pub fn chunk_headers_lossy(&mut self) -> impl DoubleEndedIterator<Item = Option<&ChunkHeader>> {
        self.chunks.iter_mut().map(|loader| {
            loader.ensure_header();
            match &loader.state {
                ChunkLoaderState::Unloaded => None,
                ChunkLoaderState::Header(header) => Some(header),
                ChunkLoaderState::Chunk(chunk) => Some(chunk.header()),
            }
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Chunk {
    header: ChunkHeader,
    seq_chunks: Vec<SeqChunk>,
}

impl Chunk {
    pub fn header(&self) -> &ChunkHeader {
        &self.header
    }

    pub fn seq_chunks(&self) -> &Vec<SeqChunk> {
        &self.seq_chunks
    }

    pub fn abs_timestamp(&self, chunk_timestamp: &ChunkTimestamp) -> AbsTimestamp {
        abs_timestamp(self.header.interval.base_time, chunk_timestamp)
    }

    fn try_from_io<IO>(reader: IO) -> Result<Self, ChunkReadError>
    where
        IO: io::Read + io::Seek,
    {
        let mut reader = reader;
        let mut end_pos = reader
            .seek(SeekFrom::End(0))
            .map_err(|_| ChunkReadError::ReadError)?;
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|_| ChunkReadError::ReadError)?;

        let mut buffer = vec![0_u8; 1024];
        let mut file_buffer = (&mut reader, buffer.as_mut_slice());

        // TODO(hds): Should we validate the identifier?
        let (_identifier, _): (FormatIdentifier, _) = postcard::from_io(file_buffer).unwrap();

        let (header, _): (ChunkHeader, _) =
            postcard::from_io((&mut reader, Vec::new().as_mut_slice())).unwrap();

        let (seq_chunk_len, _): (usize, _) =
            postcard::from_io((&mut reader, Vec::new().as_mut_slice())).unwrap();
        let mut seq_chunks: Vec<SeqChunk> = Vec::with_capacity(seq_chunk_len);

        file_buffer = (&mut reader, buffer.as_mut_slice());

        'seq_chunk: for idx in 0..seq_chunk_len {
            let seq_chunk_result = loop {
                let Ok(file_pos) = file_buffer.0.stream_position() else {
                    println!("at {idx} cannot get file position");
                    break 'seq_chunk;
                };

                if file_pos >= end_pos {
                    let Ok(new_end_pos) = file_buffer.0.seek(SeekFrom::End(0)) else {
                        println!("at {idx} cannot get file length");
                        break 'seq_chunk;
                    };
                    if new_end_pos <= end_pos {
                        break 'seq_chunk;
                    }

                    end_pos = new_end_pos;
                    let Ok(_) = file_buffer.0.seek(SeekFrom::Start(0)) else {
                        println!("at {idx} cannot seek back to previous file position");
                        break 'seq_chunk;
                    };
                    // Start loop from the beginning, even if this means we need to get the stream
                    // position again.
                    continue;
                }

                break match postcard::from_io(file_buffer) {
                    Ok(result) => result,
                    Err(postcard::Error::DeserializeUnexpectedEnd) => {
                        let new_size = buffer.len() * 2;
                        const MAX_BUFFER_SIZE: usize = 1 << 20; // 1 MiB
                        if new_size > MAX_BUFFER_SIZE {
                            println!(
                                "excessive buffer required for element (> {MAX_BUFFER_SIZE}), skipping"
                            );
                            file_buffer = (&mut reader, buffer.as_mut_slice());
                            continue 'seq_chunk;
                        }
                        buffer.resize(new_size * 2, 0);
                        if let Err(err) = reader.seek(SeekFrom::Start(file_pos)) {
                            println!("Could not seek back to start of element after making buffer bigger: {err}");
                            file_buffer = (&mut reader, buffer.as_mut_slice());
                            continue 'seq_chunk;
                        }

                        // We've successfully increased the buffer size, we'll loop back around and
                        // try to read this element again.
                        file_buffer = (&mut reader, buffer.as_mut_slice());
                        continue;
                    }
                    Err(error) => {
                        return Err(ChunkReadError::DeserializeError { index: idx, error });
                    }
                };
            };

            seq_chunks.push(seq_chunk_result.0);
            file_buffer = (seq_chunk_result.1 .0, buffer.as_mut_slice());
        }

        Ok(Self { header, seq_chunks })
    }
}

fn abs_timestamp(base_time: AbsTimestampSecs, chunk_timestamp: &ChunkTimestamp) -> AbsTimestamp {
    let chunk_timestamp_secs = chunk_timestamp.micros / 1_000_000;
    let chunk_timestamp_subsec_micros = (chunk_timestamp.micros % 1_000_000) as u32;

    AbsTimestamp {
        secs: base_time.secs + chunk_timestamp_secs,
        subsec_micros: chunk_timestamp_subsec_micros,
    }
}

#[derive(Clone, Debug)]
pub enum ChunkReadError {
    Unimplemented,
    ReadError,
    DeserializeError {
        index: usize,
        error: postcard::Error,
    },
}

impl fmt::Display for ChunkReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl error::Error for ChunkReadError {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChunkHeader {
    pub interval: ChunkInterval,

    pub earliest_timestamp: ChunkTimestamp,
    pub latest_timestamp: ChunkTimestamp,
}

impl ChunkHeader {
    fn new(interval: ChunkInterval) -> Self {
        let earliest_timestamp = interval.end_time;
        let latest_timestamp = interval.start_time;

        Self {
            interval,
            earliest_timestamp,
            latest_timestamp,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkInterval {
    base_time: AbsTimestampSecs,
    start_time: ChunkTimestamp,
    end_time: ChunkTimestamp,
}

impl ChunkInterval {
    pub fn from_timestamp_and_period(timestamp: AbsTimestamp, period_micros: u64) -> Self {
        let (base_time, start_time) = if period_micros > 1_000_000 {
            let secs = AbsTimestampSecs::from(timestamp.clone());
            (
                AbsTimestampSecs {
                    secs: secs.secs - (secs.secs % (period_micros / 1_000_000)),
                },
                // Since the period is in whole seconds, the start offset is always 0.
                ChunkTimestamp::ZERO,
            )
        } else {
            (
                AbsTimestampSecs::from(timestamp.clone()),
                // Calculate the start time (offset) based on the period.
                ChunkTimestamp::new(
                    (timestamp.subsec_micros - (timestamp.subsec_micros % period_micros as u32))
                        as u64,
                ),
            )
        };

        let end_time = ChunkTimestamp::new(start_time.micros + period_micros);

        Self {
            base_time,
            start_time,
            end_time,
        }
    }

    /// The start time of the interval as an absolute timestamp
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{chunked::ChunkInterval, rec::AbsTimestamp};
    /// let now = AbsTimestamp::now();
    /// let interval = ChunkInterval::from_timestamp_and_period(now.clone(), 1_000_000);
    ///
    /// let start_time = interval.abs_start_time();
    /// assert!(start_time <= now);
    /// ```
    pub fn abs_start_time(&self) -> AbsTimestamp {
        self.start_time.to_abs_timestamp(self.base_time)
    }

    /// The end time of the interval as an absolute timestamp
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{chunked::ChunkInterval, rec::AbsTimestamp};
    /// let now = AbsTimestamp::now();
    /// let interval = ChunkInterval::from_timestamp_and_period(now.clone(), 1_000_000);
    ///
    /// let end_time = interval.abs_end_time();
    /// assert!(end_time >= now);
    /// ```
    pub fn abs_end_time(&self) -> AbsTimestamp {
        self.end_time.to_abs_timestamp(self.base_time)
    }
}

#[derive(Debug)]
pub struct ChunkPath {
    path: PathBuf,
}

impl ChunkPath {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Debug)]
pub struct ChunkLoader {
    path: ChunkPath,
    state: ChunkLoaderState,
}

#[derive(Debug)]
enum ChunkLoaderState {
    Unloaded,
    Header(ChunkHeader),
    Chunk(Chunk),
}

impl From<ChunkPath> for ChunkLoader {
    fn from(value: ChunkPath) -> Self {
        ChunkLoader {
            path: value,
            state: ChunkLoaderState::Unloaded,
        }
    }
}

impl ChunkLoader {
    pub fn path(&self) -> &PathBuf {
        &self.path.path
    }

    fn ensure_header(&mut self) {
        if let ChunkLoaderState::Unloaded = self.state {
            let mut file = fs::File::open(&self.path.path).unwrap();
            let mut buffer = vec![0_u8; 24];
            let (_identifier, _): (FormatIdentifier, _) =
                postcard::from_io((&mut file, buffer.as_mut_slice())).unwrap();

            let (header, _) = postcard::from_io((&mut file, Vec::new().as_mut_slice())).unwrap();

            self.state = ChunkLoaderState::Header(header);
        }
    }

    fn ensure_chunk(&mut self) {
        match self.state {
            ChunkLoaderState::Unloaded | ChunkLoaderState::Header(_) => {
                let file = fs::File::open(&self.path.path).unwrap();
                match Chunk::try_from_io(file) {
                    Ok(chunk) => self.state = ChunkLoaderState::Chunk(chunk),
                    Err(err) => println!(
                        "failed to load chunk={file_path}: {err:?}",
                        file_path = &self
                            .path
                            .path
                            .to_str()
                            .unwrap_or("couldn't convert file path")
                    ), // do something...
                }
            }
            _ => {}
        }
    }
}

pub fn from_path(recording_path: String) -> Result<Recording, RecordingReadError> {
    let recording_path = Path::new(&recording_path);
    let path = recording_path.join("meta.rfr");
    let meta_file = fs::File::open(&path).map_err(RecordingReadError::MetaFileNotReadable)?;
    let meta =
        ChunkedMeta::try_from_io(meta_file).map_err(RecordingReadError::ReadingMetaFailed)?;

    let current = current_software_version();

    if !current.can_read_version(&meta.header.format_identifiers[0]) {
        return Err(RecordingReadError::IncompatibleVersion(
            meta.header.format_identifiers[0].clone(),
        ));
    }

    let mut chunks = Vec::new();
    for entry in WalkDir::new(recording_path).sort_by_file_name() {
        let entry = entry.map_err(RecordingReadError::FilesystemError)?;
        if !entry.file_type().is_file() {
            // Skip anything that isn't a file, we're not interested in that.
            continue;
        }

        match entry.file_name().to_str() {
            Some("meta.rfr") | Some("callsites.rfr") => {
                // We've already read the meta data, so we'll skip it (and any other metadata files).
                continue;
            }
            Some(file_name) if file_name.ends_with(".rfr") => {
                // We assume that this is a chunk
                chunks.push(ChunkPath::new(entry.clone().into_path()).into());
            }
            _ => {}
        }

        println!("dir entry: {:?}", entry.file_name());
    }

    Ok(Recording { meta, chunks })
}

#[derive(Debug)]
#[non_exhaustive]
pub enum RecordingReadError {
    Unimplemented,
    MetaFileNotReadable(io::Error),
    ReadingMetaFailed(MetaTryFromIoError),
    IncompatibleVersion(FormatIdentifier),
    FilesystemError(walkdir::Error),
}
