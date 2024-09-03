use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use tracing::{span, subscriber::Interest, Event, Metadata, Subscriber};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

use rfr::{
    chunked::{self, AbsTimestampSecs, ChunkedWriter},
    common, rec,
};

use crate::subscriber::common::{
    get_context_task_id, CallsiteId, EventKind, SpanKind, SpawnFields, SpawnSpan, TaskId, TaskKind,
    TraceKind, WakerFields, WakerOp,
};

struct WriterHandle {
    writer: Arc<ChunkedWriter>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

pub struct Flusher {
    writer: Arc<ChunkedWriter>,
}

impl Flusher {
    pub fn flush(&self) {
        self.writer.write();
    }
}

pub struct RfrChunkedLayer {
    writer_handle: WriterHandle,
    callsite_cache: Mutex<HashMap<CallsiteId, TraceKind>>,
    object_cache: Mutex<HashMap<TaskId, chunked::Object>>,
}

impl RfrChunkedLayer {
    pub fn new(base_dir: &str) -> Self {
        let writer_handle = Self::spawn_writer(base_dir.to_owned());

        Self {
            writer_handle,
            callsite_cache: Default::default(),
            object_cache: Default::default(),
        }
    }

    fn spawn_writer(base_dir: String) -> WriterHandle {
        let writer = Arc::new(ChunkedWriter::new(base_dir.to_owned()));

        let thread_writer = Arc::clone(&writer);
        let join_handle = thread::Builder::new()
            .name("rfr-writer".to_owned())
            .spawn(move || run_writer_loop(thread_writer))
            .unwrap();

        WriterHandle {
            writer,
            join_handle: Mutex::new(Some(join_handle)),
        }
    }

    pub fn flusher(&self) -> Flusher {
        Flusher {
            writer: Arc::clone(&self.writer_handle.writer),
        }
    }

    pub fn complete(&self) {
        let join_handle = {
            let mut guard = self.writer_handle.join_handle.lock().expect("poisoned");
            guard.take()
        };
        if let Some(join_handle) = join_handle {
            // TODO(hds): signal writer thread to stop
            join_handle.join().unwrap();

            self.writer_handle.writer.write();
        } else {
            // Otherwise some other thread has joined on the writer.
        }
    }

    fn new_object(&self, task_id: TaskId, object: chunked::Object) {
        let mut object_cache = self.object_cache.lock().expect("object cache poisoned");
        object_cache.insert(task_id, object);
    }

    fn drop_object(&self, task_id: &TaskId) {
        let mut object_cache = self.object_cache.lock().expect("object cache poisoned");
        object_cache.remove(task_id);
    }

    fn get_objects(&self, task_ids: &[common::TaskId]) -> Vec<Option<chunked::Object>> {
        let object_cache = self.object_cache.lock().expect("object cache poisoned");
        task_ids
            .iter()
            .map(|task_id| object_cache.get(&TaskId(task_id.as_u64())).cloned())
            .collect()
    }

    fn write_record(&self, timestamp: rec::AbsTimestamp, event: common::Event) {
        thread_local! {
            pub static CHUNK_BUFFER: RefCell<Option<Arc<chunked::ThreadChunkBuffer>>>
                = const { RefCell::new(None) };
        }

        CHUNK_BUFFER.with_borrow_mut(|chunk_buffer| {
            let current_buffer = self.current_chunk_buffer(chunk_buffer, timestamp.clone());
            let record = chunked::EventRecord {
                meta: chunked::Meta {
                    timestamp: current_buffer.chunk_timestamp(timestamp),
                },
                event,
            };
            current_buffer.append_record(record, |task_ids| self.get_objects(task_ids));
        });
    }

    fn current_chunk_buffer<'a>(
        &self,
        local_buffer: &'a mut Option<Arc<chunked::ThreadChunkBuffer>>,
        timestamp: rec::AbsTimestamp,
    ) -> &'a mut Arc<chunked::ThreadChunkBuffer> {
        let base_time = AbsTimestampSecs::from(timestamp.clone());
        let buffer = local_buffer.get_or_insert_with(|| self.new_chunk(timestamp.clone()));

        match base_time.cmp(&buffer.base_time()) {
            std::cmp::Ordering::Equal => {
                // Stored chunk is the current one, do nothing
            }
            std::cmp::Ordering::Greater => {
                // Stored chunk is old, we need a new one
                *buffer = self.new_chunk(timestamp.clone());
            }
            std::cmp::Ordering::Less => {
                // Stored chunk is from the future? This is invalid!
                panic!(
                    "Current base time is {base_time:?}, but stored base time is \
                    {buffer_base_time:?}, which is from the future!",
                    buffer_base_time = buffer.base_time()
                );
            }
        }

        buffer
    }

    fn new_chunk(&self, timestamp: rec::AbsTimestamp) -> Arc<chunked::ThreadChunkBuffer> {
        let new_chunk = Arc::new(chunked::ThreadChunkBuffer::new(timestamp));
        self.writer_handle
            .writer
            .register_chunk(Arc::clone(&new_chunk));

        new_chunk
    }
}

fn run_writer_loop(writer: Arc<ChunkedWriter>) {
    _ = writer;
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
                    let task_id = common::TaskId::from(spawn.task_id.0);
                    let task_event = chunked::Object::Task(common::Task {
                        task_id,
                        task_name: spawn.task_name,
                        task_kind: match spawn.task_kind {
                            TaskKind::Task => common::TaskKind::Task,
                            TaskKind::Local => common::TaskKind::Local,
                            TaskKind::Blocking => common::TaskKind::Blocking,
                            TaskKind::BlockOn => common::TaskKind::BlockOn,
                            TaskKind::Other(val) => common::TaskKind::Other(val),
                        },

                        context: spawn.context.map(|task_id| common::TaskId::from(task_id.0)),
                    });
                    self.new_object(spawn.task_id, task_event);
                    let new_event = common::Event::NewTask { id: task_id };
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
                    let waker = common::Waker {
                        task_id: common::TaskId::from(task_id.0),
                        context: context.map(|task_id| common::TaskId::from(task_id.0)),
                    };
                    let waker_event = match op {
                        WakerOp::Wake => common::Event::WakerWake { waker },
                        WakerOp::WakeByRef => common::Event::WakerWakeByRef { waker },
                        WakerOp::Clone => common::Event::WakerClone { waker },
                        WakerOp::Drop => common::Event::WakerDrop { waker },
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
                let task_id = common::TaskId::from(task_id.0);
                let poll_start = common::Event::TaskPollStart { id: task_id };

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
                let task_id = common::TaskId::from(task_id.0);
                let poll_end = common::Event::TaskPollEnd { id: task_id };

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
                let task_drop = common::Event::TaskDrop {
                    id: common::TaskId::from(task_id.0),
                };

                self.write_record(rec_meta.timestamp, task_drop);
                self.drop_object(&task_id);
            }
        }
    }
}
