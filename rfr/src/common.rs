use serde::{Deserialize, Serialize};

use crate::chunked::CallsiteId;

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
pub struct FieldName(pub String);

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
pub struct InstrumentationId(u64);

impl From<u64> for InstrumentationId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl InstrumentationId {
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Span {
    iid: InstrumentationId,
    callsite_id: CallsiteId,
    parent: Parent,
    split_field_values: Vec<FieldValue>,
    dynamic_fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Parent {
    Current,
    Root,
    Explicit { iid: InstrumentationId },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Event {
    pub callsite_id: CallsiteId,
    pub parent: Parent,
    pub split_field_values: Vec<FieldValue>,
    pub dynamic_fields: Vec<Field>,
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
    pub iid: InstrumentationId,
    pub callsite_id: CallsiteId,
    pub task_id: TaskId,
    pub task_name: String,
    pub task_kind: TaskKind,

    pub context: Option<InstrumentationId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Waker {
    pub task_iid: InstrumentationId,
    pub context: Option<InstrumentationId>,
}
