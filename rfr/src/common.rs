use serde::{Deserialize, Serialize};

/// The level of a span or event.
///
/// The `tracing` levels are mapped as per the [Bunyan level suggestions].
///
/// | Value | Level |
/// |-------|-------|
/// | 10    | trace |
/// | 20    | debug |
/// | 30    | info  |
/// | 40    | warn  |
/// | 50    | error |
///
/// [Bunyan level suggestions]: https://github.com/trentm/node-bunyan?tab=readme-ov-file#levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct Level(pub u8);

/// The kind of a callsite.
///
/// This indicates the type of instrumentation that is emitted from that location in the
/// application code.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Kind {
    /// An unknown instrumentation type.
    Unknown,
    /// An event representing a point in time.
    Event,
    /// A span representing potentially disjoint periods of time.
    Span,
}

/// A complete field containing the name and value together.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Field {
    pub name: FieldName,
    pub value: FieldValue,
}

/// The name (key) of a field.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FieldName(String);

/// The value of a field.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum FieldValue {
    /// 64-bit floating point number.
    F64(f64),
    /// 64-bit signed integer.
    I64(i64),
    /// 64-bit unsigned integer.
    U64(u64),
    /// 128-bit signed integer.
    I128(i128),
    /// 128-bit unsigned integer.
    U128(u128),
    /// Boolean
    Bool(bool),
    /// String
    Str(String),
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
