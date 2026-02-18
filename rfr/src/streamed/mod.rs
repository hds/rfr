use serde::{Deserialize, Serialize};

use crate::{
    AbsTimestamp, Callsite, Event, FormatIdentifier, FormatVariant, InstrumentationId, Span, Task,
    Waker,
};

mod read;
mod write;

pub use read::from_file;
pub use write::StreamWriter;

fn current_software_version() -> FormatIdentifier {
    FormatIdentifier {
        variant: FormatVariant::RfrStreaming,
        major: 0,
        minor: 0,
        patch: 2,
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Meta {
    pub timestamp: AbsTimestamp,
}

impl Meta {
    pub fn now() -> Self {
        Self {
            timestamp: AbsTimestamp::now(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Record {
    pub meta: Meta,
    pub data: RecordData,
}

impl Record {
    pub fn new(meta: Meta, data: RecordData) -> Self {
        Self { meta, data }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub enum RecordData {
    End,
    Callsite { callsite: Callsite },
    Span { span: Span },
    Event { event: Event },
    Task { task: Task },
    SpanNew { iid: InstrumentationId },
    SpanEnter { iid: InstrumentationId },
    SpanExit { iid: InstrumentationId },
    SpanClose { iid: InstrumentationId },
    TaskNew { iid: InstrumentationId },
    TaskPollStart { iid: InstrumentationId },
    TaskPollEnd { iid: InstrumentationId },
    TaskDrop { iid: InstrumentationId },
    WakerWake { waker: Waker },
    WakerWakeByRef { waker: Waker },
    WakerClone { waker: Waker },
    WakerDrop { waker: Waker },
}
