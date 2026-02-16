use std::{
    cmp::Ordering,
    fs,
    io::{self, BufWriter, SeekFrom},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    FormatIdentifier, FormatVariant,
    chunked::Callsite,
    common::{Event, InstrumentationId, Span, Task, Waker},
};

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
#[derive(Debug, Clone, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct AbsTimestamp {
    /// Whole seconds component of the timestamp, measured from the [`UNIX_EPOCH`].
    pub secs: u64,
    /// Sub-second component of the timestamp, measured in microseconds.
    pub subsec_micros: u32,
}

impl Ord for AbsTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.secs.cmp(&other.secs) {
            Ordering::Equal => self.subsec_micros.cmp(&other.subsec_micros),
            other => other,
        }
    }
}

impl PartialOrd for AbsTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<Duration> for AbsTimestamp {
    fn from(value: Duration) -> Self {
        Self {
            secs: value.as_secs(),
            subsec_micros: value.subsec_micros(),
        }
    }
}

impl AbsTimestamp {
    /// Earliest measurable time
    pub const EARLIEST: Self = Self {
        secs: 0,
        subsec_micros: 1,
    };

    /// Get an absolute timestamp representing the current time.
    pub fn now() -> Self {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().into()
    }

    /// Return the [`Duration`] since the UNIX epoch represented by this absolute timestamp.
    pub fn as_duration_since_epoch(&self) -> Duration {
        Duration::new(self.secs, self.subsec_micros * 1_000)
    }
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

    pub fn as_nanos(&self) -> u64 {
        self.micros * 1_000
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Meta {
    pub timestamp: AbsTimestamp,
}

impl Meta {
    pub fn now() -> Self {
        Self {
            timestamp: AbsTimestamp::now(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Record {
    pub meta: Meta,
    pub data: RecordData,
}

impl Record {
    pub fn new(meta: Meta, data: RecordData) -> Self {
        Self { meta, data }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub enum RecordData {
    End,
    Callsite { callsite: Callsite },
    Span { span: Span },
    Event { event: Event },
    Task { task: Task },
    SpanNew { iid: InstrumentationId },
    SpanEnter { iid: InstrumentationId },
    SpanExit { iid: InstrumentationId },
    SpanClose { iid: InstrumentationId },
    TaskNew { iid: InstrumentationId },
    TaskPollStart { iid: InstrumentationId },
    TaskPollEnd { iid: InstrumentationId },
    TaskDrop { iid: InstrumentationId },
    WakerWake { waker: Waker },
    WakerWakeByRef { waker: Waker },
    WakerClone { waker: Waker },
    WakerDrop { waker: Waker },
}

#[derive(Debug)]
pub struct StreamWriter<W>
where
    W: std::io::Write,
{
    inner: BufWriter<W>,
    record_count: usize,
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
            record_count: 0,
        }
    }

    pub fn write_record(&mut self, record: Record) {
        postcard::to_io(&record, &mut self.inner).unwrap();
        self.record_count += 1;
    }

    pub fn record_count(&self) -> usize {
        self.record_count
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

    let (version, _): (FormatIdentifier, _) = postcard::from_io(file_buffer).unwrap();
    let current = current_software_version();
    if !current.can_read_version(&version) {
        panic!("Software version {current} cannot read file format version {version}",);
    }

    let mut records = Vec::new();

    use std::io::Seek;
    file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
    'record: for idx in 0_usize.. {
        let result = loop {
            let Ok(file_pos) = file_buffer.0.stream_position() else {
                println!("at {idx} cannot get file position");
                break 'record;
            };

            if file_pos >= end_pos {
                let Ok(new_end_pos) = file_buffer.0.seek(SeekFrom::End(0)) else {
                    println!("at {idx} cannot get file length");
                    break 'record;
                };
                if new_end_pos <= end_pos {
                    break 'record;
                }

                end_pos = new_end_pos;
                let Ok(_) = file_buffer.0.seek(SeekFrom::Start(0)) else {
                    println!("at {idx} cannot seek back to previous file position");
                    break 'record;
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
                        continue 'record;
                    }
                    buffer_vec.resize(new_size * 2, 0);
                    if let Err(err) = file.seek(SeekFrom::Start(file_pos)) {
                        println!(
                            "Could not seek back to start of element after making buffer bigger: {err}"
                        );
                        file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
                        continue 'record;
                    }
                    file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
                    continue;
                }
                Err(err) => {
                    println!("Received error deserializing record index {idx}: {err} ({err:?})",);
                    return Vec::default();
                }
            };
        };

        records.push(result.0);
        file_buffer = (result.1.0, &mut buffer_vec as &mut [u8]);
    }

    records
}
