use std::{fs, sync::Arc, thread, time::Duration};

use rfr::{
    AbsTimestamp, Callsite, CallsiteId, Event, FieldName, FieldValue, InstrumentationId, Kind,
    Level, Parent,
    chunked::{
        self, ChunkedWriter, Meta, NewChunkedWriterError, Record, RecordData, StorageQuota,
        from_path,
    },
};
use tempfile::tempdir;

/// This is the measured size of a chunk containing just one event. It is used when testing the
/// storage quota and chunk chealup. If the chunk format changes, this may need to be updated.
const BYTES_PER_TEST_CHUNK: usize = 47;

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
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");

    let writer = Arc::new(ChunkedWriter::try_new(&recording_dir).unwrap());

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

    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
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

#[test]
fn directory_already_exists() {
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");
    //let recording_dir = base_dir..to_str().unwrap().to_string();

    fs::create_dir_all(&recording_dir).unwrap();

    let result = ChunkedWriter::try_new(recording_dir);
    assert!(
        result.is_err(),
        "expected `ChunkedWriter::try_new` to return an error"
    );

    match result.unwrap_err() {
        NewChunkedWriterError::AlreadyExists => {} // expected result
        other_err => panic!(
            "expected error `NewChunkedWriterError::AlreadyExists`, but instead got `{other_err:?}`"
        ),
    }
}

#[test]
fn meta_already_exists() {
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");
    //let base_dir = tempdir().unwrap();
    //let recording_dir = base_dir.path().to_str().unwrap().to_string();

    _ = ChunkedWriter::try_new(&recording_dir).unwrap();

    let result = ChunkedWriter::try_new(recording_dir.clone());
    assert!(
        result.is_err(),
        "expected `ChunkedWriter::try_new` to return an error"
    );

    match result.unwrap_err() {
        NewChunkedWriterError::AlreadyExists => {} // expected result
        other_err => panic!(
            "expected error `NewChunkedWriterError::AlreadyExists`, but instead got `{other_err:?}`"
        ),
    }
}

fn write_event_chunk_loop(writer: &ChunkedWriter, repeats: u64) {
    let callsite_id = CallsiteId::from(1);
    let callsite = Callsite {
        callsite_id,
        level: Level(10),
        kind: Kind::Event,
        const_fields: vec![],
        split_field_names: vec![FieldName("loop_index".into())],
    };
    writer.register_callsite(callsite);

    // The first write will be empty (no data, no chunk written), but then we'll know that after
    // the sleep we'll be directly inside the time period for the next chunk. From there, we know
    // that the first write will be empty again, so we add 1 to `repeats`.
    let sleep_duration = writer.write_completed_chunks().unwrap();
    thread::sleep(sleep_duration);

    for idx in 0..repeats + 1 {
        let timestamp = AbsTimestamp::now();
        let event = Event {
            callsite_id,
            parent: Parent::Root,
            split_field_values: vec![FieldValue::U64(idx)],
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

        let sleep_duration = writer.write_completed_chunks().unwrap();
        thread::sleep(sleep_duration);
    }
}

#[test]
fn clean_outdated_chunks_over_hard_limit_count() {
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");

    let storage_quota = StorageQuota {
        max_chunks_hard: Some(3),
        ..Default::default()
    };
    let writer = ChunkedWriter::try_new_with_config(&recording_dir, storage_quota).unwrap();

    write_event_chunk_loop(&writer, 5);

    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    // The first write may have been for an empty chunk.
    assert_eq!(chunks.len(), 5);

    writer.clean_outdated_chunks().unwrap();
    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    assert_eq!(chunks.len(), 3);
}

#[test]
fn clean_outdated_chunks_over_hard_limit_size() {
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");

    let storage_quota = StorageQuota {
        max_chunks_hard: Some(3),
        max_bytes_hard: Some(BYTES_PER_TEST_CHUNK * 2 + 1), // 47 bytes per chunk
        ..Default::default()
    };
    let writer = ChunkedWriter::try_new_with_config(&recording_dir, storage_quota).unwrap();

    write_event_chunk_loop(&writer, 5);

    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    // The first write may have been for an empty chunk.
    assert_eq!(chunks.len(), 5);

    writer.clean_outdated_chunks().unwrap();
    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    assert_eq!(chunks.len(), 2);
}

#[test]
fn clean_outdated_chunks_over_soft_limit_count() {
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");

    let storage_quota = StorageQuota {
        max_chunks_soft: Some(3),
        ..Default::default()
    };
    let writer = ChunkedWriter::try_new_with_config(&recording_dir, storage_quota).unwrap();

    write_event_chunk_loop(&writer, 5);

    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    // The first write may have been for an empty chunk.
    assert_eq!(chunks.len(), 5);

    writer.clean_outdated_chunks().unwrap();
    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    assert_eq!(chunks.len(), 3);
}

#[test]
fn clean_outdated_chunks_over_soft_limit_size() {
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");

    let storage_quota = StorageQuota {
        max_chunks_soft: Some(3),
        max_bytes_soft: Some(BYTES_PER_TEST_CHUNK * 2 + 1), // 47 bytes per chunk
        ..Default::default()
    };
    let writer = ChunkedWriter::try_new_with_config(&recording_dir, storage_quota).unwrap();

    write_event_chunk_loop(&writer, 5);

    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    // The first write may have been for an empty chunk.
    assert_eq!(chunks.len(), 5);

    writer.clean_outdated_chunks().unwrap();
    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    assert_eq!(chunks.len(), 2);
}

#[test]
fn clean_outdated_chunks_over_soft_with_min_limit_count() {
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");

    let storage_quota = StorageQuota {
        max_chunks_soft: Some(3),
        min_bytes: Some(BYTES_PER_TEST_CHUNK * 4 + 1),
        ..Default::default()
    };
    let writer = ChunkedWriter::try_new_with_config(&recording_dir, storage_quota).unwrap();

    write_event_chunk_loop(&writer, 5);

    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    // The first write may have been for an empty chunk.
    assert_eq!(chunks.len(), 5);

    writer.clean_outdated_chunks().unwrap();
    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    assert_eq!(chunks.len(), 4);
}

#[test]
fn clean_outdated_chunks_over_soft_with_min_limit_size() {
    let recording_dir = tempdir().unwrap().path().join("recording.rfr");

    let storage_quota = StorageQuota {
        max_bytes_soft: Some(BYTES_PER_TEST_CHUNK * 2 + 1),
        min_chunks: Some(3),
        ..Default::default()
    };
    let writer = ChunkedWriter::try_new_with_config(&recording_dir, storage_quota).unwrap();

    write_event_chunk_loop(&writer, 5);

    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    // The first write may have been for an empty chunk.
    assert_eq!(chunks.len(), 5);

    writer.clean_outdated_chunks().unwrap();
    let mut recording = from_path(recording_dir.to_str().unwrap().to_owned()).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();
    assert_eq!(chunks.len(), 3);
}
