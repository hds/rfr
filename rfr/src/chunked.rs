use std::{
    collections::{HashMap, HashSet},
    error, fmt, fs,
    io::{self, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use jiff::{tz::TimeZone, Timestamp, Zoned};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::{
    common::{Event, Task, TaskId},
    identifier::ReadFormatIdentifierError,
    rec::{self, AbsTimestamp},
    FormatIdentifier, FormatVariant,
};

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

    chunks: Mutex<Vec<Arc<ThreadChunkBuffer>>>,
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

        let writer = Self {
            root_dir,
            base_time,
            chunks: Mutex::new(Vec::new()),
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

    pub fn register_chunk(&self, chunk: Arc<ThreadChunkBuffer>) {
        let mut chunks = self.chunks.lock().expect("poisoned");

        chunks.push(chunk);
    }

    pub fn write(&self) {
        let mut chunk_buffers: HashMap<AbsTimestampSecs, ChunkBuffer> = HashMap::new();
        let mut thread_chunks = self.chunks.lock().expect("poisoned");

        for thread_chunk in &*thread_chunks {
            chunk_buffers
                .entry(thread_chunk.base_time)
                .and_modify(|chunk| chunk.push_thread_chunk(Arc::clone(thread_chunk)))
                .or_insert_with(|| ChunkBuffer::with_first_thread_chunk(Arc::clone(thread_chunk)));
        }

        for chunk_buffer in chunk_buffers.into_values() {
            let writer = self.writer_for_chunk(&chunk_buffer);
            chunk_buffer.write(writer);
        }

        thread_chunks.clear();
    }

    fn writer_for_chunk(&self, chunk: &ChunkBuffer) -> impl io::Write {
        fs::File::create(self.chunk_path(&chunk.base_time)).unwrap()
    }
}

#[derive(Debug)]
pub struct ChunkBuffer {
    base_time: AbsTimestampSecs,
    start_time: AbsTimestamp,
    end_time: AbsTimestamp,

    thread_chunks: Vec<Arc<ThreadChunkBuffer>>,
}

impl ChunkBuffer {
    fn with_first_thread_chunk(thread_chunk: Arc<ThreadChunkBuffer>) -> Self {
        Self {
            base_time: thread_chunk.base_time,
            start_time: thread_chunk.start_time.clone(),
            end_time: thread_chunk.end_time.clone(),
            thread_chunks: vec![thread_chunk],
        }
    }

    fn push_thread_chunk(&mut self, thread_chunk: Arc<ThreadChunkBuffer>) {
        assert_eq!(self.base_time, thread_chunk.base_time);

        self.start_time = self.start_time.clone().min(thread_chunk.start_time.clone());
        self.end_time = self.end_time.clone().max(thread_chunk.end_time.clone());

        self.thread_chunks.push(thread_chunk);
    }

    fn write(&self, writer: impl io::Write) {
        let mut writer = writer;

        let version = format!("{}", current_software_version());
        postcard::to_io(&version, &mut writer).unwrap();

        postcard::to_io(&self.base_time, &mut writer).unwrap();
        postcard::to_io(&self.start_time, &mut writer).unwrap();
        postcard::to_io(&self.end_time, &mut writer).unwrap();

        postcard::to_io(&self.thread_chunks.len(), &mut writer).unwrap();
        for thread_chunk in &self.thread_chunks {
            thread_chunk.write(&mut writer);
        }
    }
}

#[derive(Debug)]
pub struct ThreadChunkBuffer {
    base_time: AbsTimestampSecs,
    start_time: AbsTimestamp,
    end_time: AbsTimestamp,
    buffer: Mutex<Buffer>,
}

#[derive(Debug, Default)]
struct Buffer {
    objects: HashMap<TaskId, Vec<u8>>,
    missing_objects: HashSet<TaskId>,
    event_count: usize,
    events: Vec<u8>,
}

impl ThreadChunkBuffer {
    pub fn new(now: AbsTimestamp) -> Self {
        Self {
            base_time: now.clone().into(),
            start_time: now.clone(),
            end_time: now,
            buffer: Default::default(),
        }
    }

    pub fn base_time(&self) -> AbsTimestampSecs {
        self.base_time
    }

    pub fn chunk_timestamp(&self, timestamp: rec::AbsTimestamp) -> ChunkTimestamp {
        let secs = timestamp.secs.saturating_sub(self.base_time.secs);
        let micros = (secs * 1_000_000) + timestamp.subsec_micros as u64;
        ChunkTimestamp::new(micros)
    }

    pub fn append_record<F>(&self, record: EventRecord, get_objects: F)
    where
        F: FnOnce(&[TaskId]) -> Vec<Option<Object>>,
    {
        let mut buffer = self.buffer.lock().expect("poisoned");
        let mut missing_task_ids = Vec::new();
        match &record.event {
            Event::NewTask { id }
            | Event::TaskPollStart { id }
            | Event::TaskPollEnd { id }
            | Event::TaskDrop { id } => {
                if !buffer.objects.contains_key(id) {
                    missing_task_ids.push(*id);
                }
            }
            Event::WakerWake { waker }
            | Event::WakerWakeByRef { waker }
            | Event::WakerClone { waker }
            | Event::WakerDrop { waker } => {
                if !buffer.objects.contains_key(&waker.task_id) {
                    missing_task_ids.push(waker.task_id);
                }
                if let Some(context_task_id) = &waker.context {
                    if context_task_id != &waker.task_id
                        && !buffer.objects.contains_key(context_task_id)
                    {
                        missing_task_ids.push(*context_task_id);
                    }
                }
            }
        }

        let missing_tasks = get_objects(missing_task_ids.as_slice());
        for (task_id, task) in missing_task_ids.into_iter().zip(missing_tasks.into_iter()) {
            match task {
                Some(task) => {
                    let task_buffer = postcard::to_stdvec(&task).unwrap();
                    buffer.objects.insert(task_id, task_buffer);
                }
                None => {
                    // TODO(hds): Currently we don't do anything with this information, should we?
                    //            Also, should we actually return early here or should we continue?
                    //            If we do want to return early, we should probably not write any
                    //            task data to `buffer.objects`.
                    buffer.missing_objects.insert(task_id);
                    return;
                }
            }
        }

        postcard::to_io(&record, &mut buffer.events).unwrap();
        buffer.event_count += 1;
    }

    fn write(&self, writer: impl io::Write) {
        let mut writer = writer;
        let buffer = self.buffer.lock().expect("poisoned");

        postcard::to_io(&self.start_time, &mut writer).unwrap();
        postcard::to_io(&self.end_time, &mut writer).unwrap();

        postcard::to_io(&buffer.objects.len(), &mut writer).unwrap();
        for object_data in buffer.objects.values() {
            writer.write_all(object_data.as_slice()).unwrap();
        }

        postcard::to_io(&buffer.event_count, &mut writer).unwrap();
        writer.write_all(buffer.events.as_slice()).unwrap();
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
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChunkTimestamp {
    /// Microseconds since the chunk's base time
    pub micros: u64,
}

impl ChunkTimestamp {
    pub fn new(micros: u64) -> Self {
        Self { micros }
    }
}

/// Metadata for an [`EventRecord`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Meta {
    /// The timestamp that the event occurs at.
    pub timestamp: ChunkTimestamp,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventRecord {
    pub meta: Meta,
    pub event: Event,
}

#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, Serialize)]
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
    thread_chunks: Vec<ThreadChunk>,
}

impl Chunk {
    pub fn header(&self) -> &ChunkHeader {
        &self.header
    }

    pub fn thread_chunks(&self) -> &Vec<ThreadChunk> {
        &self.thread_chunks
    }

    pub fn abs_timestamp(&self, chunk_timestamp: &ChunkTimestamp) -> AbsTimestamp {
        let chunk_timestamp_secs = chunk_timestamp.micros / 1_000_000;
        let chunk_timestamp_subsec_micros = (chunk_timestamp.micros % 1_000_000) as u32;

        AbsTimestamp {
            secs: self.header.base_time.secs + chunk_timestamp_secs,
            subsec_micros: chunk_timestamp_subsec_micros,
        }
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

        let (thread_chunk_len, _): (usize, _) =
            postcard::from_io((&mut reader, Vec::new().as_mut_slice())).unwrap();
        let mut thread_chunks: Vec<ThreadChunk> = Vec::with_capacity(thread_chunk_len);

        let mut buffer = vec![0_u8; 1024];
        let mut file_buffer = (&mut reader, buffer.as_mut_slice());

        'thread_chunk: for idx in 0..thread_chunk_len {
            let thread_chunk_result = loop {
                let Ok(file_pos) = file_buffer.0.stream_position() else {
                    println!("at {idx} cannot get file position");
                    break 'thread_chunk;
                };

                if file_pos >= end_pos {
                    let Ok(new_end_pos) = file_buffer.0.seek(SeekFrom::End(0)) else {
                        println!("at {idx} cannot get file length");
                        break 'thread_chunk;
                    };
                    if new_end_pos <= end_pos {
                        break 'thread_chunk;
                    }

                    end_pos = new_end_pos;
                    let Ok(_) = file_buffer.0.seek(SeekFrom::Start(0)) else {
                        println!("at {idx} cannot seek back to previous file position");
                        break 'thread_chunk;
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
                            continue 'thread_chunk;
                        }
                        buffer.resize(new_size * 2, 0);
                        if let Err(err) = reader.seek(SeekFrom::Start(file_pos)) {
                            println!("Could not seek back to start of element after making buffer bigger: {err}");
                            file_buffer = (&mut reader, buffer.as_mut_slice());
                            continue 'thread_chunk;
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

            thread_chunks.push(thread_chunk_result.0);
            file_buffer = (thread_chunk_result.1 .0, buffer.as_mut_slice());
        }

        Ok(Self {
            header,
            thread_chunks,
        })
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
    base_time: AbsTimestampSecs,
    start_time: AbsTimestamp,
    end_time: AbsTimestamp,
}

impl ChunkHeader {
    pub fn base_time(&self) -> &AbsTimestampSecs {
        &self.base_time
    }

    pub fn start_time(&self) -> &AbsTimestamp {
        &self.start_time
    }

    pub fn end_time(&self) -> &AbsTimestamp {
        &self.end_time
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThreadChunk {
    start_time: AbsTimestamp,
    end_time: AbsTimestamp,
    objects: Vec<Object>,
    events: Vec<EventRecord>,
}

impl ThreadChunk {
    pub fn objects(&self) -> &Vec<Object> {
        &self.objects
    }

    pub fn events(&self) -> &Vec<EventRecord> {
        &self.events
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
    for entry in WalkDir::new(recording_path) {
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
