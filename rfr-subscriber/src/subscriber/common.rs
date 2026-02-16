use std::{error, fmt, ptr};

use rfr::{
    chunked::{Callsite, CallsiteId},
    common::{Field, FieldName, FieldValue, InstrumentationId},
};
use tracing::{
    Level, Metadata, Subscriber,
    field::{self, Visit},
    span,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan};

#[derive(Clone)]
pub(super) enum TraceKind {
    Span(SpanKind),
    Event(EventKind),
}

#[derive(Clone)]
pub(super) enum SpanKind {
    Spawn,
    Resource,
    AsyncOp,
    AsyncOpPoll,
}

#[derive(Clone)]
pub(super) enum EventKind {
    Waker,
    PollOp,
    ResourceStateUpdate,
    AsyncOpUpdate,
}

impl From<SpanKind> for TraceKind {
    fn from(value: SpanKind) -> Self {
        Self::Span(value)
    }
}

impl From<EventKind> for TraceKind {
    fn from(value: EventKind) -> Self {
        Self::Event(value)
    }
}

impl TryFrom<&Metadata<'_>> for TraceKind {
    type Error = TryFromMetadataError;

    fn try_from(metadata: &Metadata<'_>) -> Result<Self, Self::Error> {
        if metadata.is_span() {
            Ok(match (metadata.name(), metadata.target()) {
                ("runtime.spawn", _) | ("task", "tokio::task") => SpanKind::Spawn,
                ("runtime.resource", _) => SpanKind::Resource,
                ("runtime.resource.async_op", _) => SpanKind::AsyncOp,
                ("runtime.resource.async_op.poll", _) => SpanKind::AsyncOpPoll,
                _ => {
                    return Err(TryFromMetadataError {
                        desc: "span metadata isn't interesting",
                    });
                }
            }
            .into())
        } else if metadata.is_event() {
            Ok(match metadata.target() {
                "runtime::waker" | "tokio::task::waker" => EventKind::Waker,
                "runtime::resource::poll_op" => EventKind::PollOp,
                "runtime::resource::state_update" => EventKind::ResourceStateUpdate,
                "runtime::resource::async_op::state_update" => EventKind::AsyncOpUpdate,
                _ => {
                    return Err(TryFromMetadataError {
                        desc: "event metadata isn't interesting",
                    });
                }
            }
            .into())
        } else {
            Err(TryFromMetadataError {
                desc: "metadata is not span or event, we don't want it",
            })
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct TryFromMetadataError {
    desc: &'static str,
}
impl fmt::Display for TryFromMetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.desc)
    }
}
impl error::Error for TryFromMetadataError {}

pub(super) fn to_callsite(metadata: &Metadata<'_>) -> Callsite {
    let mut const_fields = vec![
        Field {
            name: FieldName("name".into()),
            value: FieldValue::Str(metadata.name().to_string()),
        },
        Field {
            name: FieldName("target".into()),
            value: FieldValue::Str(metadata.name().to_string()),
        },
    ];
    if let Some(module_path) = metadata.module_path() {
        const_fields.push(Field {
            name: FieldName("module_path".into()),
            value: FieldValue::Str(module_path.to_string()),
        });
    }
    if let Some(module_pathfile) = metadata.file() {
        const_fields.push(Field {
            name: FieldName("file".into()),
            value: FieldValue::Str(module_pathfile.to_string()),
        });
    }
    if let Some(line) = metadata.line() {
        const_fields.push(Field {
            name: FieldName("line".into()),
            value: FieldValue::U64(line as u64),
        });
    }

    let mut split_field_names = Vec::new();
    for field in metadata.fields() {
        split_field_names.push(FieldName(field.name().into()));
    }
    Callsite {
        callsite_id: to_callsite_id(metadata),
        level: rfr::common::Level(match *metadata.level() {
            Level::ERROR => 50,
            Level::WARN => 40,
            Level::INFO => 30,
            Level::DEBUG => 20,
            Level::TRACE => 10,
        }),
        kind: if metadata.is_span() {
            rfr::common::Kind::Span
        } else {
            rfr::common::Kind::Event
        },
        const_fields,
        split_field_names,
    }
}

pub(super) fn to_callsite_id(metadata: &Metadata<'_>) -> CallsiteId {
    CallsiteId::from(ptr::from_ref(metadata) as u64)
}

pub(super) fn to_iid(span_id: &span::Id) -> InstrumentationId {
    InstrumentationId::from(span_id.into_u64())
}

pub(crate) fn get_context_task_iid<S>(ctx: &Context<'_, S>) -> Option<InstrumentationId>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let current_span = &ctx.current_span();
    Some(to_iid(current_span.id()?))
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct TaskId(pub(crate) u64);

#[derive(Debug)]
pub(crate) struct SpawnSpan {
    pub(crate) task_id: TaskId,
    pub(crate) task_name: String,
    pub(crate) task_kind: TaskKind,

    pub(crate) context: Option<InstrumentationId>,

    pub(crate) callsite_id: CallsiteId,
    pub(crate) iid: InstrumentationId,
}

impl SpawnSpan {
    pub(crate) fn new(
        callsite_id: CallsiteId,
        span_id: span::Id,
        context: Option<InstrumentationId>,
        fields: SpawnFields,
    ) -> Self {
        debug_assert!(fields.is_valid(), "invalid fields passed to SpawnSpan::new");
        Self {
            task_id: fields.task_id.unwrap(),
            task_name: fields.task_name.unwrap_or_default(),
            task_kind: fields.task_kind.unwrap(),

            context,

            callsite_id,
            iid: to_iid(&span_id),
        }
    }
}

#[derive(Debug)]
pub(crate) enum TaskKind {
    Task,
    Local,
    Blocking,
    BlockOn,
    Other(String),
}

impl From<String> for TaskKind {
    fn from(value: String) -> Self {
        match value.as_str() {
            "task" => Self::Task,
            "local" => Self::Local,
            "blocking" => Self::Blocking,
            "block_on" => Self::BlockOn,
            _ => Self::Other(value),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SpawnFields {
    task_id: Option<TaskId>,
    task_name: Option<String>,
    task_kind: Option<TaskKind>,
}

impl SpawnFields {
    const TASK_ID: &'static str = "task.id";
    const TASK_NAME: &'static str = "task.name";
    const KIND: &'static str = "kind";

    pub(crate) fn is_valid(&self) -> bool {
        self.task_id.is_some() && self.task_kind.is_some()
    }
}

impl Visit for SpawnFields {
    fn record_debug(&mut self, field: &field::Field, value: &dyn fmt::Debug) {
        match field.name() {
            Self::TASK_NAME => self.task_name = Some(format!("{value:?}")),
            Self::KIND => self.task_kind = Some(format!("{value:?}").into()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &field::Field, value: u64) {
        if field.name() == Self::TASK_ID {
            self.task_id = Some(TaskId(value));
        }
    }
}

#[derive(Debug)]
pub(crate) enum WakerOp {
    Wake,
    WakeByRef,
    Clone,
    Drop,
}

impl<'a> TryFrom<&'a str> for WakerOp {
    type Error = InvalidWakerOpError<'a>;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "waker.wake" => Ok(Self::Wake),
            "waker.wake_by_ref" => Ok(Self::WakeByRef),
            "waker.clone" => Ok(Self::Clone),
            "waker.drop" => Ok(Self::Drop),
            other => Err(InvalidWakerOpError { value: other }),
        }
    }
}

#[derive(Debug)]
pub(crate) struct InvalidWakerOpError<'a> {
    value: &'a str,
}
impl fmt::Display for InvalidWakerOpError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid waker op value: {}", self.value)
    }
}
impl error::Error for InvalidWakerOpError<'_> {}

#[derive(Debug, Default)]
pub(crate) struct WakerFields {
    pub(crate) op: Option<WakerOp>,
    pub(crate) task_span_id: Option<span::Id>,
}

impl WakerFields {
    const OP: &'static str = "op";
    const TASK_ID: &'static str = "task.id";
}

impl WakerFields {
    pub(crate) fn is_valid(&self) -> bool {
        self.op.is_some() && self.task_span_id.is_some()
    }
}

impl Visit for WakerFields {
    fn record_debug(&mut self, _field: &field::Field, _value: &dyn fmt::Debug) {}

    fn record_u64(&mut self, field: &field::Field, value: u64) {
        if field.name() == Self::TASK_ID {
            self.task_span_id = Some(span::Id::from_u64(value));
        }
    }

    fn record_str(&mut self, field: &field::Field, value: &str) {
        if field.name() == Self::OP {
            self.op = value.try_into().ok();
        }
    }
}
