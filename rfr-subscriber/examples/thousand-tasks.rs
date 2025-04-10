use std::{future::Future, time::Duration};

use rfr_subscriber::RfrChunkedLayer;
use tracing_subscriber::prelude::*;

fn main() {
    let rfr_layer = RfrChunkedLayer::new("./chunked-thousand-tasks.rfr");
    let flusher = rfr_layer.flusher();
    tracing_subscriber::registry().with(rfr_layer).init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let mut join_handles = Vec::new();
        for idx in 0..1_000 {
            let task_name = format!("task-{idx}");
            join_handles.push(spawn_named(&task_name, async {
                tokio::time::sleep(Duration::from_micros(100)).await;
            }));
        }

        for jh in join_handles {
            jh.await.unwrap();
        }
    });

    flusher.wait_flush().unwrap();
}

#[track_caller]
fn spawn_named<Fut>(name: &str, f: Fut) -> tokio::task::JoinHandle<<Fut as Future>::Output>
where
    Fut: Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    tokio::task::Builder::new()
        .name(name)
        .spawn(f)
        .unwrap_or_else(|_| panic!("spawning task '{name}' failed"))
}
