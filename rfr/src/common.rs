use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum TaskKind {
    Task,
    Local,
    Blocking,
    BlockOn,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Task {
    pub task_id: TaskId,
    pub task_name: String,
    pub task_kind: TaskKind,

    pub context: Option<TaskId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Waker {
    pub task_id: TaskId,
    pub context: Option<TaskId>,
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
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
