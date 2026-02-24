use std::time::Duration;

use tempfile::tempdir;
use tracing_subscriber::{prelude::*, registry};

use rfr::chunked;
use rfr_subscriber::{ChunkedLayer, StorageQuota};

#[test]
fn record_spawn() {
    let recording_dir = temp_recording_dir();
    let layer = ChunkedLayer::builder()
        .recording_dir(recording_dir.clone())
        .build()
        .unwrap();
    let flusher = layer.flusher();
    let subscriber = registry().with(layer);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async move {
        tracing::subscriber::with_default(subscriber, move || {
            tokio::task::Builder::new()
                .name("the-task")
                .spawn(async {})
                .unwrap();
            flusher.wait_flush().unwrap();
        });
    });

    let mut recording = chunked::from_path(recording_dir).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();

    let last_chunk = chunks.last().unwrap();
    let objects: Vec<&chunked::Object> = last_chunk
        .seq_chunks()
        .iter()
        .flat_map(|seq_chunk| &seq_chunk.objects)
        .collect();

    assert_eq!(objects.len(), 1);
    match objects[0] {
        chunked::Object::Task(task) => assert_eq!(task.task_name, "the-task".to_string()),
        object => panic!("Expected object to be a Task, but instead got `{object:?}`"),
    }
}

#[test]
fn storage_quota() {
    let recording_dir = temp_recording_dir();
    let quota = StorageQuota {
        max_chunks_hard: Some(2),
        ..Default::default()
    };
    let layer = ChunkedLayer::builder()
        .recording_dir(recording_dir.clone())
        .storage_quota(quota)
        .build()
        .unwrap();
    let flusher = layer.flusher();
    let subscriber = registry().with(layer);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    tracing::subscriber::with_default(subscriber, move || {
        rt.block_on(async move {
            for _ in 0..5 {
                tokio::task::Builder::new()
                    .name("the-task")
                    .spawn(async {})
                    .unwrap();
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            flusher.wait_flush().unwrap();
        });
    });

    let mut recording = chunked::from_path(recording_dir).unwrap();
    let chunks: Vec<_> = recording.chunks_lossy().flatten().collect();

    assert_eq!(chunks.len(), 2);
}

fn temp_recording_dir() -> String {
    tempdir()
        .unwrap()
        .path()
        .join("recording.rfr")
        .to_str()
        .unwrap()
        .to_owned()
}
