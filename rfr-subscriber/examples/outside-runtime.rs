use std::{future::Future, thread};

use tokio::sync::mpsc;

use rfr_subscriber::RfrChunkedLayer;
use tracing_subscriber::prelude::*;

fn main() {
    let rfr_layer = RfrChunkedLayer::new("./chunked-outside-runtime.rfr");
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
        let serve_dn_tx = dn_tx.clone();

        let task_jh = spawn_task("ping", ping_pong_task(3, up_tx, dn_rx));
        let thread_jh = spawn_thread("pong", || ping_pong_thread(3, dn_tx, up_rx));

        // serve
        serve_dn_tx.send(()).await.unwrap();

        task_jh.await.unwrap();
        thread_jh.join().unwrap();
    });

    flusher.wait_flush().unwrap();
}

async fn ping_pong_task(count: usize, tx: mpsc::Sender<()>, mut rx: mpsc::Receiver<()>) {
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

fn ping_pong_thread(count: usize, tx: mpsc::Sender<()>, mut rx: mpsc::Receiver<()>) {
    for _ in 0..count {
        match rx.blocking_recv() {
            Some(_) => {
                // received something!
            }
            None => break,
        }

        match tx.blocking_send(()) {
            Ok(_) => {
                // sent something!
            }
            Err(_) => break,
        }
    }
}

#[track_caller]
fn spawn_task<Fut>(name: &str, f: Fut) -> tokio::task::JoinHandle<<Fut as Future>::Output>
where
    Fut: Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    tokio::task::Builder::new()
        .name(name)
        .spawn(f)
        .unwrap_or_else(|_| panic!("spawning task '{name}' failed"))
}

#[track_caller]
fn spawn_thread<F, T>(name: &str, func: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    thread::Builder::new()
        .name(name.into())
        .spawn(func)
        .unwrap_or_else(|_| panic!("sapwning thread '{name}' failed"))
}
