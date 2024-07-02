use std::{fmt, fs, io::SeekFrom, time::{Duration, SystemTime, UNIX_EPOCH}};

use serde::{Deserialize, Serialize};

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum FormatName {
    Rfr,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FormatVersion {
    pub name: FormatName,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{name}/{major}.{minor}.{patch}",
            name = match self.name {
                FormatName::Rfr => "rfr",
            },
            major = self.major,
            minor = self.minor,
            patch = self.patch,
        )
    }
}

impl FormatVersion {
    /// Returns the format version implemented by this library.
    pub fn current_software_version() -> Self {
        Self {
            name: FormatName::Rfr,
            major: 0,
            minor: 0,
            patch: 1,
        }
    }

    pub fn can_read_version(version: &FormatVersion) -> bool {
        let current = Self::current_software_version();

        // Completely different format
        if current.name != version.name {
            return false;
        }

        // Different major version
        if current.major != version.major {
            return false;
        }

        // Pre 1.0.0
        if current.major == 0 {
            // Different minor in pre-1.0
            if current.minor != version.minor {
                return false;
            }

            // Pre 0.1.0
            if current.minor == 0 {
                // Different patch in pre-0.1.0
                if current.patch != version.patch {
                    return false;
                }
            }

            if current.patch >= version.patch {
                return true;
            }
        }

        if current.minor >= version.minor {
            return true;
        }

        false
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Meta {
    pub timestamp_s: u64,
    pub timestamp_subsec_us: u32,
}

impl Meta {
    pub fn now() -> Self {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().into()
    }
}

impl From<Duration> for Meta {
    fn from(value: Duration) -> Self {
        Self {
            timestamp_s: value.as_secs(),
            timestamp_subsec_us: value.subsec_micros(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Record {
    pub meta: Meta,
    pub event: Event,
}

impl Record {
    pub fn new(meta: Meta, event: Event) -> Self {
        Self { meta, event }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub enum Event {
    Task(Task),
    NewTask { id: TaskId },
    TaskPollStart { id: TaskId },
    TaskPollEnd { id: TaskId },
    TaskDrop { id: TaskId },
    WakerOp(WakerAction),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct TaskId(u64);

impl From<u64> for TaskId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl TaskId {
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum TaskKind {
    Task,
    Local,
    Blocking,
    BlockOn,
    Other(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Task {
    pub task_id: TaskId,
    pub task_name: String,
    pub task_kind: TaskKind,

    pub context: Option<TaskId>,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum WakerOp {
    Wake,
    WakeByRef,
    Clone,
    Drop,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WakerAction {
    pub op: WakerOp,
    pub task_id: TaskId,

    pub context: Option<TaskId>,
}

#[derive(Debug, Default)]
pub struct Writer {
    buffer: Vec<u8>,
    pub event_count: usize,
}

impl Writer {
    pub fn write_record(&mut self, record: Record) {
        postcard::to_io(&record, &mut self.buffer).unwrap();
        self.event_count += 1;
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.buffer.as_slice()
    }

    pub fn write_to_file(&mut self, mut file: &fs::File) {
        let version = FormatVersion::current_software_version();
        postcard::to_io(&version, &mut file).unwrap();
        postcard::to_io(&self.event_count, &mut file).unwrap();
        use std::io::Write;
        file.write_all(self.buffer.as_slice()).unwrap();
    }
}

pub fn from_file(filename: String) -> Vec<Record> {
    let mut file = fs::File::open(filename).unwrap();

    let mut buffer_vec = vec![0_u8; 1024];
    //let buffer: &mut [u8] = &mut buffer_vec;
    let mut file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);

    let result = postcard::from_io(file_buffer).unwrap();
    let version: FormatVersion = result.0;
    file_buffer = result.1;

    if !FormatVersion::can_read_version(&version) {
        panic!(
            "Software version {current} cannot read file format version {version}",
            current = FormatVersion::current_software_version()
        );
    }

    let result = postcard::from_io(file_buffer).unwrap();
    let count: usize = result.0;
    _ = result.1;

    let mut records = Vec::new();

    use std::io::Seek;
    file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
    'event: for idx in 0..count {
        let result = loop {
            let Ok(file_pos) = file_buffer.0.stream_position() else {
                println!("at {idx} cannot get file position");
                break 'event;
            };
            break match postcard::from_io(file_buffer) {
                Ok(result) => result,
                Err(postcard::Error::DeserializeUnexpectedEnd) => {
                    let new_size = buffer_vec.len() * 2;
                    const MAX_BUFFER_SIZE: usize = 1 << 20; // 1 MiB
                    if new_size > MAX_BUFFER_SIZE {
                        println!(
                            "excessive buffer required for element (> {MAX_BUFFER_SIZE}), skipping"
                        );
                        file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
                        continue 'event;
                    }
                    buffer_vec.resize(new_size * 2, 0);
                    if let Err(err) = file.seek(SeekFrom::Start(file_pos)) {
                        println!("Could not seek back to start of element after making buffer bigger: {err}");
                        file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
                        continue 'event;
                    }
                    file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
                    continue;
                }
                Err(err) => {
                    println!("Received error deserializing event index {idx}: {err} ({err:?})",);
                    return Vec::default();
                }
            };
        };

        records.push(result.0);
        file_buffer = (result.1 .0, &mut buffer_vec as &mut [u8]);
    }

    records
}
