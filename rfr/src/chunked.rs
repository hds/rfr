use std::{collections::HashMap, fs, io, path::Path};

use jiff::{tz::TimeZone, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{
    rec::{self, AbsTimestamp, Task, TaskId},
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
}

impl ChunkedWriter {
    pub fn new(root_dir: String) -> Self {
        let timestamp = rec::AbsTimestamp::now();
        let base_time = AbsTimestampSecs::from(timestamp.clone());
        let header = MetaHeader {
            created_time: timestamp,
            base_time: base_time.clone(),
        };
        fs::create_dir_all(&root_dir).unwrap();
        Self::write_meta(&root_dir, &header);

        let mut writer = Self {
            root_dir,
            base_time,
        };

        let base_time = writer.base_time.clone();
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

    fn ensure_dir(&mut self, time: &AbsTimestampSecs) {
        let ts = Timestamp::from_second(time.secs as i64).unwrap();
        let ts_utc = ts.to_zoned(TimeZone::UTC);

        let path = Path::new(&self.root_dir)
            .join(format!("{}", ts_utc.strftime("%Y-%m")))
            .join(format!("{}", ts_utc.strftime("%d-%H")));
        fs::create_dir_all(path).unwrap();
    }
}

pub struct ThreadChunkBuffer {
    base_time: AbsTimestampSecs,
    start_time: AbsTimestamp,
    end_time: AbsTimestamp,
    objects: HashMap<TaskId, Vec<u8>>,
    event_count: usize,
    events: Vec<u8>,
}

impl ThreadChunkBuffer {
    pub fn new(now: AbsTimestamp) -> Self {
        Self {
            base_time: now.clone().into(),
            start_time: now.clone(),
            end_time: now,
            objects: HashMap::new(),
            event_count: 0,
            // TODO(hds): reserve some capacity?
            events: Vec::new(),
        }
    }

    pub fn base_time(&self) -> AbsTimestampSecs {
        self.base_time
    }

    pub fn append_record<F>(&mut self, record: EventRecord, get_objects: F)
    where
        F: FnOnce(&[TaskId]) -> Option<Vec<Object>>,
    {
        let mut missing_task_ids = Vec::new();
        match &record.event {
            Event::NewTask { id } | Event::TaskPollStart { id } | Event::TaskPollEnd { id } | Event::TaskDrop { id } => {
                if !self.objects.contains_key(id) {
                    missing_task_ids.push(*id);
                }
            }
            Event::WakerWake { waker } | Event::WakerWakeByRef { waker } | Event::WakerClone { waker } | Event::WakerDrop { waker } => {
                if !self.objects.contains_key(&waker.task_id) {
                    missing_task_ids.push(waker.task_id);
                }
                if let Some(context_task_id) = &waker.context {
                    if context_task_id != &waker.task_id && !self.objects.contains_key(context_task_id) {
                        missing_task_ids.push(*context_task_id);
                    }
                }
            }
        }

        let Some(missing_tasks) = get_objects(missing_task_ids.as_slice()) else {
            // If we don't have all the tasks, we can't write this record.
            // TODO(hds): write error
            return;
        };


        for (task_id, task,) in missing_task_ids.into_iter().zip(missing_tasks.into_iter()) {
            let buffer = postcard::to_stdvec(&task).unwrap();
            self.objects.insert(task_id, buffer);
        }

        postcard::to_io(&record, &mut self.events).unwrap();
        self.event_count += 1;
    }

    fn write(&mut self, writer: impl io::Write) {
        let mut writer = writer;

        postcard::to_io(&self.base_time, &mut writer).unwrap();
        postcard::to_io(&self.start_time, &mut writer).unwrap();
        postcard::to_io(&self.end_time, &mut writer).unwrap();
        
        postcard::to_io(&self.objects.len(), &mut writer).unwrap();
        for object_data in self.objects.values() {
            writer.write_all(object_data.as_slice()).unwrap();
        }

        postcard::to_io(&self.event_count, &mut writer).unwrap();
        writer.write_all(self.events.as_slice()).unwrap();

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
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
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
}

// A timestamp within a chunk.
//
// A chunk timestamp represents the time of an event with respect to the chunk's base time. It is
// stored as the number of microseconds since the base time. All events within a chunk must occur
// at the base time or afterwards.
#[derive(Debug, Deserialize, Serialize)]
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
#[derive(Debug, Deserialize, Serialize)]
pub struct Meta {
    /// The timestamp that the event occurs at.
    pub timestamp: ChunkTimestamp,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EventRecord {
    pub meta: Meta,
    pub event: Event,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum Event {
    NewTask { id: TaskId },
    TaskPollStart { id: TaskId },
    TaskPollEnd { id: TaskId },
    TaskDrop { id: TaskId },
    WakerWake { waker: Waker },
    WakerWakeByRef { waker: Waker },
    WakerClone { waker: Waker },
    WakerDrop { waker: Waker },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Waker {
    pub task_id: TaskId,
    pub context: Option<TaskId>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Object {
    Task(Task),
}
