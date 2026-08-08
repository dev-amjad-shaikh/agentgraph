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
//!
//! ## Test hooks (defaults unchanged)
//!
//! The crash-recovery release proof (`rusty-server/tests/crash_recovery.rs`)
//! runs this binary as a real process it can SIGKILL mid-effect. The hooks
//! are env vars; with none set the interactive demo behaves exactly as
//! before:
//!
//! - `RUSTY_DEMO_SERVER_URL` — the server to claim from
//!   (default `http://127.0.0.1:8100`).
//! - `RUSTY_DEMO_WORKER_ID` — the identity sent on claim/heartbeat/settle
//!   (default `demo-activity-worker`).
//! - `RUSTY_DEMO_LEASE_MS` — the requested lease (default `30000`; the
//!   proof uses `1000` so a killed worker's task returns to visibility
//!   fast).
//! - `RUSTY_DEMO_EFFECT_PAUSE_MS` — the post-effect pause standing in for
//!   provider latency (default `2000`). It applies only when the effect
//!   actually fires — a deduplicated re-attempt returns immediately, like a
//!   real idempotent provider — which is what gives the proof its
//!   deterministic kill window.
//! - `RUSTY_DEMO_PROVIDER_FILE` — when set, `send_receipt` runs against a
//!   file-backed idempotent "provider" (see [`FileProvider`]): the file is
//!   the provider's ledger of effect invocations, keyed by the task's
//!   idempotency key, so a redelivered attempt is a no-op at the effect and
//!   the ledger is inspectable evidence of exactly how many times the
//!   effect fired.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use rusty_agent_runtime::error::Result;
use rusty_agent_runtime::record::EffectReceipt;
use rusty_worker::{Activity, ActivityCompletion, ActivityContext, ActivityWorker};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

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

/// A file-backed stand-in for an idempotent external provider (Stripe,
/// SendGrid — any store with idempotent-put semantics).
///
/// The ledger file is the provider's state: one JSON line per *effect
/// invocation*, keyed by the idempotency key the task carried. The two rules
/// are exactly a real provider's:
///
/// - **Perform** — a key the ledger does not know fires the effect: append
///   the invocation record and `fsync` *before* returning, so a SIGKILL
///   landing anywhere after this point still leaves the effect durable at
///   the provider. That durability is what creates the classic crash window
///   — effect fired, completion never reported.
/// - **Dedupe** — a key the ledger already knows is a no-op: the effect
///   does NOT fire again; the stored confirmation is answered immediately
///   (no pause), and the same `provider_id` comes back.
///
/// The file is also the proof's evidence: after killing and restarting
/// everything, counting the lines under one idempotency key tells you
/// exactly how many times the external effect fired.
struct FileProvider {
    /// The provider ledger (JSONL); the "external system" both worker
    /// processes talk to.
    ledger: PathBuf,
    /// Stand-in for provider latency, applied only on a real effect firing
    /// (see the module docs): the window in which a crash leaves "effect
    /// fired, completion never reported".
    pause: Duration,
}

impl FileProvider {
    /// The provider's idempotency lookup: the stored invocation record for
    /// `key`, if the effect already landed.
    fn find(&self, key: &str) -> Option<Value> {
        let contents = std::fs::read_to_string(&self.ledger).ok()?;
        contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|record| record["idempotency_key"] == json!(key))
    }

    /// Perform the effect: append one invocation record and `fsync` so the
    /// effect survives a `kill -9` of this process arriving any time after.
    /// (Blocking IO inside an async handler — fine for a demo ledger of a
    /// few lines, and honesty about durability beats looking async.)
    fn fire(&self, record: &Value) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.ledger)
            .expect("the provider ledger must be writable");
        use std::io::Write;
        let mut line = record.to_string();
        line.push('\n');
        file.write_all(line.as_bytes())
            .expect("the provider ledger must accept the effect record");
        file.sync_all()
            .expect("the effect must be durable at the provider before we report it");
    }
}

#[async_trait]
impl Activity for FileProvider {
    async fn run(&self, ctx: ActivityContext) -> Result<Value> {
        Ok(self.run_with_receipt(ctx).await?.result)
    }

    async fn run_with_receipt(&self, ctx: ActivityContext) -> Result<ActivityCompletion> {
        // The correlation handle the whole effectively-once contract hangs
        // on: enqueue-time idempotency key, falling back to the task id.
        let key = ctx
            .idempotency_key()
            .unwrap_or_else(|| ctx.task_id())
            .to_owned();
        let to = ctx.payload()["to"].as_str().unwrap_or("unknown").to_owned();

        let make_receipt = |provider_id: String| EffectReceipt {
            provider: "file-provider".to_string(),
            provider_id,
            idempotency_key: key.clone(),
            task_id: Some(ctx.task_id().to_owned()),
        };

        // Dedupe: the effect already landed for this key — answer the stored
        // confirmation WITHOUT re-firing and without the latency pause. This
        // is the branch the crash-recovery proof's second attempt must take.
        if let Some(record) = self.find(&key) {
            let provider_id = record["provider_id"].as_str().unwrap_or("").to_owned();
            return Ok(ActivityCompletion {
                result: json!({"sent": true, "to": to, "provider_id": provider_id,
                               "deduplicated": true}),
                receipt: Some(make_receipt(provider_id)),
            });
        }

        // Perform the effect, then pause: the window in which the proof
        // SIGKILLs this process — effect durable at the provider, completion
        // never reported to the server.
        let provider_id = format!("msg-{}", Uuid::new_v4());
        self.fire(&json!({
            "idempotency_key": key,
            "provider_id": provider_id,
            "task_id": ctx.task_id(),
            "attempt": ctx.attempt(),
            "to": to,
        }));
        tokio::time::sleep(self.pause).await;
        Ok(ActivityCompletion {
            result: json!({"sent": true, "to": to, "provider_id": provider_id,
                           "deduplicated": false}),
            receipt: Some(make_receipt(provider_id)),
        })
    }
}

/// Read an env-var test hook, falling back to `default` when unset.
fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rusty_worker=info".into()),
        )
        .init();

    let server_url = env_or("RUSTY_DEMO_SERVER_URL", "http://127.0.0.1:8100");
    let worker_id = env_or("RUSTY_DEMO_WORKER_ID", "demo-activity-worker");
    let lease_ms: u64 = env_or("RUSTY_DEMO_LEASE_MS", "30000")
        .parse()
        .expect("RUSTY_DEMO_LEASE_MS must be milliseconds");
    let pause_ms: u64 = env_or("RUSTY_DEMO_EFFECT_PAUSE_MS", "2000")
        .parse()
        .expect("RUSTY_DEMO_EFFECT_PAUSE_MS must be milliseconds");
    let provider_file = std::env::var("RUSTY_DEMO_PROVIDER_FILE")
        .ok()
        .map(PathBuf::from);

    let worker = ActivityWorker::new(&server_url);
    // The provider-backed activity replaces the plain stand-in when the test
    // hook points at a ledger — same `send_receipt` kind either way.
    let worker = match provider_file {
        Some(ledger) => worker.register(
            "send_receipt",
            FileProvider {
                ledger,
                pause: Duration::from_millis(pause_ms),
            },
        ),
        None => worker.register("send_receipt", move |ctx: ActivityContext| async move {
            // `task_id` / `idempotency_key` are the correlation handles that
            // make a redelivered attempt effectively-once at the effect.
            let _dedup_key = ctx.idempotency_key().unwrap_or_else(|| ctx.task_id());
            let to = ctx.payload()["to"].as_str().unwrap_or("unknown");
            // Stand in for a slow provider call so a drain is observable.
            tokio::time::sleep(Duration::from_millis(pause_ms)).await;
            Ok(json!({"sent": true, "to": to}))
        }),
    };
    let worker = worker
        .with_worker_id(&worker_id)
        .with_pools(["default"])
        // Demo-sized: production defaults are 30 s lease / 25 s grace.
        .with_lease(Duration::from_millis(lease_ms))
        .with_drain_grace(Duration::from_secs(25));

    println!("rusty activity worker demo (against {server_url})");
    println!("  enqueue: curl -X POST {server_url}/tasks \\");
    println!("    -H 'content-type: application/json' \\");
    println!("    -d '{{\"kind\": \"send_receipt\", \"payload\": {{\"to\": \"a@b.c\"}}}}'");
    println!("  drain:   Ctrl-C or `kill <pid>` — claiming stops, the in-flight");
    println!("           activity settles within the grace, then the worker exits");

    let drain = CancellationToken::new();
    tokio::spawn(watch_signals(drain.clone()));
    worker.run(drain).await;
    println!("drained: in-flight work settled or released; exiting");
}
