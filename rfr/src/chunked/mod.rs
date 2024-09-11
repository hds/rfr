use std::{
    cell::RefCell,
    error, fmt, fs,
    io::{self, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use jiff::{tz::TimeZone, Timestamp, Zoned};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::{
    common::{Event, Task},
    identifier::ReadFormatIdentifierError,
    rec::{self, AbsTimestamp},
    FormatIdentifier, FormatVariant,
};

mod sequence;
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
    root_dir: String,
    base_time: AbsTimestampSecs,

    /// The length of time a chunk is "responsible" for. This value must either be a multiple of
    /// seconds (multiple of 1_000_000) or a divisor of a whole second (divisor of 1_000_000).
    chunk_period_micros: u32,

    chunk_buffers: Mutex<Vec<ChunkBuffer>>,
}

impl ChunkedWriter {
    pub fn new(root_dir: String) -> Self {
        let timestamp = rec::AbsTimestamp::now();
        let base_time = AbsTimestampSecs::from(timestamp.clone());
        let header = MetaHeader {
            created_time: timestamp,
            base_time,
        };
        fs::create_dir_all(&root_dir).unwrap();
        Self::write_meta(&root_dir, &header);

        // By default, chunks contain 1 second of execution time.
        let chunk_period_micros = 1_000_000;
        let writer = Self {
            root_dir,
            base_time,
            chunk_period_micros,
            chunk_buffers: Mutex::new(Vec::new()),
        };

        let base_time = writer.base_time;
        writer.ensure_dir(&base_time);

        writer
    }

    fn write_meta(base_dir: &String, header: &MetaHeader) -> bool {
        let path = Path::new(base_dir).join("meta.rfr");
        {
            let mut file = fs::File::create(path).unwrap();

            let version = format!("{}", current_software_version());
            postcard::to_io(&version, &mut file).unwrap();

            postcard::to_io(header, &mut file).unwrap();
        }

        true
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

    pub fn write(&self) {
        let chunk_buffers = self.chunk_buffers.lock().expect("poisoned");

        for chunk_buffer in chunk_buffers.iter() {
            let writer = self.writer_for_chunk(chunk_buffer);
            chunk_buffer.write(writer);
        }
    }

    fn writer_for_chunk(&self, chunk: &ChunkBuffer) -> impl io::Write {
        fs::File::create(self.chunk_path(&chunk.header.interval.base_time)).unwrap()
    }
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

        let version = format!("{}", current_software_version());
        postcard::to_io(&version, &mut writer).unwrap();

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

/// Header for the metadata file which is stored at `<chunked-recording.rfr>/meta.rfr`
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetaHeader {
    pub created_time: rec::AbsTimestamp,
    pub base_time: AbsTimestampSecs,
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
// A chunk timestamp represents the time of an event with respect to the chunk's base time. It is
// stored as the number of microseconds since the base time. All events within a chunk must occur
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

    pub fn from_base_and_timestamp(base_time: AbsTimestampSecs, timestamp: &AbsTimestamp) -> Self {
        let secs = timestamp.secs.saturating_sub(base_time.secs);
        let micros = (secs * 1_000_000) + timestamp.subsec_micros as u64;
        Self::new(micros)
    }
}

/// Metadata for an [`EventRecord`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Meta {
    /// The timestamp that the event occurs at.
    pub timestamp: ChunkTimestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EventRecord {
    pub meta: Meta,
    pub event: Event,
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Object {
    Task(Task),
}

#[derive(Debug)]
pub struct Recording {
    identifier: FormatIdentifier,
    meta: MetaHeader,
    chunks: Vec<ChunkLoader>,
}

impl Recording {
    pub fn load_all_chunks(&mut self) {
        for chunk_loader in &mut self.chunks {
            chunk_loader.ensure_chunk();
        }
    }

    pub fn identifier(&self) -> &FormatIdentifier {
        &self.identifier
    }

    pub fn meta(&self) -> &MetaHeader {
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

        // TODO(hds): Should we validate the identifier?
        let _identifier = FormatIdentifier::try_from_io(&mut reader).unwrap();

        let (header, _): (ChunkHeader, _) =
            postcard::from_io((&mut reader, Vec::new().as_mut_slice())).unwrap();

        let (seq_chunk_len, _): (usize, _) =
            postcard::from_io((&mut reader, Vec::new().as_mut_slice())).unwrap();
        let mut seq_chunks: Vec<SeqChunk> = Vec::with_capacity(seq_chunk_len);

        let mut buffer = vec![0_u8; 1024];
        let mut file_buffer = (&mut reader, buffer.as_mut_slice());

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
            let _identifier = FormatIdentifier::try_from_io(&mut file).unwrap();

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
                    Err(err) => println!("failed to load chunk: {err}"), // do something...
                }
            }
            _ => {}
        }
    }
}

fn read_meta(path: &PathBuf) -> Result<(FormatIdentifier, MetaHeader), MetaReadError> {
    let mut meta_file = fs::File::open(path).map_err(MetaReadError::OpenFileFailed)?;
    let format_identifier = FormatIdentifier::try_from_io(&mut meta_file)
        .map_err(MetaReadError::FormatIdenifierInvalid)?;

    let (header, _) = postcard::from_io((&mut meta_file, Vec::new().as_mut_slice()))
        .map_err(MetaReadError::HeaderInvalid)?;

    Ok((format_identifier, header))
}

#[derive(Debug)]
#[non_exhaustive]
pub enum MetaReadError {
    OpenFileFailed(io::Error),
    FormatIdenifierInvalid(ReadFormatIdentifierError),
    HeaderInvalid(postcard::Error),
}

pub fn from_path(recording_path: String) -> Result<Recording, RecordingReadError> {
    let recording_path = Path::new(&recording_path);
    let path = recording_path.join("meta.rfr");
    let (identifier, meta) = read_meta(&path).map_err(RecordingReadError::ReadingMetaFailed)?;

    let current = current_software_version();
    if !current.can_read_version(&identifier) {
        return Err(RecordingReadError::IncompatibleVersion(identifier));
    }

    let mut chunks = Vec::new();
    for entry in WalkDir::new(recording_path).sort_by_file_name() {
        let entry = entry.map_err(RecordingReadError::FilesystemError)?;
        if !entry.file_type().is_file() {
            // Skip anything that isn't a file, we're not interested in that.
            continue;
        }

        match entry.file_name().to_str() {
            Some("meta.rfr") => {
                // We've already read the meta data, so we'll skip it (and any other "meta.rfr" files).
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

    Ok(Recording {
        identifier,
        meta,
        chunks,
    })
}

#[derive(Debug)]
#[non_exhaustive]
pub enum RecordingReadError {
    Unimplemented,
    ReadingMetaFailed(MetaReadError),
    IncompatibleVersion(FormatIdentifier),
    FilesystemError(walkdir::Error),
}