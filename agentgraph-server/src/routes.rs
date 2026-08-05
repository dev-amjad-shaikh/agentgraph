//! HTTP handlers and application state (Agent-Protocol subset, design doc §3).

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use agentgraph::checkpoint::{Checkpoint, Checkpointer, JsonFileCheckpointer};
use agentgraph::state::State;
use axum::extract::{Path, State as AxumState};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{delete, get, post, put};
use axum::{middleware, Json, Router};
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::assistants::{self, AssistantRecord};
use crate::crons::{self, CronRecord, OnRunCompleted};
use crate::error::ApiError;
use crate::runs::{
    self, MultitaskStrategy, RunConfigPayload, RunDeps, RunManager, RunPayload, RunStatus,
};
use crate::sse;
use crate::{store, GraphRegistry, ServerConfig};

/// A thread: a conversation/session bound to one registered graph at
/// creation time (design doc §8, open question 2 — per-thread binding).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ThreadRecord {
    pub thread_id: String,
    pub graph: String,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

/// Shared application state.
pub(crate) struct AppState {
    pub registry: GraphRegistry,
    pub config: ServerConfig,
    pub checkpointer: Arc<dyn Checkpointer>,
    pub threads: Mutex<HashMap<String, ThreadRecord>>,
    pub run_deps: RunDeps,
    pub assistants: Mutex<HashMap<String, AssistantRecord>>,
    pub crons: Mutex<HashMap<String, CronRecord>>,
    /// Serializes KV-store writes so `created_at` preservation can't race.
    pub store_lock: Mutex<()>,
}

/// Build the full router (used by [`crate::router`]).
pub(crate) fn router(registry: GraphRegistry, config: ServerConfig) -> Router {
    let checkpointer: Arc<dyn Checkpointer> =
        Arc::new(JsonFileCheckpointer::new(config.store_path.clone()));
    let run_deps = RunDeps {
        registry: registry.clone(),
        checkpointer: Arc::clone(&checkpointer),
        manager: RunManager::new(),
        queue_cap: config.max_concurrent_runs_per_thread.max(1),
        log_capacity: config.event_log_capacity.max(16),
    };
    let state = Arc::new(AppState {
        registry,
        assistants: Mutex::new(assistants::load(&config.store_path)),
        crons: Mutex::new(crons::load(&config.store_path)),
        config,
        checkpointer,
        threads: Mutex::new(HashMap::new()),
        run_deps,
        store_lock: Mutex::new(()),
    });
    crons::spawn_scheduler(Arc::clone(&state));

    Router::new()
        .route("/ok", get(ok))
        .route("/info", get(info))
        .route("/threads", post(create_thread))
        .route(
            "/threads/{thread_id}/state",
            get(get_state).post(update_state),
        )
        .route("/threads/{thread_id}/history", post(history))
        .route("/threads/{thread_id}/runs", post(create_run))
        .route("/threads/{thread_id}/runs/wait", post(create_run_wait))
        .route("/threads/{thread_id}/runs/stream", post(create_run_stream))
        .route(
            "/threads/{thread_id}/runs/{run_id}",
            delete(delete_run_checkpoints),
        )
        .route("/runs/{run_id}", get(get_run))
        .route("/assistants", post(create_assistant).get(list_assistants))
        .route("/assistants/{assistant_id}", get(get_assistant))
        .route("/crons", post(create_cron).get(list_crons))
        .route("/crons/{cron_id}", delete(delete_cron))
        .route("/store/{namespace}", get(list_store_namespace))
        .route(
            "/store/{namespace}/{key}",
            put(put_store_item)
                .get(get_store_item)
                .delete(delete_store_item),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            crate::auth::require_api_key,
        ))
        .with_state(state)
}

// --------------------------------------------------------------------- //
// Helpers
// --------------------------------------------------------------------- //

fn internal_err<E: std::fmt::Display>(e: E) -> ApiError {
    ApiError::internal(e.to_string())
}

async fn require_thread(state: &AppState, thread_id: &str) -> Result<ThreadRecord, ApiError> {
    state
        .threads
        .lock()
        .await
        .get(thread_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("thread `{thread_id}` not found")))
}

fn checkpoint_ref(cp: &Checkpoint) -> Value {
    json!({
        "checkpoint_id": cp.id,
        "thread_id": cp.thread_id,
        "step": cp.step,
        "created_at": cp.created_at,
    })
}

// --------------------------------------------------------------------- //
// Liveness & info
// --------------------------------------------------------------------- //

async fn ok() -> Json<Value> {
    Json(json!({ "ok": true }))
}

async fn info(AxumState(state): AxumState<Arc<AppState>>) -> Json<Value> {
    let graphs: Vec<Value> = state
        .registry
        .names()
        .into_iter()
        .map(|name| {
            json!({
                "name": name,
                "channels": state.registry.channel_names(&name),
            })
        })
        .collect();
    Json(json!({
        "service": "agentgraph-server",
        "version": env!("CARGO_PKG_VERSION"),
        "checkpointer": "json_file",
        "store_path": state.config.store_path,
        "graphs": graphs,
    }))
}

// --------------------------------------------------------------------- //
// Threads
// --------------------------------------------------------------------- //

#[derive(Debug, Deserialize)]
struct CreateThreadPayload {
    /// Registered graph name this thread binds to.
    graph: String,
    #[serde(default)]
    metadata: Option<Value>,
    /// Client-chosen thread id (a UUID v4 is generated when omitted).
    #[serde(default)]
    thread_id: Option<String>,
}

async fn create_thread(
    AxumState(state): AxumState<Arc<AppState>>,
    Json(payload): Json<CreateThreadPayload>,
) -> Result<(StatusCode, Json<ThreadRecord>), ApiError> {
    if !state.registry.contains(&payload.graph) {
        return Err(ApiError::bad_request(format!(
            "unknown graph `{}` (see GET /info for registered graphs)",
            payload.graph
        )));
    }
    let thread_id = payload
        .thread_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut threads = state.threads.lock().await;
    if threads.contains_key(&thread_id) {
        return Err(ApiError::conflict(format!(
            "thread `{thread_id}` already exists"
        )));
    }
    let record = ThreadRecord {
        thread_id: thread_id.clone(),
        graph: payload.graph,
        metadata: payload.metadata.unwrap_or(Value::Null),
        created_at: Utc::now(),
    };
    threads.insert(thread_id, record.clone());
    Ok((StatusCode::CREATED, Json(record)))
}

// --------------------------------------------------------------------- //
// Thread state & history
// --------------------------------------------------------------------- //

async fn get_state(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(thread_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_thread(&state, &thread_id).await?;
    let latest = state
        .checkpointer
        .get_latest(&thread_id)
        .await
        .map_err(internal_err)?;
    Ok(Json(match latest {
        None => json!({ "values": {}, "next": [], "checkpoint": null }),
        Some(cp) => json!({
            "values": cp.state.to_value(),
            "next": cp.next_nodes,
            "checkpoint": checkpoint_ref(&cp),
        }),
    }))
}

#[derive(Debug, Deserialize)]
struct UpdateStatePayload {
    /// The full new state (JSON object).
    values: Value,
    /// Recorded for API compatibility with LangGraph's `update_state`;
    /// checkpoints do not carry per-node metadata in v0.1.
    #[serde(default)]
    as_node: Option<String>,
    /// Override for the next-node set (defaults to the previous value).
    #[serde(default)]
    next_nodes: Option<Vec<String>>,
}

async fn update_state(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(thread_id): Path<String>,
    Json(payload): Json<UpdateStatePayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    require_thread(&state, &thread_id).await?;
    let UpdateStatePayload {
        values,
        as_node,
        next_nodes,
    } = payload;
    let _ = as_node;

    let new_state = State::from_value(values)
        .map_err(|e| ApiError::bad_request(format!("`values` must be a JSON object: {e}")))?;
    let latest = state
        .checkpointer
        .get_latest(&thread_id)
        .await
        .map_err(internal_err)?;
    let (step, prev_next) = latest
        .map(|cp| (cp.step + 1, cp.next_nodes))
        .unwrap_or((0, Vec::new()));

    let cp = Checkpoint::new(&thread_id, step, new_state, next_nodes.unwrap_or(prev_next));
    state
        .checkpointer
        .put(cp.clone())
        .await
        .map_err(internal_err)?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "values": cp.state.to_value(),
            "next": cp.next_nodes,
            "checkpoint": checkpoint_ref(&cp),
        })),
    ))
}

#[derive(Debug, Default, Deserialize)]
struct HistoryPayload {
    #[serde(default)]
    limit: Option<usize>,
    /// Return only checkpoints older than this checkpoint id.
    #[serde(default)]
    before: Option<String>,
}

async fn history(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(thread_id): Path<String>,
    Json(payload): Json<HistoryPayload>,
) -> Result<Json<Value>, ApiError> {
    require_thread(&state, &thread_id).await?;
    let mut checkpoints = state
        .checkpointer
        .list(&thread_id)
        .await
        .map_err(internal_err)?;
    checkpoints.reverse(); // newest first

    if let Some(before) = &payload.before {
        if let Some(pos) = checkpoints.iter().position(|cp| &cp.id == before) {
            checkpoints.drain(..=pos);
        }
    }
    if let Some(limit) = payload.limit {
        checkpoints.truncate(limit);
    }

    let items: Vec<Value> = checkpoints
        .iter()
        .map(|cp| {
            json!({
                "values": cp.state.to_value(),
                "next": cp.next_nodes,
                "checkpoint": checkpoint_ref(cp),
            })
        })
        .collect();
    Ok(Json(Value::Array(items)))
}

// --------------------------------------------------------------------- //
// Runs
// --------------------------------------------------------------------- //

async fn schedule_for_thread(
    state: &Arc<AppState>,
    thread_id: &str,
    mut payload: RunPayload,
) -> Result<runs::Scheduled, ApiError> {
    let record = require_thread(state, thread_id).await?;
    if let Some(input) = &payload.input {
        if !input.is_object() {
            return Err(ApiError::bad_request(
                "`input` must be a JSON object".to_string(),
            ));
        }
    }
    if let Some(assistant_id) = &payload.assistant_id {
        let assistants = state.assistants.lock().await;
        let assistant = assistants
            .get(assistant_id)
            .ok_or_else(|| ApiError::not_found(format!("assistant `{assistant_id}` not found")))?;
        if assistant.graph != record.graph {
            return Err(ApiError::bad_request(format!(
                "assistant `{assistant_id}` is bound to graph `{}` but thread `{thread_id}` uses `{}`",
                assistant.graph, record.graph
            )));
        }
        // Assistant config supplies a default recursion limit; an explicit
        // `config.recursion_limit` on the payload wins.
        let payload_limit = payload.config.as_ref().and_then(|c| c.recursion_limit);
        if payload_limit.is_none() {
            if let Some(limit) = assistant
                .config
                .get("recursion_limit")
                .and_then(Value::as_u64)
            {
                payload
                    .config
                    .get_or_insert_with(RunConfigPayload::default)
                    .recursion_limit = Some(limit as usize);
            }
        }
    }
    let strategy = MultitaskStrategy::parse(payload.multitask_strategy.as_deref())
        .map_err(ApiError::bad_request)?;
    runs::schedule(&state.run_deps, thread_id, &record.graph, payload, strategy).await
}

/// `POST /threads/{id}/runs` — background run: `202 + run_id`.
async fn create_run(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(thread_id): Path<String>,
    Json(payload): Json<RunPayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let scheduled = schedule_for_thread(&state, &thread_id, payload).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "run_id": scheduled.run_id,
            "thread_id": thread_id,
            "status": scheduled.status.as_str(),
        })),
    ))
}

/// `POST /threads/{id}/runs/wait` — blocking run: terminal result as JSON.
async fn create_run_wait(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(thread_id): Path<String>,
    Json(payload): Json<RunPayload>,
) -> Result<Json<Value>, ApiError> {
    let scheduled = schedule_for_thread(&state, &thread_id, payload).await?;
    let mut terminal = scheduled.terminal;
    let result = terminal
        .wait_for(|v| v.is_some())
        .await
        .map_err(|_| ApiError::internal("run ended without a terminal result".to_string()))?;
    let value = result.clone().expect("wait_for predicate guarantees Some");
    Ok(Json(value))
}

/// `POST /threads/{id}/runs/stream` — run with SSE streaming.
async fn create_run_stream(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<RunPayload>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let last_seen =
        sse::parse_last_event_id(headers.get("last-event-id").and_then(|v| v.to_str().ok()));
    let scheduled = schedule_for_thread(&state, &thread_id, payload).await?;
    let stream = sse::frame_stream(scheduled.replay, scheduled.broadcast, last_seen);
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// `DELETE /threads/{id}/runs/{run_id}` — rollback: delete the checkpoints a
/// finished run created, re-anchoring the thread to the pre-run checkpoint.
async fn delete_run_checkpoints(
    AxumState(state): AxumState<Arc<AppState>>,
    Path((thread_id, run_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    require_thread(&state, &thread_id).await?;
    let info = state
        .run_deps
        .manager
        .info(&run_id)
        .await
        .ok_or_else(|| ApiError::not_found(format!("run `{run_id}` not found")))?;
    if info.thread_id != thread_id {
        return Err(ApiError::bad_request(format!(
            "run `{run_id}` does not belong to thread `{thread_id}`"
        )));
    }
    if matches!(info.status, RunStatus::Pending | RunStatus::Running) {
        return Err(ApiError::conflict(
            "run is still active; rollback applies to finished runs".to_string(),
        ));
    }

    let ids = info
        .checkpoint_ids
        .lock()
        .expect("checkpoint ids lock poisoned")
        .clone();
    let dir = state.config.store_path.join(&thread_id);
    let mut deleted = 0usize;
    for id in &ids {
        let path = dir.join(format!("{id}.json"));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => deleted += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(ApiError::internal(format!(
                    "failed to delete `{}`: {e}",
                    path.display()
                )))
            }
        }
    }

    // Re-anchor the latest pointer to the newest remaining checkpoint.
    let remaining = state
        .checkpointer
        .list(&thread_id)
        .await
        .map_err(internal_err)?;
    let latest_path = dir.join("latest");
    match remaining.last() {
        Some(cp) => tokio::fs::write(&latest_path, cp.id.as_bytes())
            .await
            .map_err(internal_err)?,
        None => {
            let _ = tokio::fs::remove_file(&latest_path).await;
        }
    }

    Ok(Json(json!({
        "run_id": run_id,
        "thread_id": thread_id,
        "deleted_checkpoints": deleted,
        "remaining_checkpoints": remaining.len(),
    })))
}

// --------------------------------------------------------------------- //
// Run status polling
// --------------------------------------------------------------------- //

/// `GET /runs/{run_id}` — poll a run's lifecycle status; once terminal, the
/// response carries the run's `output` / `error` / `interrupt` fields.
async fn get_run(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let info = state
        .run_deps
        .manager
        .info(&run_id)
        .await
        .ok_or_else(|| ApiError::not_found(format!("run `{run_id}` not found")))?;
    let mut body = json!({
        "run_id": run_id,
        "thread_id": info.thread_id,
        "graph": info.graph,
        "attempt": info.attempt,
        "status": info.status.as_str(),
    });
    if let Some(terminal) = info.terminal {
        if let (Some(body), Some(terminal)) = (body.as_object_mut(), terminal.as_object()) {
            for (key, value) in terminal {
                body.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(Json(body))
}

// --------------------------------------------------------------------- //
// Assistants
// --------------------------------------------------------------------- //

#[derive(Debug, Deserialize)]
struct CreateAssistantPayload {
    /// Human-readable name (need not be unique).
    name: String,
    /// Registered graph this assistant runs.
    graph: String,
    /// Client-chosen assistant id (a UUID v4 is generated when omitted).
    #[serde(default)]
    assistant_id: Option<String>,
    /// Free-form config metadata; `recursion_limit` is honored as a run
    /// default.
    #[serde(default)]
    config: Option<Value>,
    #[serde(default)]
    metadata: Option<Value>,
}

async fn create_assistant(
    AxumState(state): AxumState<Arc<AppState>>,
    Json(payload): Json<CreateAssistantPayload>,
) -> Result<(StatusCode, Json<AssistantRecord>), ApiError> {
    if payload.name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "`name` must not be empty".to_string(),
        ));
    }
    if !state.registry.contains(&payload.graph) {
        return Err(ApiError::bad_request(format!(
            "unknown graph `{}` (see GET /info for registered graphs)",
            payload.graph
        )));
    }
    let assistant_id = payload
        .assistant_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let record = AssistantRecord {
        assistant_id: assistant_id.clone(),
        name: payload.name,
        graph: payload.graph,
        config: payload.config.unwrap_or(Value::Null),
        metadata: payload.metadata.unwrap_or(Value::Null),
        created_at: Utc::now(),
    };
    {
        let mut assistants = state.assistants.lock().await;
        if assistants.contains_key(&assistant_id) {
            return Err(ApiError::conflict(format!(
                "assistant `{assistant_id}` already exists"
            )));
        }
        assistants.insert(assistant_id, record.clone());
    }
    assistants::persist(&state.config.store_path, &record)
        .await
        .map_err(internal_err)?;
    Ok((StatusCode::CREATED, Json(record)))
}

async fn list_assistants(AxumState(state): AxumState<Arc<AppState>>) -> Json<Value> {
    let mut records: Vec<AssistantRecord> =
        state.assistants.lock().await.values().cloned().collect();
    records.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.assistant_id.cmp(&b.assistant_id))
    });
    Json(json!(records))
}

async fn get_assistant(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(assistant_id): Path<String>,
) -> Result<Json<AssistantRecord>, ApiError> {
    state
        .assistants
        .lock()
        .await
        .get(&assistant_id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("assistant `{assistant_id}` not found")))
}

// --------------------------------------------------------------------- //
// Crons
// --------------------------------------------------------------------- //

#[derive(Debug, Deserialize)]
struct CreateCronPayload {
    /// Registered graph the fired runs execute.
    graph: String,
    /// Fixed-interval schedule in seconds (XOR `cron_expr`).
    #[serde(default)]
    interval_secs: Option<u64>,
    /// 5-field cron expression, UTC (XOR `interval_secs`).
    #[serde(default)]
    cron_expr: Option<String>,
    /// Initial state for fired runs (must be a JSON object when present).
    #[serde(default)]
    input: Option<Value>,
    /// Client-chosen cron id (a UUID v4 is generated when omitted).
    #[serde(default)]
    cron_id: Option<String>,
    #[serde(default)]
    metadata: Option<Value>,
    /// `"keep"` (default) or `"delete"` (remove the cron after its first
    /// run reaches a terminal state).
    #[serde(default)]
    on_run_completed: Option<String>,
}

async fn create_cron(
    AxumState(state): AxumState<Arc<AppState>>,
    Json(payload): Json<CreateCronPayload>,
) -> Result<(StatusCode, Json<CronRecord>), ApiError> {
    if !state.registry.contains(&payload.graph) {
        return Err(ApiError::bad_request(format!(
            "unknown graph `{}` (see GET /info for registered graphs)",
            payload.graph
        )));
    }
    crons::validate_schedule(payload.interval_secs, payload.cron_expr.as_deref())
        .map_err(ApiError::bad_request)?;
    if let Some(input) = &payload.input {
        if !input.is_object() {
            return Err(ApiError::bad_request(
                "`input` must be a JSON object".to_string(),
            ));
        }
    }
    let on_run_completed = OnRunCompleted::parse(payload.on_run_completed.as_deref())
        .map_err(ApiError::bad_request)?;
    let cron_id = payload
        .cron_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let record = CronRecord {
        cron_id: cron_id.clone(),
        graph: payload.graph,
        interval_secs: payload.interval_secs,
        cron_expr: payload.cron_expr,
        input: payload.input,
        metadata: payload.metadata.unwrap_or(Value::Null),
        on_run_completed,
        created_at: Utc::now(),
        last_run_at: None,
        runs_fired: 0,
    };
    {
        let mut crons_map = state.crons.lock().await;
        if crons_map.contains_key(&cron_id) {
            return Err(ApiError::conflict(format!(
                "cron `{cron_id}` already exists"
            )));
        }
        crons_map.insert(cron_id, record.clone());
    }
    crons::persist(&state.config.store_path, &record)
        .await
        .map_err(internal_err)?;
    Ok((StatusCode::CREATED, Json(record)))
}

async fn list_crons(AxumState(state): AxumState<Arc<AppState>>) -> Json<Value> {
    let mut records: Vec<CronRecord> = state.crons.lock().await.values().cloned().collect();
    records.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.cron_id.cmp(&b.cron_id))
    });
    Json(json!(records))
}

async fn delete_cron(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(cron_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if crons::remove(&state, &cron_id).await {
        Ok(Json(json!({ "cron_id": cron_id, "deleted": true })))
    } else {
        Err(ApiError::not_found(format!("cron `{cron_id}` not found")))
    }
}

// --------------------------------------------------------------------- //
// Store (cross-thread KV)
// --------------------------------------------------------------------- //

async fn put_store_item(
    AxumState(state): AxumState<Arc<AppState>>,
    Path((namespace, key)): Path<(String, String)>,
    Json(value): Json<Value>,
) -> Result<(StatusCode, Json<store::StoreItem>), ApiError> {
    store::validate_segment("namespace", &namespace)?;
    store::validate_segment("key", &key)?;
    let _guard = state.store_lock.lock().await;
    let (item, created) = store::put(&state.config.store_path, &namespace, &key, value)
        .await
        .map_err(internal_err)?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(item)))
}

async fn get_store_item(
    AxumState(state): AxumState<Arc<AppState>>,
    Path((namespace, key)): Path<(String, String)>,
) -> Result<Json<store::StoreItem>, ApiError> {
    store::validate_segment("namespace", &namespace)?;
    store::validate_segment("key", &key)?;
    store::get(&state.config.store_path, &namespace, &key)
        .await
        .map_err(internal_err)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no store item at `{namespace}/{key}`")))
}

async fn delete_store_item(
    AxumState(state): AxumState<Arc<AppState>>,
    Path((namespace, key)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    store::validate_segment("namespace", &namespace)?;
    store::validate_segment("key", &key)?;
    let _guard = state.store_lock.lock().await;
    if store::delete(&state.config.store_path, &namespace, &key)
        .await
        .map_err(internal_err)?
    {
        Ok(Json(
            json!({ "namespace": namespace, "key": key, "deleted": true }),
        ))
    } else {
        Err(ApiError::not_found(format!(
            "no store item at `{namespace}/{key}`"
        )))
    }
}

async fn list_store_namespace(
    AxumState(state): AxumState<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> Result<Json<Value>, ApiError> {
    store::validate_segment("namespace", &namespace)?;
    let items = store::list(&state.config.store_path, &namespace)
        .await
        .map_err(internal_err)?;
    Ok(Json(json!(items)))
}
