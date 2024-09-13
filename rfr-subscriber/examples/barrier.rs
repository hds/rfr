use std::{future::Future, sync::Arc, time::Duration};

use tokio::sync::Barrier;
use tracing_subscriber::prelude::*;

use rfr_subscriber::RfrChunkedLayer;

fn main() {
    let rfr_layer = RfrChunkedLayer::new("./chunked-barrier.rfr");
    let flusher = rfr_layer.flusher();
    tracing_subscriber::registry().with(rfr_layer).init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        spawn_named("main-task", async move {
            let task_count = 10;
            let mut handles = Vec::with_capacity(task_count);
            let barrier = Arc::new(Barrier::new(task_count));
            for i in 0..task_count {
                let c = barrier.clone();
                let task_name = format!("task-{}", i);
                handles.push(spawn_named(&task_name, async move {
                    tokio::time::sleep(Duration::from_micros(i as u64)).await;
                    c.wait().await
                }));
            }

            // Will not resolve until all "after wait" messages have been printed
            let mut num_leaders = 0;
            for handle in handles {
                let wait_result = handle.await.unwrap();
                if wait_result.is_leader() {
                    num_leaders += 1;
                }
            }

            // Exactly one barrier will resolve as the "leader"
            assert_eq!(num_leaders, 1);
        })
        .await
        .unwrap();
    });

    flusher.flush();
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
