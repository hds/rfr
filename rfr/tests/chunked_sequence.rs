use rfr::{
    AbsTimestamp, CallsiteId, InstrumentationId, Task, TaskKind,
    chunked::{ChunkInterval, Meta, Object, Record, RecordData, SeqChunk, SeqChunkBuffer},
};

#[test]
fn round_trip() {
    let mut buffer = Vec::new();

    let seq_chunk_buffer = SeqChunkBuffer::new(ChunkInterval::from_timestamp_and_period(
        AbsTimestamp::now(),
        1_000_000,
    ));

    let task = test_task(2);
    let record = Record {
        meta: Meta {
            timestamp: seq_chunk_buffer.chunk_timestamp(&AbsTimestamp::now()),
        },
        data: RecordData::TaskNew { iid: task.iid },
    };
    seq_chunk_buffer.append_record(record.clone(), |_task_ids| {
        vec![Some(Object::Task(task.clone()))]
    });
    seq_chunk_buffer.write(&mut buffer);

    assert!(!buffer.is_empty());

    let seq_chunk: SeqChunk = postcard::from_bytes(buffer.as_mut_slice()).unwrap();
    let header = seq_chunk.header;

    assert_eq!(header.seq_id, seq_chunk_buffer.seq_id());
    assert_eq!(
        header.earliest_timestamp,
        seq_chunk_buffer.earliest_timestamp()
    );
    assert_eq!(header.latest_timestamp, seq_chunk_buffer.latest_timestamp());

    assert_eq!(seq_chunk.objects.len(), 1);
    assert_eq!(seq_chunk.objects[0], Object::Task(task));

    assert_eq!(seq_chunk.records.len(), 1);
    assert_eq!(seq_chunk.records[0], record);
}

#[test]
fn skip_records_with_unknown_objects() {
    let seq_chunk_buffer = SeqChunkBuffer::new(ChunkInterval::from_timestamp_and_period(
        AbsTimestamp::now(),
        1_000_000,
    ));

    let record = Record {
        meta: Meta {
            timestamp: seq_chunk_buffer.chunk_timestamp(&AbsTimestamp::now()),
        },
        data: RecordData::TaskNew {
            iid: InstrumentationId::from(5),
        },
    };
    seq_chunk_buffer.append_record(record.clone(), |_task_ids| vec![None]);

    assert_eq!(seq_chunk_buffer.record_count(), 0);
}

#[test]
fn only_requests_object_once() {
    let mut buffer = Vec::new();

    let seq_chunk_buffer = SeqChunkBuffer::new(ChunkInterval::from_timestamp_and_period(
        AbsTimestamp::now(),
        1_000_000,
    ));

    let task = test_task(2);
    let record_1 = Record {
        meta: Meta {
            timestamp: seq_chunk_buffer.chunk_timestamp(&AbsTimestamp::now()),
        },
        data: RecordData::TaskNew { iid: task.iid },
    };
    seq_chunk_buffer.append_record(record_1, |task_ids| {
        assert_eq!(task_ids.len(), 1);
        assert_eq!(task_ids[0], InstrumentationId::from(2));

        vec![Some(Object::Task(task.clone()))]
    });

    let record_2 = Record {
        meta: Meta {
            timestamp: seq_chunk_buffer.chunk_timestamp(&AbsTimestamp::now()),
        },
        data: RecordData::TaskDrop { iid: task.iid },
    };
    seq_chunk_buffer.append_record(record_2, |task_ids| {
        assert!(task_ids.is_empty());

        vec![]
    });

    seq_chunk_buffer.write(&mut buffer);
}

fn test_task(iid: u64) -> Task {
    Task {
        iid: iid.into(),
        callsite_id: CallsiteId::from(1),
        task_name: "Cool task".into(),
        task_kind: TaskKind::Task,
        context: None,
        task_id: iid.into(),
    }
}
