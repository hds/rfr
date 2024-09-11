//! Sequences
//!
//! Each chunk in a chunked recording is made up of one or more sequence chunks ([`SeqChunk`]). Each sequence
//! chunk contains an in-order series of events and the objects referenced by those events.
//!
//! Sequence chunks are generally used to model events from a single thread (as they can be
//! recorded in order). Sequences can be tracked across multiple chunks by the sequence identifier
//! [`SeqId`].

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

use serde::{Deserialize, Serialize};

use crate::{
    chunked::{AbsTimestampSecs, ChunkInterval, ChunkTimestamp, EventRecord, Object},
    common::{Event, TaskId},
    rec::AbsTimestamp,
};

/// Sequence chunk
///
/// A chunk is made up of multiple sequence chunks. All the events in a sequence chunk are in
/// order, whereas no such guarantee is made regarding the events from different sequences. A
/// single sequence chunk contains all the events in a sequence which fall within the time range of
/// the parent chunk.
///
/// Sequence chunks can be linked by their sequence identifier ([`SeqId`]).
///
/// A sequence generally models a single thread and the events emitted from within it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SeqChunk {
    pub header: SeqChunkHeader,
    pub objects: Vec<Object>,
    pub events: Vec<EventRecord>,
}

/// Sequence chunk header
///
/// The header data for a sequence chunk.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SeqChunkHeader {
    pub seq_id: SeqId,
    pub earliest_timestamp: ChunkTimestamp,
    pub latest_timestamp: ChunkTimestamp,
}

/// Sequence identifier
///
/// The sequence identifier links together multiple sequence chunks with different parent chunks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct SeqId(u64);

impl From<u64> for SeqId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl SeqId {
    const INVALID: SeqId = Self(0);

    fn current() -> Self {
        static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);
        thread_local! {
            pub static THREAD_ID: Cell<SeqId> = const { Cell::new(SeqId::INVALID) };
        }

        let current = THREAD_ID.get();
        if current == Self::INVALID {
            let new_current = Self(NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst));
            THREAD_ID.set(new_current);
            new_current
        } else {
            current
        }
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub struct SeqChunkBuffer {
    interval: ChunkInterval,
    buffer: Mutex<Buffer>,
}

#[derive(Debug)]
struct Buffer {
    header: SeqChunkHeader,
    objects: HashMap<TaskId, Vec<u8>>,
    missing_objects: HashSet<TaskId>,
    event_count: usize,
    events: Vec<u8>,
}

impl SeqChunkBuffer {
    pub fn new(interval: ChunkInterval) -> Self {
        let buffer = Mutex::new(Buffer {
            header: SeqChunkHeader {
                seq_id: SeqId::current(),
                earliest_timestamp: interval.end_time,
                latest_timestamp: interval.start_time,
            },
            objects: HashMap::new(),
            missing_objects: HashSet::new(),
            event_count: 0,
            events: Vec::new(),
        });
        Self { interval, buffer }
    }

    pub fn interval(&self) -> &ChunkInterval {
        &self.interval
    }

    pub fn base_time(&self) -> AbsTimestampSecs {
        self.interval.base_time
    }

    pub fn seq_id(&self) -> SeqId {
        let buffer = self.buffer.lock().expect("poisoned");
        buffer.header.seq_id
    }

    pub fn earliest_timestamp(&self) -> ChunkTimestamp {
        let buffer = self.buffer.lock().expect("poisoned");
        buffer.header.earliest_timestamp
    }

    pub fn latest_timestamp(&self) -> ChunkTimestamp {
        let buffer = self.buffer.lock().expect("poisoned");
        buffer.header.latest_timestamp
    }

    pub fn event_count(&self) -> usize {
        let buffer = self.buffer.lock().expect("poisoned");
        buffer.event_count
    }

    /// Converts an absolute timestamp into a chunk timestamp, using the base time of the parent
    /// chunk of this sequence chunk.
    pub fn chunk_timestamp(&self, timestamp: &AbsTimestamp) -> ChunkTimestamp {
        ChunkTimestamp::from_base_and_timestamp(self.base_time(), timestamp)
    }

    // FIXME(hds): modify to take an absolute timestamp and an event instead of an EventRecord.
    // Then this function will convert the timestamp to a chunked timestamp and validate it at the
    // same time. If it is invalid, an error will be returned.
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

        // FIXME(hds): What if the 2 vecs are different sizes?
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

        if buffer.event_count == 0 {
            buffer.header.earliest_timestamp = record.meta.timestamp;
        }
        buffer.header.latest_timestamp = record.meta.timestamp;
        postcard::to_io(&record, &mut buffer.events).unwrap();
        buffer.event_count += 1;
    }

    pub fn write(&self, writer: impl io::Write) {
        let mut writer = writer;
        let buffer = self.buffer.lock().expect("poisoned");

        postcard::to_io(&buffer.header, &mut writer).unwrap();

        postcard::to_io(&buffer.objects.len(), &mut writer).unwrap();
        for object_data in buffer.objects.values() {
            writer.write_all(object_data.as_slice()).unwrap();
        }

        postcard::to_io(&buffer.event_count, &mut writer).unwrap();
        writer.write_all(buffer.events.as_slice()).unwrap();
    }
}