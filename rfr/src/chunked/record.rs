use serde::{Deserialize, Serialize};

use crate::{Event, InstrumentationId, Waker, chunked::ChunkTimestamp};

/// A record containing timing metadata and record data.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Record {
    pub meta: Meta,
    pub data: RecordData,
}

/// Metadata for a [`Record`].
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Meta {
    /// The timestamp that the record occurs at.
    pub timestamp: ChunkTimestamp,
}

/// The data for a discrete
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum RecordData {
    SpanNew { iid: InstrumentationId },
    SpanEnter { iid: InstrumentationId },
    SpanExit { iid: InstrumentationId },
    SpanClose { iid: InstrumentationId },
    Event { event: Event },
    TaskNew { iid: InstrumentationId },
    TaskPollStart { iid: InstrumentationId },
    TaskPollEnd { iid: InstrumentationId },
    TaskDrop { iid: InstrumentationId },
    WakerWake { waker: Waker },
    WakerWakeByRef { waker: Waker },
    WakerClone { waker: Waker },
    WakerDrop { waker: Waker },
}
