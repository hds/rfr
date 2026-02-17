use std::{
    error, fmt, fs,
    io::{self, SeekFrom},
    path::{Path, PathBuf},
};

use walkdir::WalkDir;

use crate::{
    FormatIdentifier,
    chunked::{
        Chunk, ChunkHeader, ChunkedMeta, MetaTryFromIoError, SeqChunk, current_software_version,
    },
};

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

#[derive(Clone, Debug)]
enum ChunkReadError {
    ReadError,
    DeserializeError {
        #[expect(unused)]
        index: usize,
        #[expect(unused)]
        error: postcard::Error,
    },
}

impl fmt::Display for ChunkReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl error::Error for ChunkReadError {}

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
                match read_chunk_from_io(file) {
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

fn read_chunk_from_io<IO>(reader: IO) -> Result<Chunk, ChunkReadError>
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
                        println!(
                            "Could not seek back to start of element after making buffer bigger: {err}"
                        );
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
        file_buffer = (seq_chunk_result.1.0, buffer.as_mut_slice());
    }

    Ok(Chunk { header, seq_chunks })
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
