use std::{future::Future, task::Poll, time::Duration};

use tracing_subscriber::prelude::*;

use rfr_subscriber::RfrChunkedLayer;

fn main() {
    let rfr_layer = RfrChunkedLayer::new("./chunked-long.rfr");
    let flusher = rfr_layer.flusher();
    tracing_subscriber::registry().with(rfr_layer).init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        spawn_named("blocks", double_sleepy(100, 110));
        spawn_named("burn", burn(40, 50));
        spawn_named("noyield", no_yield(1_000));
        spawn_named("spawns_blocking", spawn_blocking(5_000));

        let _task1 = spawn_named("task1", spawn_tasks(40, 50));
        let _task2 = spawn_named("task1", spawn_tasks(100, 120));

        tokio::time::sleep(Duration::from_secs(60)).await;
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

#[tracing::instrument]
async fn spawn_tasks(min: u64, max: u64) {
    loop {
        for i in min..max {
            tracing::trace!(i, "spawning wait task");
            tokio::task::Builder::new()
                .name("wait")
                .spawn(wait(i))
                .unwrap();

            let sleep = Duration::from_micros(i);
            tracing::trace!(?sleep, "sleeping...");
            tokio::time::sleep(sleep).await;
        }
    }
}

#[tracing::instrument]
async fn wait(micros: u64) {
    tracing::debug!("waiting...");
    tokio::time::sleep(Duration::from_micros(micros)).await;
    tracing::trace!("done!");
}

#[tracing::instrument]
async fn double_sleepy(min: u64, max: u64) {
    loop {
        for i in min..max {
            // woops!
            std::thread::sleep(Duration::from_micros(i));
            tokio::time::sleep(Duration::from_micros(max - i)).await;
        }
    }
}

#[tracing::instrument]
async fn burn(min: u64, max: u64) {
    loop {
        for i in min..max {
            for _ in 0..i {
                self_wake().await;
            }
            tokio::time::sleep(Duration::from_micros(i - min)).await;
        }
    }
}

#[tracing::instrument]
async fn no_yield(micros: u64) {
    loop {
        let handle = tokio::task::Builder::new()
            .name("greedy")
            .spawn(async move {
                std::thread::sleep(Duration::from_micros(micros));
            })
            .expect("Couldn't spawn greedy task");

        _ = handle.await;
    }
}

#[tracing::instrument]
async fn spawn_blocking(micros: u64) {
    loop {
        _ = tokio::task::spawn_blocking(move || {
            std::thread::sleep(Duration::from_micros(micros));
        })
        .await;
    }
}

fn self_wake() -> impl Future<Output = ()> {
    struct SelfWake {
        yielded: bool,
    }

    impl Future for SelfWake {
        type Output = ();

        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<Self::Output> {
            if self.yielded {
                return Poll::Ready(());
            }

            self.yielded = true;
            cx.waker().wake_by_ref();

            Poll::Pending
        }
    }

    SelfWake { yielded: false }
}
