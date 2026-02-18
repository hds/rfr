use std::{
    collections::HashMap,
    error, fmt,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use tracing::{Event, Metadata, Subscriber, span, subscriber::Interest};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use rfr::{
    AbsTimestamp, Callsite, CallsiteId, InstrumentationId,
    chunked::{self, ChunkedWriter},
};

use crate::subscriber::common::{
    EventKind, SpanKind, SpawnFields, SpawnSpan, TaskId, TaskKind, TraceKind, WakerFields, WakerOp,
    get_context_task_iid, to_callsite, to_callsite_id, to_iid,
};

struct WriterHandle {
    writer: Arc<ChunkedWriter>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

pub struct Flusher {
    writer: Arc<ChunkedWriter>,
}

impl Flusher {
    /// Waits until the current traces have been flushed to disk
    ///
    /// In order to ensure that consistent chunks are written, this method will wait until the
    /// chunk which would contain a trace recorded at the moment it is called is written to disk
    /// before returning.
    pub fn wait_flush(&self) -> Result<(), FlushError> {
        self.writer
            .wait_for_write_timeout(Duration::from_micros(
                self.writer.chunk_period_micros() as u64 * 2,
            ))
            .map_err(|inner| FlushError { inner })
    }
}

/// Error waiting for a chunk to be written
#[derive(Debug, Clone, Copy)]
pub struct FlushError {
    inner: chunked::WaitForWriteError,
}

impl fmt::Display for FlushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl error::Error for FlushError {}

pub struct RfrChunkedLayer {
    writer_handle: WriterHandle,
    callsite_cache: Mutex<HashMap<CallsiteId, (Callsite, TraceKind)>>,
    object_cache: Mutex<HashMap<InstrumentationId, chunked::Object>>,
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
        let writer = Arc::new(ChunkedWriter::try_new(base_dir).unwrap());

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

            self.writer_handle.writer.write_all_chunks();
        } else {
            // Otherwise some other thread has joined on the writer.
        }
    }

    fn new_object(&self, iid: InstrumentationId, object: chunked::Object) {
        let mut object_cache = self.object_cache.lock().expect("object cache poisoned");
        object_cache.insert(iid, object);
    }

    fn drop_object(&self, iid: &InstrumentationId) {
        let mut object_cache = self.object_cache.lock().expect("object cache poisoned");
        object_cache.remove(iid);
    }

    fn get_objects(&self, iids: &[InstrumentationId]) -> Vec<Option<chunked::Object>> {
        let object_cache = self.object_cache.lock().expect("object cache poisoned");
        iids.iter()
            .map(|iid| object_cache.get(iid).cloned())
            .collect()
    }

    fn write_record(&self, timestamp: AbsTimestamp, data: chunked::RecordData) {
        self.writer_handle
            .writer
            .with_seq_chunk_buffer(timestamp.clone(), |current_buffer| {
                let record = chunked::Record {
                    meta: chunked::Meta {
                        timestamp: current_buffer.chunk_timestamp(&timestamp),
                    },
                    data,
                };
                current_buffer.append_record(record, |task_ids| self.get_objects(task_ids));
            });
    }
}

fn run_writer_loop(writer: Arc<ChunkedWriter>) {
    loop {
        if writer.is_closed() {
            break;
        }

        let Ok(sleep_duration) = writer.write_completed_chunks() else {
            // Error occurred, break.
            break;
        };
        thread::sleep(sleep_duration);
    }
}

impl<S> Layer<S> for RfrChunkedLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        match TraceKind::try_from(metadata) {
            Ok(kind) => {
                let callsite_id = to_callsite_id(metadata);
                let mut callsite_cache = self
                    .callsite_cache
                    .lock()
                    .expect("callsite cache is poisoned");
                callsite_cache.entry(callsite_id).or_insert_with(|| {
                    let new_callsite = to_callsite(metadata);
                    self.writer_handle
                        .writer
                        .register_callsite(new_callsite.clone());
                    (new_callsite, kind)
                });

                Interest::always()
            }
            Err(_) => Interest::never(),
        }
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let timestamp = AbsTimestamp::now();
        let callsite_id = to_callsite_id(attrs.metadata());
        let kind = {
            let callsite_cache = self.callsite_cache.lock().expect("callsite cache poisoned");
            let Some(callsite_kind) = callsite_cache.get(&callsite_id) else {
                return;
            };
            callsite_kind.1.clone()
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
                    let task_id = rfr::TaskId::from(spawn.task_id.0);
                    let task = chunked::Object::Task(rfr::Task {
                        iid: spawn.iid,
                        callsite_id: spawn.callsite_id,
                        task_id,
                        task_name: spawn.task_name,
                        task_kind: match spawn.task_kind {
                            TaskKind::Task => rfr::TaskKind::Task,
                            TaskKind::Local => rfr::TaskKind::Local,
                            TaskKind::Blocking => rfr::TaskKind::Blocking,
                            TaskKind::BlockOn => rfr::TaskKind::BlockOn,
                            TaskKind::Other(val) => rfr::TaskKind::Other(val),
                        },

                        context: spawn.context,
                    });
                    self.new_object(spawn.iid, task);
                    let rec_data = chunked::RecordData::TaskNew { iid: spawn.iid };
                    self.write_record(timestamp, rec_data);
                }
            }
            _ => {
                // Not yet implemented
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let timestamp = AbsTimestamp::now();
        let callsite = to_callsite_id(event.metadata());
        let kind = {
            let callsite_cache = self.callsite_cache.lock().expect("callsite cache poisoned");
            let Some(callsite_kind) = callsite_cache.get(&callsite) else {
                return;
            };
            callsite_kind.1.clone()
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

                {
                    let waker = rfr::Waker {
                        task_iid: to_iid(&task_span_id),
                        context: ctx.current_span().id().map(to_iid),
                    };
                    let waker_data = match op {
                        WakerOp::Wake => chunked::RecordData::WakerWake { waker },
                        WakerOp::WakeByRef => chunked::RecordData::WakerWakeByRef { waker },
                        WakerOp::Clone => chunked::RecordData::WakerClone { waker },
                        WakerOp::Drop => chunked::RecordData::WakerDrop { waker },
                    };

                    self.write_record(timestamp, waker_data);
                }
            }
            _ => {
                // Not yet implemented
            }
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let timestamp = AbsTimestamp::now();
        let span = ctx.span(id).expect("enter {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if extensions.get::<TaskId>().is_some() {
            // This is a runtime.spawn span
            let poll_start = chunked::RecordData::TaskPollStart { iid: to_iid(id) };
            self.write_record(timestamp, poll_start);
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let timestamp = AbsTimestamp::now();
        let span = ctx.span(id).expect("exit {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if extensions.get::<TaskId>().is_some() {
            // This is a runtime.spawn span
            let poll_end = chunked::RecordData::TaskPollEnd { iid: to_iid(id) };
            self.write_record(timestamp, poll_end);
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let timestamp = AbsTimestamp::now();
        let span = ctx
            .span(&id)
            .expect("close {id:?} not found, this is a bug");
        let extensions = span.extensions();
        if extensions.get::<TaskId>().is_some() {
            // This is a runtime.spawn span
            let iid = to_iid(&id);
            let task_drop = chunked::RecordData::TaskDrop { iid };

            self.write_record(timestamp, task_drop);
            self.drop_object(&iid);
        }
    }
}
