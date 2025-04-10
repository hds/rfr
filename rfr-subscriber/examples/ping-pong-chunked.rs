use std::future::Future;

use tokio::sync::mpsc;

use rfr_subscriber::RfrChunkedLayer;
use tracing_subscriber::prelude::*;

fn main() {
    let rfr_layer = RfrChunkedLayer::new("./chunked-ping-pong.rfr");
    let flusher = rfr_layer.flusher();
    tracing_subscriber::registry().with(rfr_layer).init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let (up_tx, up_rx) = mpsc::channel::<()>(1);
        let (dn_tx, dn_rx) = mpsc::channel::<()>(1);

        let join_handles = vec![
            spawn_named("ping", ping_pong(3, up_tx, dn_rx)),
            spawn_named("pong", ping_pong(3, dn_tx.clone(), up_rx)),
        ];

        // serve
        dn_tx.send(()).await.unwrap();

        for jh in join_handles {
            jh.await.unwrap();
        }
    });

    flusher.wait_flush().unwrap();
}

async fn ping_pong(count: usize, tx: mpsc::Sender<()>, mut rx: mpsc::Receiver<()>) {
    for _ in 0..count {
        match rx.recv().await {
            Some(_) => {
                // received something!
            }
            None => break,
        }

        match tx.send(()).await {
            Ok(_) => {
                // sent something!
            }
            Err(_) => break,
        }
    }
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
