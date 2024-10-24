use std::{collections::HashMap, convert::identity, fmt, ops::Add, time::Duration};

use rfr::{
    chunked,
    common::{Event, Task, TaskId, Waker},
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
    pub(crate) event: Event,
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
            for events in &seq_chunk.events {
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
                        .entry(task.task_id)
                        .or_insert_with(|| TaskEvents::new(task.clone()));
                }
            }

            for record in &seq_chunk.events {
                let task_id = match &record.event {
                    Event::NewTask { id }
                    | Event::TaskPollStart { id }
                    | Event::TaskPollEnd { id }
                    | Event::TaskDrop { id } => id,
                    Event::WakerWake { waker }
                    | Event::WakerWakeByRef { waker }
                    | Event::WakerClone { waker }
                    | Event::WakerDrop { waker } => &waker.task_id,
                    _ => continue,
                };

                let record = EventRecord {
                    timestamp: chunk.abs_timestamp(&record.meta.timestamp),
                    event: record.event.clone(),
                };

                tasks.entry(*task_id).and_modify(|r| r.events.push(record));
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
        if let rec::Event::Task(task) = &record.event {
            let task_entry = TaskEvents::new(task.clone());
            tasks.insert(task.task_id, task_entry);
        } else {
            let (event, task_id) = match &record.event {
                rec::Event::NewTask { id } => (Event::NewTask { id: *id }, *id),
                rec::Event::TaskPollStart { id } => (Event::TaskPollStart { id: *id }, *id),
                rec::Event::TaskPollEnd { id } => (Event::TaskPollEnd { id: *id }, *id),
                rec::Event::TaskDrop { id } => (Event::TaskDrop { id: *id }, *id),
                rec::Event::WakerOp(action) => {
                    let waker = Waker {
                        task_id: action.task_id,
                        context: action.context,
                    };
                    let event = match action.op {
                        rec::WakerOp::Wake => Event::WakerWake { waker },
                        rec::WakerOp::WakeByRef => Event::WakerWakeByRef { waker },
                        rec::WakerOp::Clone => Event::WakerClone { waker },
                        rec::WakerOp::Drop => Event::WakerDrop { waker },
                    };
                    (event, action.task_id)
                }
                rec::Event::Task(_) => {
                    unreachable!("task events have already been filtered out")
                }
            };
            let record = EventRecord {
                timestamp: record.meta.timestamp.clone(),
                event,
            };
            tasks.entry(task_id).and_modify(|r| r.events.push(record));
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
    tasks_events.sort_by_key(|t| t.task.task_id);

    let tasks_with_indicies: Vec<_> = tasks_events
        .into_iter()
        .enumerate()
        .map(|(idx, task_events)| (TaskIndex::new(idx), task_events))
        .collect();
    let task_indices: HashMap<_, _> = tasks_with_indicies
        .iter()
        .map(|(idx, task_events)| (task_events.task.task_id, *idx))
        .collect();
    let get_index = |task_id: Option<TaskId>| task_id.and_then(|id| task_indices.get(&id).copied());

    let mut task_rows = Vec::new();
    for (index, TaskEvents { task, events }) in tasks_with_indicies {
        if events.is_empty() {
            continue;
        }

        let first = &events.first().expect("events is not empty");
        let start_time = if let Event::NewTask { .. } = &first.event {
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

            match &rec.event {
                Event::NewTask { .. } => {
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
                Event::TaskPollStart { .. } => task_events.push(TaskEvent {
                    ts,
                    kind: TaskEventKind::PollStart,
                }),
                Event::TaskPollEnd { .. } => task_events.push(TaskEvent {
                    ts,
                    kind: TaskEventKind::PollEnd,
                }),
                Event::TaskDrop { .. } => task_events.push(TaskEvent {
                    ts,
                    kind: TaskEventKind::Drop,
                }),
                Event::WakerWake { waker } => {
                    task_events.push(TaskEvent {
                        ts: ts.clone(),
                        kind: TaskEventKind::Wake,
                    });

                    let kind = if Some(waker.task_id) == waker.context {
                        WakeEventKind::SelfWake
                    } else {
                        WakeEventKind::Wake {
                            by: get_index(waker.context),
                        }
                    };

                    wake_events.push(WakeEvent { ts, kind });
                }
                Event::WakerWakeByRef { waker } => {
                    task_events.push(TaskEvent {
                        ts: ts.clone(),
                        kind: TaskEventKind::Wake,
                    });

                    let kind = if Some(waker.task_id) == waker.context {
                        WakeEventKind::SelfWakeByRef
                    } else {
                        WakeEventKind::WakeByRef {
                            by: get_index(waker.context),
                        }
                    };

                    wake_events.push(WakeEvent { ts, kind });
                }
                Event::WakerClone { .. } => wake_events.push(WakeEvent {
                    ts,
                    kind: WakeEventKind::Clone,
                }),
                Event::WakerDrop { .. } => wake_events.push(WakeEvent {
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
