//! Demo activity worker: claims durable tasks from a rusty-server task
//! queue and executes them — with production-shaped lifecycle wiring.
//!
//! The point of the example is the shutdown half: `ActivityWorker` is a
//! library, so it exposes drain as a `CancellationToken` and leaves signal
//! handling to the embedding binary. This is that wiring, exactly as a
//! deployment should do it: SIGTERM (or Ctrl-C) cancels the token, which
//! drains the worker — claiming stops immediately, the in-flight activity
//! settles within the drain grace, and anything that outlives the grace is
//! left for the server to reassign at lease expiry. Under Kubernetes the
//! pod's `terminationGracePeriodSeconds` (30 s by default) should outlive
//! the drain grace (25 s by default) so the worker exits before SIGKILL.
//!
//! Run a server first (`cargo run -p rusty-server --example server_demo`),
//! then:
//!
//! ```sh
//! cargo run --example activity_worker_demo
//! # enqueue work:
//! curl -X POST localhost:8100/tasks -H 'content-type: application/json' \
//!   -d '{"kind": "send_receipt", "payload": {"to": "a@b.c"}}'
//! # then Ctrl-C the worker and watch it drain
//! ```

use std::time::Duration;

use rusty_worker::{ActivityContext, ActivityWorker};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// The SIGTERM/SIGINT half of the drain contract: first signal cancels the
/// token (drain), and the worker's grace bounds the exit. A second signal
/// means the operator wants out now — honor it immediately.
async fn watch_signals(drain: CancellationToken) {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("installing the SIGTERM handler must succeed");
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
    drain.cancel();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rusty_worker=info".into()),
        )
        .init();

    let worker = ActivityWorker::new("http://127.0.0.1:8100")
        .register("send_receipt", |ctx: ActivityContext| async move {
            // `task_id` / `idempotency_key` are the correlation handles that
            // make a redelivered attempt effectively-once at the effect.
            let _dedup_key = ctx.idempotency_key().unwrap_or_else(|| ctx.task_id());
            let to = ctx.payload()["to"].as_str().unwrap_or("unknown");
            // Stand in for a slow provider call so a drain is observable.
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(json!({"sent": true, "to": to}))
        })
        .with_worker_id("demo-activity-worker")
        .with_pools(["default"])
        // Demo-sized: production defaults are 30 s lease / 25 s grace.
        .with_lease(Duration::from_secs(30))
        .with_drain_grace(Duration::from_secs(25));

    println!("rusty activity worker demo (against http://127.0.0.1:8100)");
    println!("  enqueue: curl -X POST localhost:8100/tasks \\");
    println!("    -H 'content-type: application/json' \\");
    println!("    -d '{{\"kind\": \"send_receipt\", \"payload\": {{\"to\": \"a@b.c\"}}}}'");
    println!("  drain:   Ctrl-C or `kill <pid>` — claiming stops, the in-flight");
    println!("           activity settles within the grace, then the worker exits");

    let drain = CancellationToken::new();
    tokio::spawn(watch_signals(drain.clone()));
    worker.run(drain).await;
    println!("drained: in-flight work settled or released; exiting");
}
