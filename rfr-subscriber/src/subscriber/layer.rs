use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex},
};

use tracing::{span, subscriber::Interest, Dispatch, Event, Metadata, Subscriber};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

use rfr::{
    chunked::CallsiteId,
    common,
    rec::{self, StreamWriter},
};

use crate::subscriber::common::{
    get_context_task_iid, to_callsite_id, to_iid, EventKind, SpanKind, SpawnFields, SpawnSpan,
    TaskId, TaskKind, TraceKind, WakerFields, WakerOp,
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
                let callsite_id = to_callsite_id(metadata);
                let mut callsite_cache = self
                    .callsite_cache
                    .lock()
                    .expect("callsite cache is poisoned");
                callsite_cache.entry(callsite_id).or_insert(kind);

                Interest::always()
            }
            Err(_) => Interest::never(),
        }
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let rec_meta = rec::Meta::now();
        let callsite_id = to_callsite_id(attrs.metadata());
        let kind = {
            let callsite_cache = self.callsite_cache.lock().expect("callsite cache poisoned");
            let Some(kind) = callsite_cache.get(&callsite_id).cloned() else {
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
                let context = get_context_task_iid(&ctx);

                let spawn = SpawnSpan::new(callsite_id, id.clone(), context, fields);

                let span = ctx
                    .span(id)
                    .expect("new_span {id:?} not found, this is a bug");
                let mut extensions = span.extensions_mut();
                if extensions.get_mut::<TaskId>().is_none() {
                    extensions.insert(spawn.task_id);
                }
                {
                    let mut guard = self.writer.lock().unwrap();
                    let task_id = common::TaskId::from(spawn.task_id.0);
                    let task_data = rec::RecordData::Task {
                        task: common::Task {
                            iid: spawn.iid,
                            callsite_id: spawn.callsite_id,
                            task_id,
                            task_name: spawn.task_name,
                            task_kind: match spawn.task_kind {
                                TaskKind::Task => common::TaskKind::Task,
                                TaskKind::Local => common::TaskKind::Local,
                                TaskKind::Blocking => common::TaskKind::Blocking,
                                TaskKind::BlockOn => common::TaskKind::BlockOn,
                                TaskKind::Other(val) => common::TaskKind::Other(val),
                            },

                            context: spawn.context,
                        },
                    };
                    let task_new = rec::RecordData::TaskNew { iid: spawn.iid };

                    (*guard).write_record(rec::Record::new(rec_meta.clone(), task_data));
                    (*guard).write_record(rec::Record::new(rec_meta, task_new));
                }
            }
            _ => {
                // Not yet implemented
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let rec_meta = rec::Meta::now();
        let callsite_id = to_callsite_id(event.metadata());
        let kind = {
            let callsite_cache = self.callsite_cache.lock().expect("callsite cache poisoned");
            let Some(kind) = callsite_cache.get(&callsite_id).cloned() else {
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

                let mut guard = self.writer.lock().unwrap();
                let waker = common::Waker {
                    task_iid: to_iid(&task_span_id),
                    context: ctx.current_span().id().map(to_iid),
                };
                let waker_data = match op {
                    WakerOp::Wake => rec::RecordData::WakerWake { waker },
                    WakerOp::WakeByRef => rec::RecordData::WakerWakeByRef { waker },
                    WakerOp::Clone => rec::RecordData::WakerClone { waker },
                    WakerOp::Drop => rec::RecordData::WakerDrop { waker },
                };

                (*guard).write_record(rec::Record::new(rec_meta, waker_data));
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
        if extensions.get::<TaskId>().is_some() {
            // This is a runtime.spawn span
            let mut guard = self.writer.lock().unwrap();
            let poll_start = rec::RecordData::TaskPollStart { iid: to_iid(id) };

            guard.write_record(rec::Record::new(rec_meta, poll_start));
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let rec_meta = rec::Meta::now();
        let span = ctx.span(id).expect("exit {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if extensions.get::<TaskId>().is_some() {
            // This is a runtime.spawn span
            let mut guard = self.writer.lock().unwrap();
            let poll_end = rec::RecordData::TaskPollEnd { iid: to_iid(id) };

            (*guard).write_record(rec::Record::new(rec_meta, poll_end));
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let rec_meta = rec::Meta::now();
        let span = ctx
            .span(&id)
            .expect("close {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if extensions.get::<TaskId>().is_some() {
            // This is a runtime.spawn span
            let mut guard = self.writer.lock().unwrap();
            let task_drop = rec::RecordData::TaskDrop { iid: to_iid(&id) };

            (*guard).write_record(rec::Record::new(rec_meta, task_drop));
        }
    }
}
