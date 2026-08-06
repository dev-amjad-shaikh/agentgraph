//! Crons: durable schedule records plus a tokio scheduler that fires runs.
//!
//! A cron is `{graph, schedule, input?}` where the schedule is either a
//! fixed interval in seconds (`interval_secs`) or a 5-field cron expression
//! (`cron_expr`, minute resolution, evaluated in UTC). Records are persisted
//! as one JSON file per cron under `{store_path}/crons/{cron_id}.json` and
//! reloaded when the router is built. A single background task (spawned by
//! [`crate::routes::router`]) ticks every 200 ms, fires due crons — each
//! firing creates a fresh thread bound to the cron's graph and schedules a
//! background run on it — and honors `on_run_completed`: `keep` (default)
//! leaves the cron active, `delete` removes it once the first fired run
//! reaches a terminal state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::routes::{AppState, ThreadRecord};
use crate::runs::{self, MultitaskStrategy, RunPayload};

/// What happens to a cron after one of its runs reaches a terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OnRunCompleted {
    /// Keep the cron firing on its schedule.
    #[default]
    Keep,
    /// Delete the cron once the first fired run finishes.
    Delete,
}

impl OnRunCompleted {
    /// Parse the wire value (`None` defaults to `keep`).
    pub(crate) fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw {
            None | Some("keep") => Ok(Self::Keep),
            Some("delete") => Ok(Self::Delete),
            Some(other) => Err(format!(
                "unknown on_run_completed `{other}` (expected `keep` or `delete`)"
            )),
        }
    }
}

/// One cron: a schedule that fires runs of a registered graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CronRecord {
    pub cron_id: String,
    /// Registered graph the fired runs execute.
    pub graph: String,
    /// Fixed-interval schedule: seconds between firings (XOR `cron_expr`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u64>,
    /// 5-field cron expression (`min hour day-of-month month day-of-week`,
    /// UTC), evaluated with a leading `0` seconds field (XOR
    /// `interval_secs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_expr: Option<String>,
    /// Initial state for fired runs (must be a JSON object when present).
    #[serde(default)]
    pub input: Option<Value>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub on_run_completed: OnRunCompleted,
    pub created_at: DateTime<Utc>,
    /// Wall-clock of the most recent firing (scheduler-maintained).
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    /// Total runs fired by this cron (scheduler-maintained).
    #[serde(default)]
    pub runs_fired: u64,
}

/// Validate a create-payload schedule pair: exactly one of interval or
/// expression, interval >= 1s, expression parseable.
pub(crate) fn validate_schedule(
    interval_secs: Option<u64>,
    cron_expr: Option<&str>,
) -> Result<(), String> {
    match (interval_secs, cron_expr) {
        (Some(0), None) => Err("`interval_secs` must be >= 1".to_string()),
        (Some(_), None) => Ok(()),
        (None, Some(expr)) => parse_expr(expr).map(|_| ()),
        (None, None) => {
            Err("exactly one of `interval_secs` or `cron_expr` is required".to_string())
        }
        (Some(_), Some(_)) => {
            Err("`interval_secs` and `cron_expr` are mutually exclusive".to_string())
        }
    }
}

/// Parse a 5-field cron expression via the `cron` crate (which expects a
/// leading seconds field, so we pin it to `0`).
fn parse_expr(expr: &str) -> Result<cron::Schedule, String> {
    cron::Schedule::from_str(&format!("0 {expr}"))
        .map_err(|e| format!("invalid cron expression `{expr}`: {e}"))
}

/// The next firing strictly after `now` (`None` only for corrupt records,
/// which creation-time validation prevents).
fn next_after(cron: &CronRecord, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match (cron.interval_secs, &cron.cron_expr) {
        (Some(secs), None) => Some(now + chrono::Duration::seconds(secs.max(1) as i64)),
        (None, Some(expr)) => parse_expr(expr).ok()?.upcoming(Utc).next(),
        _ => None,
    }
}

/// The on-disk directory holding one JSON file per cron.
pub(crate) fn dir(store_root: &Path) -> PathBuf {
    store_root.join("crons")
}

/// Load all persisted crons, skipping (with a warning) any file that fails
/// to parse.
pub(crate) fn load(store_root: &Path) -> HashMap<String, CronRecord> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir(store_root)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<CronRecord>(&raw).ok());
        match parsed {
            Some(record) => {
                out.insert(record.cron_id.clone(), record);
            }
            None => {
                tracing::warn!(path = %path.display(), "skipping unreadable cron file")
            }
        }
    }
    out
}

/// Persist one cron record (create or overwrite).
pub(crate) async fn persist(store_root: &Path, record: &CronRecord) -> std::io::Result<()> {
    let dir = dir(store_root);
    tokio::fs::create_dir_all(&dir).await?;
    let raw = serde_json::to_vec_pretty(record).expect("cron serialization is infallible");
    tokio::fs::write(dir.join(format!("{}.json", record.cron_id)), raw).await
}

/// Spawn the background scheduler task. Lives for the app's lifetime; the
/// returned task is deliberately detached.
pub(crate) fn spawn_scheduler(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Per-cron next-due bookkeeping, rebuilt as crons come and go.
        let mut next_due: HashMap<String, DateTime<Utc>> = HashMap::new();
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let crons = match state.server_store.list_crons().await {
                Ok(crons) => crons,
                Err(error) => {
                    tracing::warn!(%error, "cron scheduler: listing crons failed");
                    continue;
                }
            };
            let now = Utc::now();
            for cron in crons {
                if !next_due.contains_key(&cron.cron_id) {
                    match next_after(&cron, now) {
                        Some(due) => {
                            next_due.insert(cron.cron_id.clone(), due);
                        }
                        None => continue,
                    }
                }
                let due = next_due.get_mut(&cron.cron_id).expect("inserted above");
                if now < *due {
                    continue;
                }
                match next_after(&cron, now) {
                    Some(next) => *due = next,
                    None => {
                        next_due.remove(&cron.cron_id);
                    }
                }
                tokio::spawn(fire(Arc::clone(&state), cron));
            }
        }
    });
}

/// One firing: create a fresh thread, schedule the cron's run on it, update
/// bookkeeping, and honor `on_run_completed`.
async fn fire(state: Arc<AppState>, cron: CronRecord) {
    let thread_id = uuid::Uuid::new_v4().to_string();
    {
        let mut threads = state.threads.lock().await;
        threads.insert(
            thread_id.clone(),
            ThreadRecord {
                thread_id: thread_id.clone(),
                graph: cron.graph.clone(),
                metadata: json!({"cron_id": cron.cron_id, "trigger": "cron"}),
                created_at: Utc::now(),
            },
        );
    }

    let payload = RunPayload {
        input: cron.input.clone(),
        metadata: Some(json!({"cron_id": cron.cron_id})),
        ..RunPayload::default()
    };
    let fired_at = Utc::now();
    let scheduled = runs::schedule(
        &state.run_deps,
        &thread_id,
        &cron.graph,
        payload,
        MultitaskStrategy::Enqueue,
    )
    .await;

    // Bookkeeping + persistence (best effort on the write).
    match state.server_store.get_cron(&cron.cron_id).await {
        Ok(Some(mut record)) => {
            record.last_run_at = Some(fired_at);
            record.runs_fired += 1;
            if let Err(error) = state.server_store.upsert_cron(&record).await {
                tracing::warn!(cron_id = %cron.cron_id, %error, "cron persistence failed");
            }
        }
        Ok(None) => {} // deleted between listing and firing
        Err(error) => {
            tracing::warn!(cron_id = %cron.cron_id, %error, "cron bookkeeping read failed")
        }
    }

    match scheduled {
        Ok(scheduled) => {
            if cron.on_run_completed == OnRunCompleted::Delete {
                let mut terminal = scheduled.terminal;
                let _ = terminal.wait_for(|v| v.is_some()).await;
                match state.server_store.delete_cron(&cron.cron_id).await {
                    Ok(_) => {
                        tracing::info!(cron_id = %cron.cron_id, "one-shot cron deleted after run")
                    }
                    Err(error) => {
                        tracing::warn!(cron_id = %cron.cron_id, %error, "one-shot cron delete failed")
                    }
                }
            }
        }
        Err(error) => {
            tracing::warn!(cron_id = %cron.cron_id, %error, "cron run scheduling failed")
        }
    }
}
