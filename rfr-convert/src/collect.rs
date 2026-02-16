use std::{collections::HashMap, convert::identity};

use rfr::{
    chunked::{self, RecordData, SeqChunkHeader, SeqId},
    common::{InstrumentationId, Task, Waker},
    rec::AbsTimestamp,
};

/// Data collected for conversion
///
/// Records are grouped by task or sequence (and sorted) so that they can be more easily converted
/// to another format.
#[derive(Debug)]
pub(crate) struct CollectedData {
    pub(crate) tasks: HashMap<InstrumentationId, TaskRecords>,
    pub(crate) sequences: HashMap<SeqId, SeqRecords>,
    pub(crate) largest_did: DynamicId,
}

impl CollectedData {
    pub(crate) fn tasks(&self) -> Vec<&TaskRecords> {
        let mut tasks: Vec<&TaskRecords> = self.tasks.values().collect();
        tasks.sort_by(|a, b| a.start.cmp(&b.start));
        tasks
    }

    pub(crate) fn sequences(&self) -> Vec<&SeqRecords> {
        let mut sequences: Vec<&SeqRecords> = self.sequences.values().collect();
        sequences.sort_by(|a, b| a.start.cmp(&b.start));
        sequences
    }
}

/// Records belonging to a sequence.
///
/// These records don't have an associated task and so are collected by sequence.
#[derive(Debug)]
pub(crate) struct SeqRecords {
    pub(crate) header: SeqChunkHeader,
    pub(crate) start: Option<AbsTimestamp>,
    pub(crate) end: Option<AbsTimestamp>,
    pub(crate) records: Vec<Record>,
}

impl SeqRecords {
    fn new(header: SeqChunkHeader) -> Self {
        Self {
            header,
            start: None,
            end: None,
            records: Vec::new(),
        }
    }

    fn add_record(&mut self, record: Record) {
        if self.records.is_empty() {
            self.start = Some(record.timestamp.clone());
        }
        self.end = Some(record.timestamp.clone());

        self.records.push(record);
    }
}

/// Records belonging to a task.
///
/// These are all the records which should show on a task's timeline.
///
/// Some of these records, such as polling start and end are for this task, but others, such as
/// wake operations are done by this task.
#[derive(Debug)]
pub(crate) struct TaskRecords {
    pub(crate) task: Task,
    pub(crate) did: DynamicId,
    pub(crate) greatest_wid: WakeId,
    pub(crate) start: AbsTimestamp,
    pub(crate) end: Option<AbsTimestamp>,
    pub(crate) records: Vec<Record>,
}

impl TaskRecords {
    fn new(task: Task, did: DynamicId, start: AbsTimestamp) -> Self {
        Self {
            task,
            did,
            greatest_wid: WakeId::ZERO,
            start,
            end: None,
            records: Vec::new(),
        }
    }

    fn add_record(&mut self, record: Record) {
        if self.records.is_empty()
            && let Data::TaskNew { .. } = &record.data
        {
            self.start = record.timestamp.clone();
        }
        if let Data::TaskDrop { .. } = &record.data {
            self.end = Some(record.timestamp.clone());
        }

        self.records.push(record);
    }
}

/// The Dynamic Id is used to provide a compact identifier for sequences, spans, and tasks which
/// can be used when linking items together.
///
/// It's important that the Id is compact (which the [`InstrumentationId`] from rfr may not be,
/// because this identifier needs to be masked with other values to created unique identifiers for
/// some events (`flow_id` for Perfetto traces).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
pub(crate) struct DynamicId(u64);

impl DynamicId {
    pub(crate) fn as_u64(&self) -> u64 {
        self.0
    }

    fn inc(&mut self) {
        self.0 += 1;
    }
}

/// Conversion record
///
/// These records are grouped differently to the records coming from a flight recording. This
/// involves some duplication that makes conversion to other formats more straight forward.
#[derive(Debug, Clone)]
pub(crate) struct Record {
    pub(crate) timestamp: AbsTimestamp,
    pub(crate) data: Data,
}

/// Record Data for conversion records.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Data {
    /// A new task
    ///
    /// This will be the first record for a task.
    TaskNew { iid: InstrumentationId },

    /// Task is dropped
    ///
    /// This will be the last record for a task.
    TaskDrop { iid: InstrumentationId },

    /// A task poll started
    TaskPollStart { iid: InstrumentationId },
    /// A task poll ended
    TaskPollEnd { iid: InstrumentationId },

    // This task was woken
    //
    /// A waker was invoked, waking this task. This record appears on the WOKEN task's timeline.
    WakerWoken {
        woken_by: Option<InstrumentationId>,
        action: WakerAction,
        wid: WakeId,
    },

    /// This task invoked a waker, waking another task
    ///
    /// This record appears on the WAKING task's timeline.
    ///
    /// If `action` is [`WakerAction::Consume`] then there is now 1 few wakers for this task.
    WakerWake {
        woken: InstrumentationId,
        action: WakerAction,
        wid: WakeId,
    },

    /// A waker was cloned
    ///
    /// There is now 1 additional waker for this task.
    WakerClone { waker: Waker },

    /// A waker was dropped
    ///
    /// There is now 1 fewer waker for this task.
    WakerDrop { waker: Waker },

    // Task was spawned.
    //
    // This is the event for the task (or sequence) that spawned a task.
    Spawn {
        spawned_iid: InstrumentationId,
        by_iid: InstrumentationId,
    },
}

/// The Wake Id is used to differentiate different wake (or wake by ref) operations for the purpose
/// of generating an ID to link the waker to the next poll (`flow_id` for Perfetto traces).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
pub(crate) struct WakeId(u64);

impl WakeId {
    pub(crate) const ZERO: WakeId = WakeId(0);

    pub(crate) fn as_u64(&self) -> u64 {
        self.0
    }

    fn inc(&mut self) -> Self {
        self.0 += 1;
        *self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WakerAction {
    /// Consuming wake (from `Waker::wake()`)
    Consume,
    /// Non-consuming wake (from `Waker::wake_by_ref()`)
    ByRef,
}

pub(crate) fn collect_tasks(
    recording_path: &str,
) -> Result<CollectedData, Box<dyn std::error::Error>> {
    let mut recording = chunked::from_path(recording_path.to_string())
        .map_err(|e| format!("failed to open recording: {e:?}"))?;
    recording.load_all_chunks();

    let earliest_timestamp = recording
        .chunks_lossy()
        .find_map(identity)
        .map(|chunk| chunk.abs_timestamp(&chunk.header().earliest_timestamp))
        .ok_or_else(|| "no chunks with valid timestamp found".to_string())?;

    let mut tasks: HashMap<InstrumentationId, TaskRecords> = HashMap::new();
    let mut sequences: HashMap<SeqId, SeqRecords> = HashMap::new();

    let mut dyn_id = DynamicId(0);
    for chunk in recording.chunks_lossy() {
        let Some(chunk) = chunk else { continue };
        for seq_chunk in chunk.seq_chunks() {
            sequences
                .entry(seq_chunk.header.seq_id)
                .or_insert_with(|| SeqRecords::new(seq_chunk.header.clone()));

            for object in &seq_chunk.objects {
                if let chunked::Object::Task(task) = object {
                    tasks.entry(task.iid).or_insert_with(|| {
                        dyn_id.inc();
                        TaskRecords::new(task.clone(), dyn_id, earliest_timestamp.clone())
                    });
                }
            }
        }
    }

    enum AddTo {
        Task(InstrumentationId),
        Sequence,
    }
    impl From<&InstrumentationId> for AddTo {
        fn from(value: &InstrumentationId) -> Self {
            Self::Task(*value)
        }
    }
    impl From<InstrumentationId> for AddTo {
        fn from(value: InstrumentationId) -> Self {
            Self::Task(value)
        }
    }
    for chunk in recording.chunks_lossy() {
        let Some(chunk) = chunk else { continue };
        for seq_chunk in chunk.seq_chunks() {
            let seq_records = sequences
                .get_mut(&seq_chunk.header.seq_id)
                .expect("We added all the sequences a few lines above");
            for record in &seq_chunk.records {
                // Convert RecordData to one or more (task_iid, Data) pairs
                let records_to_add: Vec<(AddTo, Data)> = match &record.data {
                    RecordData::TaskNew { iid } => vec![(iid.into(), Data::TaskNew { iid: *iid })],
                    RecordData::TaskPollStart { iid } => {
                        vec![(iid.into(), Data::TaskPollStart { iid: *iid })]
                    }
                    RecordData::TaskPollEnd { iid } => {
                        vec![(iid.into(), Data::TaskPollEnd { iid: *iid })]
                    }
                    RecordData::TaskDrop { iid } => {
                        vec![(iid.into(), Data::TaskDrop { iid: *iid })]
                    }

                    RecordData::WakerWake { waker } => {
                        let wid = tasks
                            .get_mut(&waker.task_iid)
                            .map(|t| t.greatest_wid.inc())
                            .unwrap_or(WakeId::ZERO);

                        let mut records = vec![(
                            waker.task_iid.into(),
                            Data::WakerWoken {
                                woken_by: waker.context,
                                action: WakerAction::Consume,
                                wid,
                            },
                        )];

                        let data = Data::WakerWake {
                            woken: waker.task_iid,
                            action: WakerAction::Consume,
                            wid,
                        };
                        if let Some(waking_task_iid) = waker.context {
                            records.push((waking_task_iid.into(), data));
                        } else {
                            records.push((AddTo::Sequence, data));
                        }

                        records
                    }

                    RecordData::WakerWakeByRef { waker } => {
                        let wid = tasks
                            .get_mut(&waker.task_iid)
                            .map(|t| t.greatest_wid.inc())
                            .unwrap_or(WakeId::ZERO);

                        let mut records = vec![(
                            waker.task_iid.into(),
                            Data::WakerWoken {
                                woken_by: waker.context,
                                action: WakerAction::ByRef,
                                wid,
                            },
                        )];

                        let data = Data::WakerWake {
                            woken: waker.task_iid,
                            action: WakerAction::ByRef,
                            wid,
                        };
                        if let Some(waking_task_iid) = waker.context {
                            records.push((waking_task_iid.into(), data));
                        } else {
                            records.push((AddTo::Sequence, data));
                        }

                        records
                    }

                    RecordData::WakerClone { waker } => vec![(
                        waker.task_iid.into(),
                        Data::WakerClone {
                            waker: waker.clone(),
                        },
                    )],

                    RecordData::WakerDrop { waker } => vec![(
                        waker.task_iid.into(),
                        Data::WakerDrop {
                            waker: waker.clone(),
                        },
                    )],

                    _ => continue,
                };

                let timestamp = chunk.abs_timestamp(&record.meta.timestamp);

                // Handle TaskNew specially for spawn tracking
                for (_, data) in &records_to_add {
                    if let Data::TaskNew { iid } = data
                        && let Some(task) = tasks.get(iid)
                        && let Some(context) = &task.task.context
                    {
                        let spawn_record = Record {
                            timestamp: timestamp.clone(),
                            data: Data::Spawn {
                                spawned_iid: task.task.iid,
                                by_iid: *context,
                            },
                        };

                        tasks
                            .entry(*context)
                            .and_modify(|t| t.add_record(spawn_record));
                    }
                }

                // Add all records to their respective tasks
                for (add_to, data) in records_to_add {
                    let new_record = Record {
                        timestamp: timestamp.clone(),
                        data,
                    };

                    match add_to {
                        AddTo::Task(task_iid) => {
                            tasks
                                .entry(task_iid)
                                .and_modify(|t| t.add_record(new_record));
                        }
                        AddTo::Sequence => seq_records.add_record(new_record),
                    }
                }
            }
        }
    }

    for task_records in tasks.values_mut() {
        task_records.records.sort_by_key(|r| r.timestamp.clone());
    }

    Ok(CollectedData {
        tasks,
        sequences,
        largest_did: dyn_id,
    })
}
