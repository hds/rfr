use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex},
};

use tracing::{span, subscriber::Interest, Dispatch, Event, Metadata, Subscriber};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

use rfr::rec::{self, StreamWriter};

use crate::subscriber::common::{
    get_context_task_id, CallsiteId, EventKind, SpanKind, SpawnFields, SpawnSpan, TaskId, TaskKind,
    TraceKind, WakerEvent, WakerFields, WakerOp,
};

pub struct RfrLayer {
    writer: Arc<Mutex<StreamWriter<fs::File>>>,
    callsite_cache: Mutex<HashMap<CallsiteId, TraceKind>>,
}

impl RfrLayer {
    pub fn new(file_prefix: &str) -> Self {
        let filename = format!("{prefix}-stream.rfr", prefix = file_prefix);

        let file = fs::File::create(filename).unwrap();
        let writer = Arc::new(Mutex::new(StreamWriter::new(file)));

        Self {
            writer,
            callsite_cache: Default::default(),
        }
    }

    pub fn flusher(&self) -> Flusher {
        Flusher {
            writer: Arc::clone(&self.writer),
        }
    }
}

pub struct Flusher {
    writer: Arc<Mutex<StreamWriter<fs::File>>>,
}

impl Flusher {
    pub fn flush(&self) -> std::io::Result<()> {
        let mut guard = self.writer.lock().expect("poisoned");
        guard.flush()
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
        let rec_meta = rec::Meta::now();
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
                {
                    let mut guard = self.writer.lock().unwrap();
                    let task_id = rec::TaskId::from(spawn.task_id.0);
                    let task_event = rec::Event::Task(rec::Task {
                        task_id,
                        task_name: spawn.task_name,
                        task_kind: match spawn.task_kind {
                            TaskKind::Task => rec::TaskKind::Task,
                            TaskKind::Local => rec::TaskKind::Local,
                            TaskKind::Blocking => rec::TaskKind::Blocking,
                            TaskKind::BlockOn => rec::TaskKind::BlockOn,
                            TaskKind::Other(val) => rec::TaskKind::Other(val),
                        },

                        context: spawn.context.map(|task_id| rec::TaskId::from(task_id.0)),
                    });
                    let new_event = rec::Event::NewTask { id: task_id };

                    (*guard).write_record(rec::Record::new(rec_meta.clone(), task_event));
                    (*guard).write_record(rec::Record::new(rec_meta, new_event));
                }
            }
            _ => {
                // Not yet implemented
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let rec_meta = rec::Meta::now();
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
                let Some(task_id) = ctx
                    .span(&task_span_id)
                    .and_then(|span| span.extensions().get().cloned())
                else {
                    // We can't find the task id for the task we're supposed to be waking.
                    return;
                };
                let context = get_context_task_id(&ctx);

                let waker = WakerEvent::new(op, task_id, context, callsite);
                {
                    let mut guard = self.writer.lock().unwrap();
                    let waker_action = rec::Event::WakerOp(rec::WakerAction {
                        op: match waker.op {
                            WakerOp::Wake => rec::WakerOp::Wake,
                            WakerOp::WakeByRef => rec::WakerOp::WakeByRef,
                            WakerOp::Clone => rec::WakerOp::Clone,
                            WakerOp::Drop => rec::WakerOp::Drop,
                        },
                        task_id: rec::TaskId::from(waker.task_id.0),
                        context: waker.context.map(|task_id| rec::TaskId::from(task_id.0)),
                    });

                    (*guard).write_record(rec::Record::new(rec_meta, waker_action));
                }
            }
            _ => {
                // Not yet implemented
            }
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let rec_meta = rec::Meta::now();
        let span = ctx.span(id).expect("enter {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if let Some(task_id) = extensions.get::<TaskId>() {
            // This is a runtime.spawn span
            {
                let mut guard = self.writer.lock().unwrap();
                let task_id = rec::TaskId::from(task_id.0);
                let poll_start = rec::Event::TaskPollStart { id: task_id };

                guard.write_record(rec::Record::new(rec_meta, poll_start));
            }
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let rec_meta = rec::Meta::now();
        let span = ctx.span(id).expect("exit {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if let Some(task_id) = extensions.get::<TaskId>() {
            // This is a runtime.spawn span
            {
                let mut guard = self.writer.lock().unwrap();
                let task_id = rec::TaskId::from(task_id.0);
                let poll_end = rec::Event::TaskPollEnd { id: task_id };

                (*guard).write_record(rec::Record::new(rec_meta, poll_end));
            }
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let rec_meta = rec::Meta::now();
        let span = ctx
            .span(&id)
            .expect("close {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if let Some(task_id) = extensions.get::<TaskId>() {
            // This is a runtime.spawn span
            {
                let mut guard = self.writer.lock().unwrap();
                let task_id = rec::TaskId::from(task_id.0);
                let task_drop = rec::Event::TaskDrop { id: task_id };

                (*guard).write_record(rec::Record::new(rec_meta, task_drop));
            }
        }
    }
}
