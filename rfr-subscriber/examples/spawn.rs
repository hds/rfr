use std::{future::Future, time::Duration};

use rfr_subscriber::RfrLayer;
use tracing_subscriber::prelude::*;

fn main() {
    let rfr_layer = RfrLayer::new();
    tracing_subscriber::registry().with(rfr_layer).init();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let jh = spawn_named("outer", async {
            spawn_named("inner-awesome", async {
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        _ = jh.await;
    });
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