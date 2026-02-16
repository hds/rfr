use std::{collections::HashMap, convert::identity, fmt, ops::Add, time::Duration};

use rfr::{
    chunked::{self, RecordData},
    common::{InstrumentationId, Task},
    rec::{self, AbsTimestamp, WinTimestamp, from_file},
};

pub(crate) struct WinTimeHandle {
    start_time: rec::AbsTimestamp,
}

fn duration_from_abs_timestamp(abs_time: &rec::AbsTimestamp) -> Duration {
    Duration::new(abs_time.secs, abs_time.subsec_micros * 1000)
}

impl WinTimeHandle {
    pub(crate) fn new(recording_start_time: rec::AbsTimestamp) -> Self {
        Self {
            start_time: recording_start_time,
        }
    }

    pub(crate) fn window_time(&self, abs_time: &rec::AbsTimestamp) -> rec::WinTimestamp {
        let start_time = duration_from_abs_timestamp(&self.start_time);
        let duration = Duration::new(abs_time.secs, abs_time.subsec_micros * 1000);
        let window_micros = duration.saturating_sub(start_time).as_micros();
        debug_assert!(
            window_micros < u64::MAX as u128,
            "recording time spans more than u64::MAX microseconds, which is more than 500 thousand years"
        );

        rec::WinTimestamp {
            micros: window_micros as u64,
        }
    }
}

struct TaskTimeHandle {
    start_time: rec::WinTimestamp,
}

impl TaskTimeHandle {
    fn new(task_start_time: rec::WinTimestamp) -> Self {
        Self {
            start_time: task_start_time,
        }
    }

    fn task_time(&self, win_time: &rec::WinTimestamp) -> TaskTimestamp {
        TaskTimestamp {
            micros: win_time.micros.saturating_sub(self.start_time.micros),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TaskTimestamp {
    micros: u64,
}

impl TaskTimestamp {
    fn saturating_sub(&self, other: &Self) -> u64 {
        self.micros.saturating_sub(other.micros)
    }

    fn is_zero(&self) -> bool {
        self.micros == 0
    }
}

impl Add<TaskTimestamp> for rec::WinTimestamp {
    type Output = Self;

    fn add(self, rhs: TaskTimestamp) -> Self::Output {
        WinTimestamp {
            micros: self.micros + rhs.micros,
        }
    }
}

pub(crate) struct RecordingInfo {
    pub(crate) task_rows: Vec<TaskRow>,
    // We will use this to print timestamps
    #[allow(dead_code)]
    pub(crate) win_time_handle: WinTimeHandle,
    pub(crate) end_time: rec::WinTimestamp,
}

#[derive(Debug)]
pub(crate) struct TaskRecords {
    pub(crate) task: Task,
    pub(crate) records: Vec<Record>,
}

impl TaskRecords {
    fn new(task: Task) -> Self {
        Self {
            task,
            records: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Record {
    pub(crate) timestamp: AbsTimestamp,
    pub(crate) data: chunked::RecordData,
}

trait TaskRecordsCollect {
    fn collect_into_tasks(&mut self) -> Vec<TaskRecords>;

    fn earliest_timestamp(&mut self) -> Option<AbsTimestamp>;
    fn latest_timestamp(&mut self) -> Option<AbsTimestamp>;
}

impl TaskRecordsCollect for Vec<rec::Record> {
    fn collect_into_tasks(&mut self) -> Vec<TaskRecords> {
        if self.is_empty() {
            return vec![];
        }

        collect_into_tasks_from_streaming_records(self)
    }

    fn earliest_timestamp(&mut self) -> Option<AbsTimestamp> {
        self.first().map(|r| r.meta.timestamp.clone())
    }

    fn latest_timestamp(&mut self) -> Option<AbsTimestamp> {
        self.last().map(|r| r.meta.timestamp.clone())
    }
}

impl TaskRecordsCollect for chunked::Recording {
    fn collect_into_tasks(&mut self) -> Vec<TaskRecords> {
        collect_into_tasks_from_chunked_recording(self)
    }

    fn earliest_timestamp(&mut self) -> Option<AbsTimestamp> {
        self.chunks_lossy()
            .find_map(identity)
            .map(|chunk| chunk.abs_timestamp(&chunk.header().earliest_timestamp))
    }

    fn latest_timestamp(&mut self) -> Option<AbsTimestamp> {
        self.chunks_lossy()
            .rev()
            .find_map(identity)
            .map(|chunk| chunk.abs_timestamp(&chunk.header().latest_timestamp))
    }
}

pub(crate) fn streaming_recording_info(path: String) -> Option<RecordingInfo> {
    let records = from_file(path);

    create_recording_info(records)
}

pub(crate) fn chunked_recording_info(path: String) -> Option<RecordingInfo> {
    let mut recording = chunked::from_path(path).unwrap();
    recording.load_all_chunks();
    println!("Recording: {:?}", recording.meta());
    for chunk in recording.chunks_lossy() {
        let Some(chunk) = chunk else { continue };
        println!("\n--------------------------------");
        println!("Chunk: {:?}", chunk.header());
        for seq_chunk in chunk.seq_chunks() {
            println!(
                "- Sequence Chunk: {:?} ({:?} - {:?})",
                seq_chunk.header.seq_id,
                seq_chunk.header.earliest_timestamp,
                seq_chunk.header.latest_timestamp
            );
            println!("  - Objects:");
            for object in &seq_chunk.objects {
                println!("    - {object:?}");
            }
            println!("  - Records:");
            for records in &seq_chunk.records {
                println!("    - {records:?}");
            }
        }
    }
    println!("--------------------------------");

    create_recording_info(recording)
}

fn create_recording_info(recording: impl TaskRecordsCollect) -> Option<RecordingInfo> {
    let mut recording = recording;

    let start_timestamp = recording.earliest_timestamp()?;
    let win_time_handle = WinTimeHandle::new(start_timestamp);

    let end_timestamp = recording.latest_timestamp()?;
    let end_time = win_time_handle.window_time(&end_timestamp);

    let tasks_records = recording.collect_into_tasks();
    if tasks_records.is_empty() {
        return None;
    }
    let task_rows = collect_into_rows(&win_time_handle, tasks_records);
    if task_rows.is_empty() {
        return None;
    }

    Some(RecordingInfo {
        task_rows,
        win_time_handle,
        end_time,
    })
}

pub(crate) fn collect_into_tasks_from_chunked_recording(
    recording: &mut chunked::Recording,
) -> Vec<TaskRecords> {
    let mut tasks = HashMap::new();

    for chunk in recording.chunks_lossy() {
        let Some(chunk) = chunk else { continue };
        for seq_chunk in chunk.seq_chunks() {
            for object in &seq_chunk.objects {
                if let chunked::Object::Task(task) = object {
                    tasks
                        .entry(task.iid)
                        .or_insert_with(|| TaskRecords::new(task.clone()));
                }
            }

            for record in &seq_chunk.records {
                let task_iid = match &record.data {
                    RecordData::TaskNew { iid }
                    | RecordData::TaskPollStart { iid }
                    | RecordData::TaskPollEnd { iid }
                    | RecordData::TaskDrop { iid } => iid,
                    RecordData::WakerWake { waker }
                    | RecordData::WakerWakeByRef { waker }
                    | RecordData::WakerClone { waker }
                    | RecordData::WakerDrop { waker } => &waker.task_iid,
                    _ => continue,
                };

                let record = Record {
                    timestamp: chunk.abs_timestamp(&record.meta.timestamp),
                    data: record.data.clone(),
                };

                tasks
                    .entry(*task_iid)
                    .and_modify(|r| r.records.push(record));
            }
        }
    }

    tasks
        .into_values()
        .map(|mut task_records| {
            task_records.records.sort_by_key(|r| r.timestamp.clone());
            task_records
        })
        .collect()
}

pub(crate) fn collect_into_tasks_from_streaming_records(
    records: &Vec<rec::Record>,
) -> Vec<TaskRecords> {
    let mut tasks = HashMap::new();

    for record in records {
        if let rec::RecordData::End = &record.data {
            // This should be the end of the list of records.
            // FIXME: Break?
        } else if let rec::RecordData::Callsite { callsite } = &record.data {
            // TODO: Do something with the Callsite to support Spans and Events.
            _ = callsite;
        } else if let rec::RecordData::Task { task } = &record.data {
            let task_entry = TaskRecords::new(task.clone());
            tasks.insert(task.iid, task_entry);
        } else {
            let (record_data, iid) = match &record.data {
                rec::RecordData::TaskNew { iid } => (RecordData::TaskNew { iid: *iid }, *iid),
                rec::RecordData::TaskPollStart { iid } => {
                    (RecordData::TaskPollStart { iid: *iid }, *iid)
                }
                rec::RecordData::TaskPollEnd { iid } => {
                    (RecordData::TaskPollEnd { iid: *iid }, *iid)
                }
                rec::RecordData::TaskDrop { iid } => (RecordData::TaskDrop { iid: *iid }, *iid),
                rec::RecordData::WakerWake { waker } => (
                    RecordData::WakerWake {
                        waker: waker.clone(),
                    },
                    waker.task_iid,
                ),
                rec::RecordData::WakerWakeByRef { waker } => (
                    RecordData::WakerWakeByRef {
                        waker: waker.clone(),
                    },
                    waker.task_iid,
                ),
                rec::RecordData::WakerClone { waker } => (
                    RecordData::WakerClone {
                        waker: waker.clone(),
                    },
                    waker.task_iid,
                ),
                rec::RecordData::WakerDrop { waker } => (
                    RecordData::WakerDrop {
                        waker: waker.clone(),
                    },
                    waker.task_iid,
                ),
                rec::RecordData::Task { task: _ } => {
                    unreachable!("task records have already been filtered out")
                }
                _ => {
                    todo!("support for spans and events no yet implemented")
                }
            };
            let record = Record {
                timestamp: record.meta.timestamp.clone(),
                data: record_data,
            };
            tasks.entry(iid).and_modify(|r| r.records.push(record));
        }
    }

    tasks.into_values().collect()
}

pub(crate) struct TaskRow {
    pub(crate) index: TaskIndex,
    pub(crate) start_time: rec::WinTimestamp,
    pub(crate) task: Task,
    pub(crate) sections: Vec<TaskSection>,
    pub(crate) last_state: Option<TaskState>,
    pub(crate) spawn: Option<SpawnRecord>,
    pub(crate) wakings: Vec<WakeRecord>,
}

impl TaskRow {
    pub(crate) fn total_duration(&self) -> u64 {
        self.sections.iter().map(|s| s.duration).sum()
    }
}

#[derive(Debug)]
pub(crate) struct TaskSection {
    pub(crate) duration: u64,
    pub(crate) state: TaskState,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TaskState {
    Active,
    Idle,
    ActiveScheduled,
    IdleScheduled,
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Active => "active",
                Self::Idle => "idle",
                Self::ActiveScheduled => "active",
                Self::IdleScheduled => "scheduled",
            }
        )
    }
}

#[derive(Debug, Clone)]
struct TaskRecord {
    ts: TaskTimestamp,
    kind: TaskRecordKind,
}

#[derive(Debug, Clone)]
enum TaskRecordKind {
    New,
    PollStart,
    PollEnd,
    Drop,
    Wake,
}

impl fmt::Display for TaskRecordKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::New => "new",
                Self::PollStart => "poll start",
                Self::PollEnd => "poll end",
                Self::Drop => "drop",
                Self::Wake => "wake",
            }
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WakeRecord {
    pub(crate) ts: TaskTimestamp,
    pub(crate) kind: WakeRecordKind,
}

#[derive(Debug, Clone)]
pub(crate) enum WakeRecordKind {
    Wake { by: Option<TaskIndex> },
    WakeByRef { by: Option<TaskIndex> },
    SelfWake,
    SelfWakeByRef,
    Clone,
    Drop,
}

impl fmt::Display for WakeRecordKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Wake { .. } => "W",
                Self::WakeByRef { .. } => "*W",
                Self::SelfWake => "sW",
                Self::SelfWakeByRef => "*sW",
                Self::Clone => "C",
                Self::Drop => "D",
            }
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SpawnRecord {
    pub(crate) ts: TaskTimestamp,
    pub(crate) kind: SpawnRecordKind,
}

#[derive(Debug, Clone)]
pub(crate) enum SpawnRecordKind {
    Spawn { by: Option<TaskIndex> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TaskIndex {
    index: usize,
}

impl TaskIndex {
    pub(crate) fn new(index: usize) -> Self {
        Self { index }
    }

    pub(crate) fn as_inner(&self) -> usize {
        self.index
    }
}

pub(crate) fn collect_into_rows(
    win_time_handle: &WinTimeHandle,
    tasks_records: Vec<TaskRecords>,
) -> Vec<TaskRow> {
    let mut tasks_records = tasks_records;
    tasks_records.sort_by_key(|t| t.task.iid);

    let tasks_with_indicies: Vec<_> = tasks_records
        .into_iter()
        .enumerate()
        .map(|(idx, task_records)| (TaskIndex::new(idx), task_records))
        .collect();
    let task_indices: HashMap<_, _> = tasks_with_indicies
        .iter()
        .map(|(idx, task_records)| (task_records.task.iid, *idx))
        .collect();
    let get_index = |task_iid: Option<InstrumentationId>| {
        task_iid.and_then(|iid| task_indices.get(&iid).copied())
    };

    let mut task_rows = Vec::new();
    for (index, TaskRecords { task, records }) in tasks_with_indicies {
        if records.is_empty() {
            continue;
        }

        let first = &records.first().expect("records is not empty");
        let start_time = if let RecordData::TaskNew { .. } = &first.data {
            // The record starts within this window
            win_time_handle.window_time(&first.timestamp)
        } else {
            // The task started before this window, so we set the task time to start with
            // the window.
            rec::WinTimestamp::ZERO
        };
        let task_time_handle = TaskTimeHandle::new(start_time.clone());

        let mut task_records = Vec::new();
        let mut wake_records = Vec::new();
        let mut spawn_record = None;
        for rec in records {
            let ts = task_time_handle.task_time(&win_time_handle.window_time(&rec.timestamp));

            match &rec.data {
                RecordData::TaskNew { .. } => {
                    debug_assert!(spawn_record.is_none(), "multiple NewTask records");
                    spawn_record = Some(SpawnRecord {
                        ts,
                        kind: SpawnRecordKind::Spawn {
                            by: get_index(task.context),
                        },
                    });
                    task_records.push(TaskRecord {
                        ts,
                        kind: TaskRecordKind::New,
                    });
                }
                RecordData::TaskPollStart { .. } => task_records.push(TaskRecord {
                    ts,
                    kind: TaskRecordKind::PollStart,
                }),
                RecordData::TaskPollEnd { .. } => task_records.push(TaskRecord {
                    ts,
                    kind: TaskRecordKind::PollEnd,
                }),
                RecordData::TaskDrop { .. } => task_records.push(TaskRecord {
                    ts,
                    kind: TaskRecordKind::Drop,
                }),
                RecordData::WakerWake { waker } => {
                    task_records.push(TaskRecord {
                        ts,
                        kind: TaskRecordKind::Wake,
                    });

                    let kind = if Some(waker.task_iid) == waker.context {
                        WakeRecordKind::SelfWake
                    } else {
                        WakeRecordKind::Wake {
                            by: get_index(waker.context),
                        }
                    };

                    wake_records.push(WakeRecord { ts, kind });
                }
                RecordData::WakerWakeByRef { waker } => {
                    task_records.push(TaskRecord {
                        ts,
                        kind: TaskRecordKind::Wake,
                    });

                    let kind = if Some(waker.task_iid) == waker.context {
                        WakeRecordKind::SelfWakeByRef
                    } else {
                        WakeRecordKind::WakeByRef {
                            by: get_index(waker.context),
                        }
                    };

                    wake_records.push(WakeRecord { ts, kind });
                }
                RecordData::WakerClone { .. } => wake_records.push(WakeRecord {
                    ts,
                    kind: WakeRecordKind::Clone,
                }),
                RecordData::WakerDrop { .. } => wake_records.push(WakeRecord {
                    ts,
                    kind: WakeRecordKind::Drop,
                }),
                _ => continue, // Skip unknown records
            }
        }

        let mut task_sections = Vec::new();
        if task_records.is_empty() {
            continue;
        }
        let first = task_records.first().unwrap();

        if !first.ts.is_zero() {
            let extra_section_state = match &first.kind {
                TaskRecordKind::New => None,
                TaskRecordKind::PollStart => Some(TaskState::IdleScheduled),
                TaskRecordKind::PollEnd => Some(TaskState::Active),
                TaskRecordKind::Drop => Some(TaskState::Idle),
                TaskRecordKind::Wake => {
                    if let Some(second) = task_records.get(1) {
                        if let TaskRecordKind::PollEnd = second.kind {
                            Some(TaskState::Active)
                        } else {
                            Some(TaskState::Idle)
                        }
                    } else {
                        Some(TaskState::Idle)
                    }
                }
            };

            if let Some(state) = extra_section_state {
                task_sections.push(TaskSection {
                    duration: first.ts.micros,
                    state,
                });
            }
        }

        for curr_idx in 1..task_records.len() {
            let current = &task_records[curr_idx];
            let prev = &task_records[curr_idx - 1];
            use TaskRecordKind::{Drop, New, PollEnd, PollStart, Wake};

            let section = match &current.kind {
                New => Section::Invalid {
                    from: prev.kind.clone(),
                    to: current.kind.clone(),
                },
                PollStart => match &prev.kind {
                    New | PollEnd => Section::New(TaskState::Idle),
                    Wake => Section::New(TaskState::IdleScheduled),
                    PollStart | Drop => Section::Invalid {
                        from: prev.kind.clone(),
                        to: current.kind.clone(),
                    },
                },
                PollEnd => match &prev.kind {
                    PollStart => Section::New(TaskState::Active),
                    Wake => Section::New(TaskState::ActiveScheduled),
                    New | PollEnd | Drop => Section::Invalid {
                        from: prev.kind.clone(),
                        to: current.kind.clone(),
                    },
                },
                Drop => match &prev.kind {
                    New | PollEnd => Section::ReplaceWith {
                        replace_last_n_sections: 2,
                        new_state: TaskState::Idle,
                    },
                    Wake => Section::ReplaceWith {
                        replace_last_n_sections: 2,
                        new_state: TaskState::IdleScheduled,
                    },
                    PollStart | Drop => Section::Invalid {
                        from: prev.kind.clone(),
                        to: current.kind.clone(),
                    },
                },
                Wake => match &prev.kind {
                    New | PollEnd => Section::New(TaskState::Idle),
                    PollStart => Section::New(TaskState::Active),
                    Wake => Section::ExtendLast,
                    Drop => Section::Invalid {
                        from: prev.kind.clone(),
                        to: current.kind.clone(),
                    },
                },
            };

            extend_task_sections(&mut task_sections, section, prev.ts, current.ts);
        }

        let last_state = last_state(&task_records, &mut task_sections);

        println!("\n======== {task:?} ========");
        println!("task_records: {task_records:?}");
        println!("task_sections: {task_sections:?}");
        println!("last_state: {last_state:?}");
        println!("spawn_records: {spawn_record:?}");
        println!("wake_records: {wake_records:?}");
        println!("======== ======== ======== ========");

        task_rows.push(TaskRow {
            index,
            start_time,
            task,
            sections: task_sections,
            last_state,
            spawn: spawn_record,
            wakings: wake_records,
        });
    }

    task_rows
}

enum Section {
    New(TaskState),
    ReplaceWith {
        replace_last_n_sections: usize,
        new_state: TaskState,
    },
    ExtendLast,
    Invalid {
        #[allow(dead_code)]
        from: TaskRecordKind,
        #[allow(dead_code)]
        to: TaskRecordKind,
    },
}

fn extend_task_sections(
    task_sections: &mut Vec<TaskSection>,
    section: Section,
    prev_ts: TaskTimestamp,
    current_ts: TaskTimestamp,
) {
    match section {
        Section::New(state) => {
            task_sections.push(TaskSection {
                duration: current_ts.saturating_sub(&prev_ts),
                state,
            });
        }
        Section::ReplaceWith {
            replace_last_n_sections,
            new_state,
        } => {
            // TODO(hds): should probably emit a warning if this would be less than 2.
            task_sections.truncate(task_sections.len().saturating_sub(replace_last_n_sections));
            task_sections.push(TaskSection {
                duration: current_ts.saturating_sub(&prev_ts),
                state: new_state,
            });
        }
        Section::ExtendLast => {
            if let Some(last) = task_sections.last_mut() {
                last.duration += current_ts.saturating_sub(&prev_ts);
            } else {
                // Report error (this state shouldn't occur)
            }
        }
        Section::Invalid { .. } => {
            // Report warning and then continue
        }
    }
}

fn last_state(
    task_records: &[TaskRecord],
    task_sections: &mut Vec<TaskSection>,
) -> Option<TaskState> {
    if task_records.len() <= 1 {
        return None;
    }

    let prev_section = task_sections.last()?;
    let last_record = task_records.last()?;

    match &last_record.kind {
        TaskRecordKind::New => {
            // This is an error, but we will have caught it in the main loop
            None
        }
        TaskRecordKind::PollStart => Some(TaskState::Active),
        TaskRecordKind::PollEnd => Some(TaskState::Idle),
        TaskRecordKind::Drop => None,
        TaskRecordKind::Wake => match prev_section.state {
            TaskState::Active => Some(TaskState::ActiveScheduled),
            TaskState::Idle => Some(TaskState::IdleScheduled),
            TaskState::ActiveScheduled | TaskState::IdleScheduled => {
                let last_section = task_sections
                    .pop()
                    .expect("we already checked that `task_records` has at least 2 elements");
                Some(last_section.state)
            }
        },
    }
}
