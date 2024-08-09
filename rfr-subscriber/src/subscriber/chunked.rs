use std::{cell::{Cell, RefCell}, collections::HashMap, sync::{Arc, Mutex}, thread::{self, JoinHandle}};

use tracing::{span, subscriber::Interest, Event, Metadata, Subscriber};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

use rfr::{chunked::{self, AbsTimestampSecs, ChunkedWriter}, rec};

use crate::subscriber::common::{
    get_context_task_id, CallsiteId, EventKind, SpanKind, SpawnFields, SpawnSpan, TaskId, TaskKind,
    TraceKind, WakerFields, WakerOp,
};

pub struct RfrChunkedLayer {
    //writer: Arc<Mutex<StreamWriter<fs::File>>>,
    writer_join_handle: Mutex<Option<JoinHandle<ChunkedWriter>>>,
    callsite_cache: Mutex<HashMap<CallsiteId, TraceKind>>,
    object_cache: Mutex<HashMap<rec::TaskId, chunked::Object>>,
}

impl RfrChunkedLayer {
    pub fn new(base_dir: &str) -> Self {
        let writer_join_handle = Self::spawn_writer(base_dir.to_owned());

        Self {
            writer_join_handle: Mutex::new(Some(writer_join_handle)),
            callsite_cache: Default::default(),
            object_cache: Default::default(),
        }
    }

    fn spawn_writer(base_dir: String) -> JoinHandle<ChunkedWriter> {
        let writer = ChunkedWriter::new(base_dir.to_owned());

        thread::Builder::new()
            .name("rfr-writer".to_owned())
            .spawn(move || {
                run_writer_loop(writer)
            }).unwrap()
    }

    pub fn complete(&self) {
        let join_handle = {
            let mut guard = self.writer_join_handle.lock().expect("poisoned");
            guard.take()
        };
        if let Some(join_handle) = join_handle {
            // TODO(hds): signal writer thread to stop
            join_handle.join().unwrap();
        } else {
            // Otherwise some other thread has joined on the writer.
        }
    }

    fn new_object(&self, task_id: rec::TaskId, object: chunked::Object) {
        let mut object_cache = self.object_cache.lock().expect("object cache poisoned");
        object_cache.insert(task_id, object);
    }

    fn drop_object(&self, task_id: &rec::TaskId) {
        let mut object_cache = self.object_cache.lock().expect("object cache poisoned");
        object_cache.remove(task_id);
    }

    fn get_objects(&self, task_ids: &[rec::TaskId]) -> Option<Vec<chunked::Object>> {
        let mut objects = Vec::with_capacity(task_ids.len());

        let object_cache = self.object_cache.lock().expect("object cache poisoned");
        for task_id in task_ids {
            objects.push(object_cache.get(task_id)?.clone());
        }

        Some(objects)
    }

    fn write_record(&self, timestamp: rec::AbsTimestamp, event: chunked::Event) {
        thread_local! {
            pub static CHUNK_BUFFER: RefCell<Option<Arc<chunked::ThreadChunkBuffer>>> = const { RefCell::new(None) };
        }

        let base_time = AbsTimestampSecs::from(timestamp);
        let mut chunk_buffer = CHUNK_BUFFER.borrow();
        CHUNK_BUFFER.with_borrow_mut(|chunk_buffer| {
            let current_buffer = match &chunk_buffer {
                Some(buffer) => {
                    match base_time.cmp(&buffer.base_time()) {
                        std::cmp::Ordering::Equal => {
                            // Stored chunk is the current one
                            buffer
                        }
                        std::cmp::Ordering::Greater => {
                            // Stored chunk is old, we need a new one
                            *chunk_buffer = Some(chunked::ThreadChunkBuffer::new(timestamp));
                            chunk_buffer
                        }
                        std::cmp::Ordering::Less => {
                            // Stored chunk is from the future? This is invalid!
                            panic!("Current base time is {base_time}, but stored base time is {buffer_base_time}, which is from the future!", buffer_base_time = buffer.base_time());
                        }

                    }
                }
                None => {
                    *chunk_buffer = Some(chunked::ThreadChunkBuffer::new(timestamp));
                    chunk_buffer
                }
            };

        });

//        if chunk_buffer.is_none() {
//            *chunk_buffer = Some(chunked::ThreadChunkBuffer::new(base_time.clone()));
//        }
        
        

    }
}

fn run_writer_loop(writer: ChunkedWriter) -> ChunkedWriter {

    writer
}

impl<S> Layer<S> for RfrChunkedLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
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
                    let task_id = rec::TaskId::from(spawn.task_id.0);
                    let task_event = chunked::Object::Task(rec::Task {
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
                    let new_event = chunked::Event::NewTask { id: task_id };
                    self.write_record(rec_meta.timestamp, new_event);
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
                    .and_then(|span| span.extensions().get::<TaskId>().cloned())
                else {
                    // We can't find the task id for the task we're supposed to be waking.
                    return;
                };
                let context = get_context_task_id(&ctx);

                //let waker = WakerEvent::new(op, task_id, context, callsite);
                {
                    let waker = chunked::Waker {
                        task_id: rec::TaskId::from(task_id.0),
                        context: context.map(|task_id| rec::TaskId::from(task_id.0)),
                    };
                    let waker_event = match op {
                        WakerOp::Wake => chunked::Event::WakerWake { waker },
                        WakerOp::WakeByRef => chunked::Event::WakerWakeByRef { waker },
                        WakerOp::Clone => chunked::Event::WakerClone { waker },
                        WakerOp::Drop => chunked::Event::WakerDrop { waker },
                    };

                    
                    self.write_record(rec_meta.timestamp, waker_event);
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
                let task_id = rec::TaskId::from(task_id.0);
                let poll_start = chunked::Event::TaskPollStart { id: task_id };

                self.write_record(rec_meta.timestamp, poll_start);
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
                let task_id = rec::TaskId::from(task_id.0);
                let poll_end = chunked::Event::TaskPollEnd { id: task_id };

                self.write_record(rec_meta.timestamp, poll_end);
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
                let task_id = rec::TaskId::from(task_id.0);
                let task_drop = chunked::Event::TaskDrop { id: task_id };

                self.write_record(rec_meta.timestamp, task_drop);
            }
        }
    }
}
