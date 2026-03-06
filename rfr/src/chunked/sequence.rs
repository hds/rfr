//! Sequences
//!
//! Each chunk in a chunked recording is made up of one or more sequence chunks ([`SeqChunk`]). Each sequence
//! chunk contains an in-order series of records and the objects referenced by those records.
//!
//! Sequence chunks are generally used to model records from a single thread (as they can be
//! recorded in order). Sequences can be tracked across multiple chunks by the sequence identifier
//! [`SeqId`].

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    error, fmt, io,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};

use crate::{
    AbsTimestamp, InstrumentationId,
    chunked::{AbsTimestampSecs, ChunkInterval, ChunkTimestamp, Object, Record, RecordData},
};

/// Sequence chunk
///
/// A chunk is made up of multiple sequence chunks. All the records in a sequence chunk are in
/// order, whereas no such guarantee is made regarding the records from different sequences. A
/// single sequence chunk contains all the records in a sequence which fall within the time range of
/// the parent chunk.
///
/// Sequence chunks can be linked by their sequence identifier ([`SeqId`]).
///
/// A sequence generally models a single thread and the records emitted from within it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SeqChunk {
    pub header: SeqChunkHeader,
    pub objects: Vec<Object>,
    pub records: Vec<Record>,
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
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
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
    objects: HashMap<InstrumentationId, Vec<u8>>,
    missing_objects: HashSet<InstrumentationId>,
    record_count: usize,
    records: Vec<u8>,
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
            record_count: 0,
            records: Vec::new(),
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

    pub fn record_count(&self) -> usize {
        let buffer = self.buffer.lock().expect("poisoned");
        buffer.record_count
    }

    /// Converts an absolute timestamp into a chunk timestamp, using the base time of the parent
    /// chunk of this sequence chunk.
    pub fn chunk_timestamp(&self, timestamp: &AbsTimestamp) -> ChunkTimestamp {
        ChunkTimestamp::from_base_and_timestamp(self.base_time(), timestamp)
    }

    // FIXME(hds): modify to take an absolute timestamp and a record instead of a Record. Then this
    // function will convert the timestamp to a chunked timestamp and validate it at the same time.
    // If it is invalid, an error will be returned.
    pub fn append_record<FnGetObjects>(
        &self,
        record: Record,
        get_objects: FnGetObjects,
    ) -> Result<(), AppendRecordError>
    where
        FnGetObjects: FnOnce(&[InstrumentationId]) -> Vec<Option<Object>>,
    {
        let mut buffer = self.buffer.lock().expect("poisoned");
        let mut missing_object_ids = Vec::new();
        match &record.data {
            RecordData::TaskNew { iid }
            | RecordData::TaskPollStart { iid }
            | RecordData::TaskPollEnd { iid }
            | RecordData::TaskDrop { iid } => {
                if !buffer.objects.contains_key(iid) {
                    missing_object_ids.push(*iid);
                }
            }
            RecordData::WakerWake { waker }
            | RecordData::WakerWakeByRef { waker }
            | RecordData::WakerClone { waker }
            | RecordData::WakerDrop { waker } => {
                if !buffer.objects.contains_key(&waker.task_iid) {
                    missing_object_ids.push(waker.task_iid);
                }
                if let Some(context_task_id) = &waker.context
                    && context_task_id != &waker.task_iid
                    && !buffer.objects.contains_key(context_task_id)
                {
                    missing_object_ids.push(*context_task_id);
                }
            }
            RecordData::SpanNew { iid }
            | RecordData::SpanEnter { iid }
            | RecordData::SpanExit { iid }
            | RecordData::SpanClose { iid } => {
                // TODO(hds): Do something with spans
                _ = iid;
            }
            RecordData::Event { event } => {
                // TODO(hds): Do something with events
                _ = event;
            }
        }

        // FIXME(hds): What if the 2 vecs are different sizes?
        let missing_objects = get_objects(missing_object_ids.as_slice());
        for (iid, object) in missing_object_ids
            .into_iter()
            .zip(missing_objects.into_iter())
        {
            match object {
                Some(object) => {
                    let object_buffer =
                        postcard::to_stdvec(&object).map_err(AppendRecordError::write_object)?;
                    buffer.objects.insert(iid, object_buffer);
                }
                None => {
                    // TODO(hds): Currently we don't do anything with this information, should we?
                    //            Also, should we actually return early here or should we continue?
                    //            If we do want to return early, we should probably not write any
                    //            object data to `buffer.objects`.
                    buffer.missing_objects.insert(iid);
                    return Err(AppendRecordError::missing_object(iid));
                }
            }
        }

        if buffer.record_count == 0 {
            buffer.header.earliest_timestamp = record.meta.timestamp;
        }
        buffer.header.latest_timestamp = record.meta.timestamp;
        postcard::to_io(&record, &mut buffer.records).map_err(AppendRecordError::write_record)?;
        buffer.record_count += 1;

        Ok(())
    }

    pub fn write(&self, writer: impl io::Write) -> Result<(), SeqChunkWriteError> {
        let mut writer = writer;
        let buffer = self
            .buffer
            .lock()
            .map_err(|_| SeqChunkWriteError::buffer_lock_poisoned())?;

        postcard::to_io(&buffer.header, &mut writer).map_err(SeqChunkWriteError::header)?;

        postcard::to_io(&buffer.objects.len(), &mut writer)
            .map_err(SeqChunkWriteError::objects_length)?;
        for object_data in buffer.objects.values() {
            writer
                .write_all(object_data.as_slice())
                .map_err(SeqChunkWriteError::objects)?;
        }

        postcard::to_io(&buffer.record_count, &mut writer)
            .map_err(SeqChunkWriteError::records_length)?;
        writer
            .write_all(buffer.records.as_slice())
            .map_err(SeqChunkWriteError::records)?;

        Ok(())
    }
}

/// Error appending record to sequence chunk buffer
#[derive(Debug)]
pub struct AppendRecordError {
    kind: AppendRecordErrorKind,
}

impl AppendRecordError {
    fn missing_object(iid: InstrumentationId) -> Self {
        Self {
            kind: AppendRecordErrorKind::MissingObject { iid },
        }
    }

    fn write_object(inner: postcard::Error) -> Self {
        Self {
            kind: AppendRecordErrorKind::WriteObject { inner },
        }
    }

    fn write_record(inner: postcard::Error) -> Self {
        Self {
            kind: AppendRecordErrorKind::WriteRecord { inner },
        }
    }
}

#[derive(Debug)]
enum AppendRecordErrorKind {
    MissingObject { iid: InstrumentationId },
    WriteObject { inner: postcard::Error },
    WriteRecord { inner: postcard::Error },
}

impl fmt::Display for AppendRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, inner) = match &self.kind {
            AppendRecordErrorKind::MissingObject { iid } => {
                return write!(
                    f,
                    "failed to append record, could not get object for iid={}",
                    iid.as_u64()
                );
            }
            AppendRecordErrorKind::WriteObject { inner } => ("object", inner),
            AppendRecordErrorKind::WriteRecord { inner } => ("record", inner),
        };

        write!(f, "failed to append record, write {kind} failed: {inner}")
    }
}

impl error::Error for AppendRecordError {}

/// Error writing contents of a [`SeqChunkBuffer`] to a writer.
#[derive(Debug)]
pub struct SeqChunkWriteError {
    kind: SeqChunkWriteErrorKind,
}

impl SeqChunkWriteError {
    fn buffer_lock_poisoned() -> Self {
        Self {
            kind: SeqChunkWriteErrorKind::BufferLockPoisoned,
        }
    }

    fn header(inner: postcard::Error) -> Self {
        Self {
            kind: SeqChunkWriteErrorKind::Header { inner },
        }
    }

    fn objects_length(inner: postcard::Error) -> Self {
        Self {
            kind: SeqChunkWriteErrorKind::ObjectsLength { inner },
        }
    }

    fn objects(inner: io::Error) -> Self {
        Self {
            kind: SeqChunkWriteErrorKind::Objects { inner },
        }
    }

    fn records_length(inner: postcard::Error) -> Self {
        Self {
            kind: SeqChunkWriteErrorKind::RecordsLength { inner },
        }
    }

    fn records(inner: io::Error) -> Self {
        Self {
            kind: SeqChunkWriteErrorKind::Records { inner },
        }
    }
}

#[derive(Debug)]
enum SeqChunkWriteErrorKind {
    BufferLockPoisoned,
    Header { inner: postcard::Error },
    ObjectsLength { inner: postcard::Error },
    Objects { inner: io::Error },
    RecordsLength { inner: postcard::Error },
    Records { inner: io::Error },
}

impl fmt::Display for SeqChunkWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, inner) = match &self.kind {
            SeqChunkWriteErrorKind::BufferLockPoisoned => {
                return write!(f, "sequence chunk buffer lock is poisoned");
            }
            SeqChunkWriteErrorKind::Header { inner } => ("header", inner as &dyn fmt::Display),
            SeqChunkWriteErrorKind::ObjectsLength { inner } => {
                ("objects length", inner as &dyn fmt::Display)
            }
            SeqChunkWriteErrorKind::Objects { inner } => ("objects", inner as &dyn fmt::Display),
            SeqChunkWriteErrorKind::RecordsLength { inner } => {
                ("records length", inner as &dyn fmt::Display)
            }
            SeqChunkWriteErrorKind::Records { inner } => ("records", inner as &dyn fmt::Display),
        };

        write!(f, "failed to write sequence chunk `{kind}`: {inner}")
    }
}

impl error::Error for SeqChunkWriteError {}
