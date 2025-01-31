use std::{collections::HashMap, convert::identity, fmt, ops::Add, time::Duration};

use rfr::{
    chunked::{self, RecordData},
    common::{InstrumentationId, Task},
    rec::{self, from_file, AbsTimestamp, WinTimestamp},
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
        debug_assert!(window_micros < u64::MAX as u128, "recording time spans more than u64::MAX microseconds, which is more than 500 thousand years");

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

#[derive(Debug, Clone)]
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
pub(crate) struct TaskEvents {
    pub(crate) task: Task,
    pub(crate) events: Vec<EventRecord>,
}

impl TaskEvents {
    fn new(task: Task) -> Self {
        Self {
            task,
            events: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct EventRecord {
    pub(crate) timestamp: AbsTimestamp,
    pub(crate) data: chunked::RecordData,
}

trait TaskEventsCollect {
    fn collect_into_tasks(&mut self) -> Vec<TaskEvents>;

    fn earliest_timestamp(&mut self) -> Option<AbsTimestamp>;
    fn latest_timestamp(&mut self) -> Option<AbsTimestamp>;
}

impl TaskEventsCollect for Vec<rec::Record> {
    fn collect_into_tasks(&mut self) -> Vec<TaskEvents> {
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

impl TaskEventsCollect for chunked::Recording {
    fn collect_into_tasks(&mut self) -> Vec<TaskEvents> {
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
            println!("  - Events:");
            for events in &seq_chunk.records {
                println!("    - {events:?}");
            }
        }
    }
    println!("--------------------------------");

    create_recording_info(recording)
}

fn create_recording_info(recording: impl TaskEventsCollect) -> Option<RecordingInfo> {
    let mut recording = recording;

    let start_timestamp = recording.earliest_timestamp()?;
    let win_time_handle = WinTimeHandle::new(start_timestamp);

    let end_timestamp = recording.latest_timestamp()?;
    let end_time = win_time_handle.window_time(&end_timestamp);

    let tasks_events = recording.collect_into_tasks();
    if tasks_events.is_empty() {
        return None;
    }
    let task_rows = collect_into_rows(&win_time_handle, tasks_events);
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
) -> Vec<TaskEvents> {
    let mut tasks = HashMap::new();

    for chunk in recording.chunks_lossy() {
        let Some(chunk) = chunk else { continue };
        for seq_chunk in chunk.seq_chunks() {
            for object in &seq_chunk.objects {
                if let chunked::Object::Task(task) = object {
                    tasks
                        .entry(task.iid)
                        .or_insert_with(|| TaskEvents::new(task.clone()));
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

                let record = EventRecord {
                    timestamp: chunk.abs_timestamp(&record.meta.timestamp),
                    data: record.data.clone(),
                };

                tasks.entry(*task_iid).and_modify(|r| r.events.push(record));
            }
        }
    }

    tasks
        .into_values()
        .map(|mut task_events| {
            task_events.events.sort_by_key(|r| r.timestamp.clone());
            task_events
        })
        .collect()
}

pub(crate) fn collect_into_tasks_from_streaming_records(
    records: &Vec<rec::Record>,
) -> Vec<TaskEvents> {
    let mut tasks = HashMap::new();

    for record in records {
        if let rec::RecordData::End = &record.data {
            // This should be the end of the list of records.
            // FIXME: Break?
        } else if let rec::RecordData::Callsite { callsite } = &record.data {
            // TODO: Do something with the Callsite to support Spans and Events.
            _ = callsite;
        } else if let rec::RecordData::Task { task } = &record.data {
            let task_entry = TaskEvents::new(task.clone());
            tasks.insert(task.iid, task_entry);
        } else {
            let (event, iid) = match &record.data {
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
                    unreachable!("task events have already been filtered out")
                }
                _ => {
                    todo!("support for spans and events no yet implemented")
                }
            };
            let record = EventRecord {
                timestamp: record.meta.timestamp.clone(),
                data: event,
            };
            tasks.entry(iid).and_modify(|r| r.events.push(record));
        }
    }

    tasks.into_values().collect()
}

pub(crate) struct TaskRow {
    pub(crate) index: TaskIndex,
    pub(crate) start_time: rec::WinTimestamp,
    pub(crate) task: Task,
    pub(crate) sections: Vec<TaskSection>,
    pub(crate) spawn: Option<SpawnEvent>,
    pub(crate) wakings: Vec<WakeEvent>,
}

#[derive(Debug)]
pub(crate) struct TaskSection {
    pub(crate) duration: u64,
    pub(crate) state: TaskState,
}

#[derive(Debug)]
pub(crate) enum TaskState {
    Active,
    Idle,
    ActiveSchedueld,
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
                Self::ActiveSchedueld => "active",
                Self::IdleScheduled => "scheduled",
            }
        )
    }
}

#[derive(Debug, Clone)]
struct TaskEvent {
    ts: TaskTimestamp,
    kind: TaskEventKind,
}

#[derive(Debug, Clone)]
enum TaskEventKind {
    New,
    PollStart,
    PollEnd,
    Drop,
    Wake,
}

impl fmt::Display for TaskEventKind {
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
pub(crate) struct WakeEvent {
    pub(crate) ts: TaskTimestamp,
    pub(crate) kind: WakeEventKind,
}

#[derive(Debug, Clone)]
pub(crate) enum WakeEventKind {
    Wake { by: Option<TaskIndex> },
    WakeByRef { by: Option<TaskIndex> },
    SelfWake,
    SelfWakeByRef,
    Clone,
    Drop,
}

impl fmt::Display for WakeEventKind {
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
pub(crate) struct SpawnEvent {
    pub(crate) ts: TaskTimestamp,
    pub(crate) kind: SpawnEventKind,
}

#[derive(Debug, Clone)]
pub(crate) enum SpawnEventKind {
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
    tasks_events: Vec<TaskEvents>,
) -> Vec<TaskRow> {
    let mut tasks_events = tasks_events;
    tasks_events.sort_by_key(|t| t.task.iid);

    let tasks_with_indicies: Vec<_> = tasks_events
        .into_iter()
        .enumerate()
        .map(|(idx, task_events)| (TaskIndex::new(idx), task_events))
        .collect();
    let task_indices: HashMap<_, _> = tasks_with_indicies
        .iter()
        .map(|(idx, task_events)| (task_events.task.iid, *idx))
        .collect();
    let get_index = |task_iid: Option<InstrumentationId>| {
        task_iid.and_then(|iid| task_indices.get(&iid).copied())
    };

    let mut task_rows = Vec::new();
    for (index, TaskEvents { task, events }) in tasks_with_indicies {
        if events.is_empty() {
            continue;
        }

        let first = &events.first().expect("events is not empty");
        let start_time = if let RecordData::TaskNew { .. } = &first.data {
            // The event starts within this window
            win_time_handle.window_time(&first.timestamp)
        } else {
            // The task started before this window, so we set the task time to start with
            // the window.
            rec::WinTimestamp::ZERO
        };
        let task_time_handle = TaskTimeHandle::new(start_time.clone());

        let mut task_events = Vec::new();
        let mut wake_events = Vec::new();
        let mut spawn_event = None;
        for rec in events {
            let ts = task_time_handle.task_time(&win_time_handle.window_time(&rec.timestamp));

            match &rec.data {
                RecordData::TaskNew { .. } => {
                    debug_assert!(spawn_event.is_none(), "multiple NewTask events");
                    spawn_event = Some(SpawnEvent {
                        ts: ts.clone(),
                        kind: SpawnEventKind::Spawn {
                            by: get_index(task.context),
                        },
                    });
                    task_events.push(TaskEvent {
                        ts,
                        kind: TaskEventKind::New,
                    });
                }
                RecordData::TaskPollStart { .. } => task_events.push(TaskEvent {
                    ts,
                    kind: TaskEventKind::PollStart,
                }),
                RecordData::TaskPollEnd { .. } => task_events.push(TaskEvent {
                    ts,
                    kind: TaskEventKind::PollEnd,
                }),
                RecordData::TaskDrop { .. } => task_events.push(TaskEvent {
                    ts,
                    kind: TaskEventKind::Drop,
                }),
                RecordData::WakerWake { waker } => {
                    task_events.push(TaskEvent {
                        ts: ts.clone(),
                        kind: TaskEventKind::Wake,
                    });

                    let kind = if Some(waker.task_iid) == waker.context {
                        WakeEventKind::SelfWake
                    } else {
                        WakeEventKind::Wake {
                            by: get_index(waker.context),
                        }
                    };

                    wake_events.push(WakeEvent { ts, kind });
                }
                RecordData::WakerWakeByRef { waker } => {
                    task_events.push(TaskEvent {
                        ts: ts.clone(),
                        kind: TaskEventKind::Wake,
                    });

                    let kind = if Some(waker.task_iid) == waker.context {
                        WakeEventKind::SelfWakeByRef
                    } else {
                        WakeEventKind::WakeByRef {
                            by: get_index(waker.context),
                        }
                    };

                    wake_events.push(WakeEvent { ts, kind });
                }
                RecordData::WakerClone { .. } => wake_events.push(WakeEvent {
                    ts,
                    kind: WakeEventKind::Clone,
                }),
                RecordData::WakerDrop { .. } => wake_events.push(WakeEvent {
                    ts,
                    kind: WakeEventKind::Drop,
                }),
                _ => continue, // Skip unknown events
            }
        }

        let mut task_sections = Vec::new();
        if task_events.is_empty() {
            continue;
        }
        let first = task_events.first().unwrap();

        if !first.ts.is_zero() {
            let extra_section_state = match &first.kind {
                TaskEventKind::New => None,
                TaskEventKind::PollStart => Some(TaskState::IdleScheduled),
                TaskEventKind::PollEnd => Some(TaskState::Active),
                TaskEventKind::Drop => Some(TaskState::Idle),
                TaskEventKind::Wake => {
                    if let Some(second) = task_events.get(1) {
                        if let TaskEventKind::PollEnd = second.kind {
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

        for curr_idx in 1..task_events.len() {
            let current = &task_events[curr_idx];
            let prev = &task_events[curr_idx - 1];
            use TaskEventKind::{Drop, New, PollEnd, PollStart, Wake};

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
                    Wake => Section::New(TaskState::ActiveSchedueld),
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

            match section {
                Section::New(state) => {
                    task_sections.push(TaskSection {
                        duration: current.ts.saturating_sub(&prev.ts),
                        state,
                    });
                }
                Section::ReplaceWith {
                    replace_last_n_sections,
                    new_state,
                } => {
                    // TODO(hds): should probably emit a warning if this would be less than 2.
                    task_sections
                        .truncate(task_sections.len().saturating_sub(replace_last_n_sections));
                    task_sections.push(TaskSection {
                        duration: current.ts.saturating_sub(&prev.ts),
                        state: new_state,
                    });
                }
                Section::ExtendLast => {
                    if let Some(last) = task_sections.last_mut() {
                        last.duration += current.ts.saturating_sub(&prev.ts);
                    } else {
                        // Report error (this state shouldn't occur)
                    }
                }
                Section::Invalid { .. } => {
                    // Report warning and then continue
                }
            }
        }

        println!("\n======== {task:?} ========");
        println!("task_events: {task_events:?}");
        println!("wake_events: {wake_events:?}");
        println!("spawn_event: {spawn_event:?}");
        println!("task_sections: {task_sections:?}");
        println!("======== ======== ======== ========");

        task_rows.push(TaskRow {
            index,
            start_time,
            task,
            spawn: spawn_event,
            sections: task_sections,
            wakings: wake_events,
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
        from: TaskEventKind,
        #[allow(dead_code)]
        to: TaskEventKind,
    },
}
