use std::{collections::HashMap, error, fmt, fs, io::Write, sync::Mutex};

use tracing::{
    field::{Field, Visit},
    span,
    subscriber::Interest,
    Dispatch, Event, Metadata, Subscriber,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

#[derive(Clone)]
enum TraceKind {
    Span(SpanKind),
    Event(EventKind),
}

#[derive(Clone)]
enum SpanKind {
    Spawn,
    Resource,
    AsyncOp,
    AsyncOpPoll,
}

#[derive(Clone)]
enum EventKind {
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
                    })
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
                    })
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
struct TryFromMetadataError {
    desc: &'static str,
}
impl fmt::Display for TryFromMetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.desc)
    }
}
impl error::Error for TryFromMetadataError {}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct CallsiteId(u64);

impl From<&Metadata<'_>> for CallsiteId {
    fn from(metadata: &Metadata<'_>) -> Self {
        Self(std::ptr::from_ref(metadata) as u64)
    }
}

pub struct RfrLayer {
    file_prefix: String,
    current_file: Option<Mutex<fs::File>>,
    callsite_cache: Mutex<HashMap<CallsiteId, TraceKind>>,
}

impl Default for RfrLayer {
    fn default() -> Self {
        Self {
            file_prefix: "./recording-".into(),
            current_file: Default::default(),
            callsite_cache: Default::default(),
        }
    }
}

impl RfrLayer {
    pub fn new() -> Self {
        let mut layer = Self::default();
        let filename = format!("{prefix}0", prefix = layer.file_prefix);

        let file = fs::File::create(filename).unwrap();
        layer.current_file = Some(Mutex::new(file));

        layer
    }
}

impl<S> Layer<S> for RfrLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_register_dispatch(&self, subscriber: &Dispatch) {
        let _ = subscriber;
    }

    fn on_layer(&mut self, subscriber: &mut S) {
        let _ = subscriber;
    }

    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        match TraceKind::try_from(metadata) {
            Ok(kind) => {
                let callsite = CallsiteId::from(metadata);
                let mut callsite_cache = self
                    .callsite_cache
                    .lock()
                    .expect("callsite cache is poisoned");
                callsite_cache.entry(callsite).or_insert(kind);

                Interest::always()
            }
            Err(_) => Interest::never(),
        }
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let callsite = CallsiteId::from(attrs.metadata());
        let kind = {
            let callsite_cache = self.callsite_cache.lock().expect("callsite cache poisoned");
            let Some(kind) = callsite_cache.get(&callsite).cloned() else {
                return;
            };
            kind
        };
        match kind {
            TraceKind::Span(SpanKind::Spawn) => {
                let mut fields = SpawnFields::default();
                attrs.record(&mut fields);
                if !fields.is_valid() {
                    return;
                }
                let context = get_context_task_id(&ctx);

                let spawn = SpawnSpan::new(callsite, id.clone(), context, fields);

                let span = ctx
                    .span(id)
                    .expect("new_span {id:?} not found, this is a bug");
                let mut extensions = span.extensions_mut();
                if extensions.get_mut::<TaskId>().is_none() {
                    extensions.insert(spawn.task_id);
                }
                if let Some(file) = &self.current_file {
                    let mut file = file.lock().expect("current file poisoned");
                    writeln!(file, "  new {spawn:?}").expect("write to file failed");
                }
            }
            _ => {
                // Not yet implemented
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let callsite = CallsiteId::from(event.metadata());
        let kind = {
            let callsite_cache = self.callsite_cache.lock().expect("callsite cache poisoned");
            let Some(kind) = callsite_cache.get(&callsite).cloned() else {
                return;
            };
            kind
        };
        match kind {
            TraceKind::Event(EventKind::Waker) => {
                let mut fields = WakerFields::default();
                event.record(&mut fields);
                if !fields.is_valid() {
                    return;
                }
                let op = fields.op.unwrap();
                let task_span_id = fields.task_span_id.unwrap();
                let Some(task_id) = ctx.span(&task_span_id).and_then(|span| span.extensions().get().cloned()) else  { 
                    // We can't find the task id for the task we're supposed to be waking.
                    return; };
                let context = get_context_task_id(&ctx);

                let waker = WakerEvent::new(op, task_id, context, callsite);
                if let Some(file) = &self.current_file {
                    let mut file = file.lock().expect("current file poisoned");
                    writeln!(file, "  new {waker:?}").expect("write to file failed");
                }
            }
            _ => {
                // Not yet implemented
            }
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("enter {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if let Some(task_id) = extensions.get::<TaskId>() {
            // This is a runtime.spawn span
            if let Some(file) = &self.current_file {
                let mut file = file.lock().expect("current file poisoned");
                writeln!(file, "enter {task_id:?}").expect("write to file failed");
            }
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("exit {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if let Some(task_id) = extensions.get::<TaskId>() {
            // This is a runtime.spawn span
            if let Some(file) = &self.current_file {
                let mut file = file.lock().expect("current file poisoned");
                writeln!(file, " exit {task_id:?}").expect("write to file failed");
            }
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx
            .span(&id)
            .expect("close {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if let Some(task_id) = extensions.get::<TaskId>() {
            // This is a runtime.spawn span
            if let Some(file) = &self.current_file {
                let mut file = file.lock().expect("current file poisoned");
                writeln!(file, "close {task_id:?}").expect("write to file failed");
            }
        }
    }
}

fn get_context_task_id<S>(ctx: &Context<'_, S>) -> Option<TaskId>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let current_span = &ctx.current_span();
    let span_id = current_span.id()?;
    ctx.span(span_id)?.extensions().get().cloned()
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct TaskId(u64);

#[derive(Debug)]
struct SpawnSpan {
    task_id: TaskId,
    task_name: String,
    task_kind: TaskKind,

    context: Option<TaskId>,

    callsite: CallsiteId,
    span: span::Id,
}

impl SpawnSpan {
    fn new(
        callsite: CallsiteId,
        span_id: span::Id,
        context: Option<TaskId>,
        fields: SpawnFields,
    ) -> Self {
        debug_assert!(fields.is_valid(), "invalid fields passed to SpawnSpan::new");
        Self {
            task_id: fields.task_id.unwrap(),
            task_name: fields.task_name.unwrap_or_default(),
            task_kind: fields.task_kind.unwrap(),

            context,

            callsite,
            span: span_id,
        }
    }
}

#[derive(Debug)]
enum TaskKind {
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
struct SpawnFields {
    task_id: Option<TaskId>,
    task_name: Option<String>,
    task_kind: Option<TaskKind>,
}

impl SpawnFields {
    const TASK_ID: &'static str = "task.id";
    const TASK_NAME: &'static str = "task.name";
    const KIND: &'static str = "kind";

    fn is_valid(&self) -> bool {
        self.task_id.is_some() && self.task_kind.is_some()
    }
}

impl Visit for SpawnFields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            Self::TASK_NAME => self.task_name = Some(format!("{value:?}")),
            Self::KIND => self.task_kind = Some(format!("{value:?}").into()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == Self::TASK_ID {
            self.task_id = Some(TaskId(value));
        }
    }
}

#[derive(Debug)]
enum WakerOp {
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
struct InvalidWakerOpError<'a> {
    value: &'a str,
}
impl fmt::Display for InvalidWakerOpError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid waker op value: {}", self.value)
    }
}
impl error::Error for InvalidWakerOpError<'_> {}

#[derive(Debug)]
struct WakerEvent {
    op: WakerOp,
    task_id: TaskId,

    context: Option<TaskId>,

    callsite: CallsiteId,
}

impl WakerEvent {
    fn new(
        op: WakerOp,
        task_id: TaskId,
        context: Option<TaskId>,
        callsite: CallsiteId,
    ) -> Self {
        Self {
            op,
            task_id,

            context,

            callsite,
        }
    }
}

#[derive(Debug, Default)]
struct WakerFields {
    op: Option<WakerOp>,
    task_span_id: Option<span::Id>,
}

impl WakerFields {
    const OP: &'static str = "op";
    const TASK_ID: &'static str = "task.id";
}

impl WakerFields {
    fn is_valid(&self) -> bool {
        self.op.is_some() && self.task_span_id.is_some()
    }
}


impl Visit for WakerFields {
    fn record_debug(&mut self, _field: &Field, _value: &dyn fmt::Debug) {}

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == Self::TASK_ID {
            self.task_span_id = Some(span::Id::from_u64(value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == Self::OP {
            self.op = value.try_into().ok();
        }
    }
}
