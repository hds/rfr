use std::{sync::Arc, thread, time::Duration};

use rfr::{
    chunked::{self, from_path, Callsite, CallsiteId, ChunkedWriter, Meta, Record, RecordData},
    common::{Event, FieldName, FieldValue, InstrumentationId, Kind, Level, Parent},
    rec::AbsTimestamp,
};
use tempfile::tempdir;

fn spawn_writer_loop(writer: Arc<ChunkedWriter>) {
    thread::Builder::new()
        .name(format!(
            "writer-{}",
            thread::current().name().unwrap_or("main")
        ))
        .spawn(move || {
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
        })
        .unwrap();
}

fn no_objects(iids: &[InstrumentationId]) -> Vec<Option<chunked::Object>> {
    iids.iter().map(|_| None).collect()
}

#[test]
fn record_single_event() {
    let base_dir = tempdir().unwrap();
    let recording_dir = base_dir.path().to_str().unwrap().to_string();

    let writer = Arc::new(ChunkedWriter::try_new(recording_dir.clone()).unwrap());

    spawn_writer_loop(Arc::clone(&writer));
    let timestamp = AbsTimestamp::now();

    let callsite_id = CallsiteId::from(1);
    let callsite = Callsite {
        callsite_id,
        level: Level(10),
        kind: Kind::Event,
        const_fields: vec![],
        split_field_names: vec![FieldName("message".into())],
    };
    writer.register_callsite(callsite);

    let event = Event {
        callsite_id,
        parent: Parent::Root,
        split_field_values: vec![FieldValue::Str("hi there".into())],
        dynamic_fields: vec![],
    };
    writer.with_seq_chunk_buffer(timestamp.clone(), |buffer| {
        let record = Record {
            meta: Meta {
                timestamp: buffer.chunk_timestamp(&timestamp),
            },
            data: RecordData::Event {
                event: event.clone(),
            },
        };

        buffer.append_record(record, no_objects);
    });

    writer
        .wait_for_write_timeout(Duration::from_secs(2))
        .unwrap();
    writer.close();

    let mut recording = from_path(recording_dir).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();

    assert!(!chunks.is_empty());
    let mut records = Vec::new();
    for chunk in &chunks {
        for seq_chunk in chunk.seq_chunks() {
            for record in &seq_chunk.records {
                records.push((chunk, record));
            }
        }
    }
    assert_eq!(records.len(), 1);

    let (chunk, actual_record) = records[0];
    assert_eq!(
        chunk.abs_timestamp(&actual_record.meta.timestamp),
        timestamp
    );
    assert_eq!(actual_record.data, RecordData::Event { event });
}
