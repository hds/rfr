use std::{
    fs,
    io::{self, BufWriter, SeekFrom},
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{FormatIdentifier, FormatVariant};

fn current_software_version() -> FormatIdentifier {
    FormatIdentifier {
        variant: FormatVariant::RfrStreaming,
        major: 0,
        minor: 0,
        patch: 2,
    }
}

/// A timestamp measured from the [`UNIX_EPOCH`].
///
/// This timestamp is absolute as it has an external reference.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AbsTimestamp {
    /// Whole seconds component of the timestamp, measured from the [`UNIX_EPOCH`].
    pub secs: u64,
    /// Sub-second component of the timestamp, measured in microseconds.
    pub subsec_micros: u32,
}

/// A timestamp measured from the beginning of the [recording window].
///
/// This timestamp is relative to a specific window.
///
/// [recording window]: http://need-docs-on-the-recording-window.net/
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WinTimestamp {
    /// The total number of microseconds since the beginning of the window.
    pub micros: u64,
}

impl WinTimestamp {
    pub const ZERO: Self = Self { micros: 0 };

    pub fn as_micros(&self) -> u64 {
        self.micros
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Meta {
    pub timestamp: AbsTimestamp,
}

impl Meta {
    pub fn now() -> Self {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().into()
    }
}

impl From<Duration> for Meta {
    fn from(value: Duration) -> Self {
        Self {
            timestamp: AbsTimestamp {
                secs: value.as_secs(),
                subsec_micros: value.subsec_micros(),
            },
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

#[derive(Debug)]
pub struct StreamWriter<W>
where
    W: std::io::Write,
{
    inner: BufWriter<W>,
    event_count: usize,
}

impl<W> StreamWriter<W>
where
    W: std::io::Write,
{
    pub fn new(inner: W) -> Self {
        let mut buf_writer = BufWriter::new(inner);

        let version = format!("{}", current_software_version());
        postcard::to_io(&version, &mut buf_writer).unwrap();

        Self {
            inner: buf_writer,
            event_count: 0,
        }
    }

    pub fn write_record(&mut self, record: Record) {
        postcard::to_io(&record, &mut self.inner).unwrap();
        self.event_count += 1;
    }

    pub fn event_count(&self) -> usize {
        self.event_count
    }

    pub fn flush(&mut self) -> io::Result<()> {
        use std::io::Write;
        self.inner.flush()
    }
}

pub fn from_file(filename: String) -> Vec<Record> {
    let mut file = fs::File::open(filename).unwrap();

    let mut buffer_vec = vec![0_u8; 1024];
    //let buffer: &mut [u8] = &mut buffer_vec;
    let mut file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);

    let Ok(mut end_pos) = file_buffer.0.seek(SeekFrom::End(0)) else {
        println!("cannot get file length");
        return Vec::new();
    };
    let Ok(_) = file_buffer.0.seek(SeekFrom::Start(0)) else {
        println!("cannot seek back to start of file");
        return Vec::new();
    };

    let result = postcard::from_io(file_buffer).unwrap();
    let (raw_version, _): (String, _) = result;
    let Ok(version) = FormatIdentifier::from_str(&raw_version) else {
        // TODO(hds): Really need to return a `Result` from this function.
        panic!("Cannot parse format identifier from file");
    };

    let current = current_software_version();
    if !current.can_read_version(&version) {
        panic!("Software version {current} cannot read file format version {version}",);
    }

    let mut records = Vec::new();

    use std::io::Seek;
    file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
    'event: for idx in 0_usize.. {
        let result = loop {
            let Ok(file_pos) = file_buffer.0.stream_position() else {
                println!("at {idx} cannot get file position");
                break 'event;
            };

            if file_pos >= end_pos {
                let Ok(new_end_pos) = file_buffer.0.seek(SeekFrom::End(0)) else {
                    println!("at {idx} cannot get file length");
                    break 'event;
                };
                if new_end_pos <= end_pos {
                    break 'event;
                }

                end_pos = new_end_pos;
                let Ok(_) = file_buffer.0.seek(SeekFrom::Start(0)) else {
                    println!("at {idx} cannot seek back to previous file position");
                    break 'event;
                };
                // Start loop from the beginning, even if this means we need to get the stream
                // position again.
                continue;
            }

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
