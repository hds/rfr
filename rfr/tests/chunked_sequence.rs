use rfr::{
    chunked::{ChunkInterval, EventRecord, Meta, Object, SeqChunk, SeqChunkBuffer},
    common::{Event, Task, TaskId, TaskKind},
    rec::AbsTimestamp,
};

#[test]
fn round_trip() {
    let mut buffer = Vec::new();

    let seq_chunk_buffer = SeqChunkBuffer::new(ChunkInterval::from_timestamp_and_period(
        AbsTimestamp::now(),
        1_000_000,
    ));

    let task = test_task(2);
    let event = EventRecord {
        meta: Meta {
            timestamp: seq_chunk_buffer.chunk_timestamp(&AbsTimestamp::now()),
        },
        event: Event::NewTask { id: task.task_id },
    };
    seq_chunk_buffer.append_record(event.clone(), |_task_ids| {
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

    assert_eq!(seq_chunk.events.len(), 1);
    assert_eq!(seq_chunk.events[0], event);
}

#[test]
fn skip_events_with_unknown_objects() {
    let seq_chunk_buffer = SeqChunkBuffer::new(ChunkInterval::from_timestamp_and_period(
        AbsTimestamp::now(),
        1_000_000,
    ));

    let event = EventRecord {
        meta: Meta {
            timestamp: seq_chunk_buffer.chunk_timestamp(&AbsTimestamp::now()),
        },
        event: Event::NewTask {
            id: TaskId::from(5),
        },
    };
    seq_chunk_buffer.append_record(event.clone(), |_task_ids| vec![None]);

    assert_eq!(seq_chunk_buffer.event_count(), 0);
}

#[test]
fn only_requests_object_once() {
    let mut buffer = Vec::new();

    let seq_chunk_buffer = SeqChunkBuffer::new(ChunkInterval::from_timestamp_and_period(
        AbsTimestamp::now(),
        1_000_000,
    ));

    let task = test_task(2);
    let event_1 = EventRecord {
        meta: Meta {
            timestamp: seq_chunk_buffer.chunk_timestamp(&AbsTimestamp::now()),
        },
        event: Event::NewTask { id: task.task_id },
    };
    seq_chunk_buffer.append_record(event_1, |task_ids| {
        assert_eq!(task_ids.len(), 1);
        assert_eq!(task_ids[0], TaskId::from(2));

        vec![Some(Object::Task(task.clone()))]
    });

    let event_2 = EventRecord {
        meta: Meta {
            timestamp: seq_chunk_buffer.chunk_timestamp(&AbsTimestamp::now()),
        },
        event: Event::TaskDrop { id: task.task_id },
    };
    seq_chunk_buffer.append_record(event_2, |task_ids| {
        assert!(task_ids.is_empty());

        vec![]
    });

    seq_chunk_buffer.write(&mut buffer);
}

fn test_task(task_id: u64) -> Task {
    Task {
        task_id: task_id.into(),
        task_name: "Cool task".into(),
        task_kind: TaskKind::Task,
        context: None,
    }
}
