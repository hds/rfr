use std::time::Duration;

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
        let jh = tokio::spawn(async {
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        _ = jh.await;
    });
}
