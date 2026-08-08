//! HTTP handlers and application state (Agent-Protocol subset, design doc §3).

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State as AxumState};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{middleware, Extension, Json, Router};
use chrono::Utc;
use futures::Stream;
use rusty_agent_runtime::checkpoint::{
    Checkpoint, Checkpointer, InMemoryCheckpointer, JsonFileCheckpointer,
};
use rusty_agent_runtime::journal::{Clock, Journal, JournalSnapshot, RngSource};
use rusty_agent_runtime::record::RunEventKind;
use rusty_agent_runtime::replay::{BranchDiff, ExactReplay, ReplayFixture, ReplayParams};
use rusty_agent_runtime::state::State;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::assistants::AssistantRecord;
use crate::auth::TenantContext;
use crate::crons::{self, CronRecord, OnRunCompleted};
use crate::error::ApiError;
use crate::runs::{
    self, MultitaskStrategy, RunConfigPayload, RunDeps, RunManager, RunPayload, RunStatus,
};
use crate::server_store::{JsonFileStore, ServerStore};
use crate::sse;
use crate::tasks::{self, CancelOutcome, MutationOutcome, TaskRecord, TaskStatus};
use crate::threads::ThreadRecord;
use crate::{store, GraphRegistry, ServerConfig, RESERVED_NAMES};

/// Shared application state.
pub(crate) struct AppState {
    pub registry: GraphRegistry,
    pub config: ServerConfig,
    pub checkpointer: Arc<dyn Checkpointer>,
    pub run_deps: RunDeps,
    /// Assistants / crons / threads / KV persistence (JSON files or
    /// Postgres). Thread records live here — not in a route-local map — so
    /// they survive restarts alongside their checkpoints.
    pub server_store: Arc<dyn ServerStore>,
    /// Per-thread locks serializing `update_state`'s read-modify-write:
    /// without one, two concurrent writes could mint the same `step`.
    pub state_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// Build the checkpointer + server-store backends for `config`. The default
/// is JSON files under `store_path`; `ServerConfig::with_postgres(url)`
/// (feature `postgres`) switches both to Postgres. Postgres connections are
/// established lazily on first use, keeping this builder synchronous.
fn build_backends(config: &ServerConfig) -> (Arc<dyn Checkpointer>, Arc<dyn ServerStore>) {
    #[cfg(feature = "postgres")]
    if let Some(url) = &config.database_url {
        return (
            Arc::new(crate::server_store::LazyPostgresCheckpointer::new(
                url.clone(),
            )),
            Arc::new(crate::server_store::PostgresStore::new(url.clone())),
        );
    }
    #[cfg(not(feature = "postgres"))]
    assert!(
        config.database_url.is_none(),
        "`ServerConfig::database_url` requires the `postgres` feature \
         (rebuild rusty-server with `--features postgres`)"
    );
    (
        Arc::new(JsonFileCheckpointer::new(config.store_path.clone())),
        Arc::new(JsonFileStore::load(&config.store_path)),
    )
}

/// Build the full router (used by [`crate::router`]).
pub(crate) fn router(registry: GraphRegistry, config: ServerConfig) -> Router {
    let (checkpointer, server_store) = build_backends(&config);
    let run_deps = RunDeps {
        registry: registry.clone(),
        checkpointer: Arc::clone(&checkpointer),
        manager: RunManager::new(),
        server_store: Arc::clone(&server_store),
        queue_cap: config.max_concurrent_runs_per_thread.max(1),
        log_capacity: config.event_log_capacity.max(16),
    };
    let state = Arc::new(AppState {
        registry,
        config,
        checkpointer,
        run_deps,
        server_store,
        state_locks: Mutex::new(HashMap::new()),
    });
    crons::spawn_scheduler(Arc::clone(&state));

    Router::new()
        .route("/ok", get(ok))
        .route("/info", get(info))
        .route("/threads", post(create_thread))
        .route("/threads/{thread_id}/fork", post(fork_thread))
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
        .route("/runs/{run_id}/cancel", post(cancel_run))
        .route("/runs/{run_id}/stream", get(get_run_stream))
        .route("/runs/{run_id}/events", get(get_run_events))
        .route("/runs/{run_id}/fixture", get(get_run_fixture))
        .route("/runs/replay", post(replay_run))
        .route("/runs/diff", get(diff_runs))
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
        .route("/tasks", post(enqueue_task).get(list_tasks))
        .route("/tasks/claim", post(claim_task))
        .route("/tasks/{task_id}", get(get_task))
        .route("/tasks/{task_id}/heartbeat", post(heartbeat_task))
        .route("/tasks/{task_id}/complete", post(complete_task))
        .route("/tasks/{task_id}/fail", post(fail_task))
        .route("/tasks/{task_id}/cancel", post(cancel_task))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            crate::auth::require_api_key,
        ))
        // Outermost layer: permissive CORS so browser clients (e.g. the
        // Studio) can call the API from any origin, and OPTIONS preflights
        // are answered before the API-key middleware runs. Production
        // deployments should replace this with a restrictive `CorsLayer`.
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state)
}

// --------------------------------------------------------------------- //
// Helpers
// --------------------------------------------------------------------- //

fn internal_err<E: std::fmt::Display>(e: E) -> ApiError {
    ApiError::internal(e.to_string())
}

/// Fetch the caller's thread record by external id. Lookup happens under
/// the tenant's internal id namespace, so another tenant's thread simply
/// does not exist here — cross-tenant access answers 404 (never 403, to
/// avoid leaking the thread's existence).
async fn require_thread(
    state: &AppState,
    tenant: &TenantContext,
    thread_id: &str,
) -> Result<ThreadRecord, ApiError> {
    state
        .server_store
        .get_thread(&tenant.scope(thread_id))
        .await
        .map_err(internal_err)?
        .ok_or_else(|| ApiError::not_found(format!("thread `{thread_id}` not found")))
}

/// Validate a client-chosen resource id (thread / assistant / cron). Ids
/// become path segments under the store root and carry a `{tenant}/`
/// prefix internally, so they must be non-empty, bounded, and free of path
/// separators; all-dots ids are rejected (parent-directory components), as
/// are the reserved layout names in [`RESERVED_NAMES`] — an id of `crons`
/// would otherwise write checkpoint files into the cron-records directory.
fn validate_client_id(kind: &str, id: &str) -> Result<(), ApiError> {
    let ok = !id.is_empty()
        && id.len() <= 256
        && !id.contains('/')
        && !id.contains('\\')
        && !id.chars().all(|c| c == '.')
        && !RESERVED_NAMES.contains(&id);
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "invalid {kind} `{id}` (must be non-empty, <= 256 chars, no path separators, not a reserved name)"
        )))
    }
}

/// The per-thread lock serializing `update_state` (see
/// [`AppState::state_locks`]).
async fn state_lock(state: &AppState, internal_id: &str) -> Arc<Mutex<()>> {
    state
        .state_locks
        .lock()
        .await
        .entry(internal_id.to_string())
        .or_default()
        .clone()
}

fn checkpoint_ref(cp: &Checkpoint, tenant: &TenantContext) -> Value {
    json!({
        "checkpoint_id": cp.id,
        // Checkpoints persist the internal (tenant-scoped) thread id; the
        // wire always shows the external one.
        "thread_id": tenant.unscope(&cp.thread_id).unwrap_or(&cp.thread_id),
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
    let persistence = if state.config.database_url.is_some() {
        "postgres"
    } else {
        "json_file"
    };
    Json(json!({
        "service": "rusty-server",
        "version": env!("CARGO_PKG_VERSION"),
        "checkpointer": persistence,
        "server_store": persistence,
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
    Extension(tenant): Extension<TenantContext>,
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
    validate_client_id("thread_id", &thread_id)?;

    let internal_id = tenant.scope(&thread_id);
    let record = ThreadRecord {
        thread_id: thread_id.clone(),
        tenant: tenant.tenant().to_string(),
        graph: payload.graph,
        metadata: payload.metadata.unwrap_or(Value::Null),
        created_at: Utc::now(),
    };
    // Check-and-insert in the store (durable, so pre-restart checkpoints
    // stay reachable through the API).
    let created = state
        .server_store
        .create_thread(&internal_id, &record)
        .await
        .map_err(internal_err)?;
    if !created {
        return Err(ApiError::conflict(format!(
            "thread `{thread_id}` already exists"
        )));
    }
    Ok((StatusCode::CREATED, Json(record)))
}

// --------------------------------------------------------------------- //
// Thread fork (time travel)
// --------------------------------------------------------------------- //

#[derive(Debug, Deserialize)]
struct ForkThreadPayload {
    /// Client-chosen id for the fork (a UUID v4 is generated when omitted).
    #[serde(default)]
    new_thread_id: Option<String>,
    /// Fork from this checkpoint: only checkpoints up to and including it
    /// are copied. Omit to copy the full history.
    #[serde(default)]
    checkpoint_id: Option<String>,
}

/// `POST /threads/{id}/fork` — copy the thread's checkpoint history (full,
/// or up to `checkpoint_id`) into a new thread bound to the same graph, via
/// [`Checkpointer::fork_thread`]. The fork is the safe time-travel target:
/// replay it with `"checkpoint": {"checkpoint_id": …}` on run-create.
async fn fork_thread(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(thread_id): Path<String>,
    Json(payload): Json<ForkThreadPayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let record = require_thread(&state, &tenant, &thread_id).await?;
    let new_thread_id = payload
        .new_thread_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    validate_client_id("new_thread_id", &new_thread_id)?;

    let new_internal_id = tenant.scope(&new_thread_id);
    if state
        .server_store
        .get_thread(&new_internal_id)
        .await
        .map_err(internal_err)?
        .is_some()
    {
        return Err(ApiError::conflict(format!(
            "thread `{new_thread_id}` already exists"
        )));
    }

    // Fork inside the tenant's checkpoint namespace.
    let copied = state
        .checkpointer
        .fork_thread(
            &tenant.scope(&thread_id),
            &new_internal_id,
            payload.checkpoint_id.as_deref(),
        )
        .await
        .map_err(|e| {
            let message = e.to_string();
            if message.contains("unknown checkpoint id") {
                ApiError::not_found(message)
            } else {
                // No checkpoints to fork, or src == dst id collision.
                ApiError::bad_request(message)
            }
        })?;

    let fork = ThreadRecord {
        thread_id: new_thread_id.clone(),
        tenant: tenant.tenant().to_string(),
        graph: record.graph,
        metadata: json!({
            "forked_from": thread_id,
            "fork_checkpoint_id": payload.checkpoint_id,
        }),
        created_at: Utc::now(),
    };
    // A create that loses a same-id race answers 409 (the existence check
    // above is only the fast path; the store's check-and-insert is
    // authoritative).
    let created = state
        .server_store
        .create_thread(&new_internal_id, &fork)
        .await
        .map_err(internal_err)?;
    if !created {
        return Err(ApiError::conflict(format!(
            "thread `{new_thread_id}` already exists"
        )));
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "thread_id": new_thread_id,
            "checkpoints_copied": copied,
        })),
    ))
}

// --------------------------------------------------------------------- //
// Thread state & history
// --------------------------------------------------------------------- //

async fn get_state(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(thread_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_thread(&state, &tenant, &thread_id).await?;
    let latest = state
        .checkpointer
        .get_latest(&tenant.scope(&thread_id))
        .await
        .map_err(internal_err)?;
    Ok(Json(match latest {
        None => json!({ "values": {}, "next": [], "checkpoint": null }),
        Some(cp) => json!({
            "values": cp.state.to_value(),
            "next": cp.next_nodes,
            "checkpoint": checkpoint_ref(&cp, &tenant),
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
    Extension(tenant): Extension<TenantContext>,
    Path(thread_id): Path<String>,
    Json(payload): Json<UpdateStatePayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    require_thread(&state, &tenant, &thread_id).await?;
    let UpdateStatePayload {
        values,
        as_node,
        next_nodes,
    } = payload;
    let _ = as_node;

    let internal_id = tenant.scope(&thread_id);
    let new_state = State::from_value(values)
        .map_err(|e| ApiError::bad_request(format!("`values` must be a JSON object: {e}")))?;
    // Serialize the read-modify-write per thread: two concurrent
    // `update_state` calls must not mint two checkpoints with the same
    // `step`. (Held across the checkpointer IO on purpose — this is a
    // per-thread serializer, not a global lock.)
    let lock = state_lock(&state, &internal_id).await;
    let _guard = lock.lock().await;
    let latest = state
        .checkpointer
        .get_latest(&internal_id)
        .await
        .map_err(internal_err)?;
    let (step, prev_next) = latest
        .map(|cp| (cp.step + 1, cp.next_nodes))
        .unwrap_or((0, Vec::new()));

    let cp = Checkpoint::new(
        &internal_id,
        step,
        new_state,
        next_nodes.unwrap_or(prev_next),
    );
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
            "checkpoint": checkpoint_ref(&cp, &tenant),
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
    Extension(tenant): Extension<TenantContext>,
    Path(thread_id): Path<String>,
    Json(payload): Json<HistoryPayload>,
) -> Result<Json<Value>, ApiError> {
    require_thread(&state, &tenant, &thread_id).await?;
    let mut checkpoints = state
        .checkpointer
        .list(&tenant.scope(&thread_id))
        .await
        .map_err(internal_err)?;
    checkpoints.reverse(); // newest first

    if let Some(before) = &payload.before {
        match checkpoints.iter().position(|cp| &cp.id == before) {
            Some(pos) => {
                checkpoints.drain(..=pos);
            }
            // A cursor that silently resets to the full history sends
            // paginating clients into infinite loops — answer 400 instead.
            None => {
                return Err(ApiError::bad_request(format!(
                    "unknown `before` checkpoint `{before}`"
                )));
            }
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
                "checkpoint": checkpoint_ref(cp, &tenant),
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
    tenant: &TenantContext,
    thread_id: &str,
    mut payload: RunPayload,
) -> Result<runs::Scheduled, ApiError> {
    let record = require_thread(state, tenant, thread_id).await?;
    let internal_id = tenant.scope(thread_id);
    if let Some(input) = &payload.input {
        if !input.is_object() {
            return Err(ApiError::bad_request(
                "`input` must be a JSON object".to_string(),
            ));
        }
    }
    if let Some(assistant_id) = &payload.assistant_id {
        // The id arrives in a JSON body, not a path segment, so it must be
        // validated here like every other client-chosen id: the default
        // tenant's `scope()` is the identity function, and an unvalidated
        // `"tenant/id"` value would resolve (and run) another tenant's
        // assistant record.
        validate_client_id("assistant_id", assistant_id)?;
        // Assistants are tenant-scoped: another tenant's assistant id
        // resolves to nothing here → 404.
        let assistant = state
            .server_store
            .get_assistant(&tenant.scope(assistant_id))
            .await
            .map_err(internal_err)?
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
    if let Some(checkpoint) = &payload.checkpoint {
        // Time travel: the checkpoint must exist on this thread, or the
        // replay would fail deep inside the executor — answer 404 up front.
        let found = state
            .checkpointer
            .get_by_id(&internal_id, &checkpoint.checkpoint_id)
            .await
            .map_err(internal_err)?;
        if found.is_none() {
            return Err(ApiError::not_found(format!(
                "thread `{thread_id}` has no checkpoint `{}`",
                checkpoint.checkpoint_id
            )));
        }
    }
    let strategy = MultitaskStrategy::parse(payload.multitask_strategy.as_deref())
        .map_err(ApiError::bad_request)?;
    runs::schedule(
        &state.run_deps,
        &internal_id,
        thread_id,
        &record.graph,
        payload,
        strategy,
    )
    .await
}

/// `POST /threads/{id}/runs` — background run: `202 + run_id`.
async fn create_run(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(thread_id): Path<String>,
    Json(payload): Json<RunPayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let scheduled = schedule_for_thread(&state, &tenant, &thread_id, payload).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "run_id": scheduled.run_id,
            "thread_id": thread_id,
            "status": scheduled.status.as_str(),
        })),
    ))
}

/// Server-side ceiling for the blocking wait endpoint: a graph that never
/// terminates must not pin the handler task forever. The run itself keeps
/// executing — only the wait is bounded.
const MAX_RUN_WAIT: Duration = Duration::from_secs(3600);

/// `POST /threads/{id}/runs/wait` — blocking run: terminal result as JSON.
async fn create_run_wait(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(thread_id): Path<String>,
    Json(payload): Json<RunPayload>,
) -> Result<Json<Value>, ApiError> {
    let scheduled = schedule_for_thread(&state, &tenant, &thread_id, payload).await?;
    let mut terminal = scheduled.terminal;
    let result = tokio::time::timeout(MAX_RUN_WAIT, terminal.wait_for(|v| v.is_some()))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                format!(
                    "run did not reach a terminal state within {}s",
                    MAX_RUN_WAIT.as_secs()
                ),
            )
        })?
        .map_err(|_| ApiError::internal("run ended without a terminal result".to_string()))?;
    let value = result.clone().expect("wait_for predicate guarantees Some");
    Ok(Json(value))
}

/// Shared SSE response assembly for the two streaming endpoints.
fn sse_response(
    replay: Vec<runs::SseFrame>,
    broadcast: tokio::sync::broadcast::Receiver<runs::SseFrame>,
    skip_through_seq: u64,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    Sse::new(sse::frame_stream(replay, broadcast, skip_through_seq)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// `POST /threads/{id}/runs/stream` — run with SSE streaming. A fresh run
/// starts a new frame sequence, so `Last-Event-ID` is deliberately ignored
/// here (a stale value from a previous run would silently drop the new
/// run's first frames); replay lives on `GET /runs/{id}/stream`.
async fn create_run_stream(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(thread_id): Path<String>,
    Json(payload): Json<RunPayload>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let scheduled = schedule_for_thread(&state, &tenant, &thread_id, payload).await?;
    Ok(sse_response(scheduled.replay, scheduled.broadcast, 0))
}

/// `GET /runs/{id}/stream` — attach to an existing run's SSE stream:
/// replays the event log (honoring `Last-Event-ID`, so a reconnecting
/// client skips frames it has already seen) and then follows live frames.
/// Cross-tenant runs answer 404, like `GET /runs/{id}`.
async fn get_run_stream(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (replay, broadcast, internal_thread_id) = state
        .run_deps
        .manager
        .stream_parts(&run_id)
        .await
        .ok_or_else(|| ApiError::not_found(format!("run `{run_id}` not found")))?;
    if !tenant.owns(&internal_thread_id) {
        return Err(ApiError::not_found(format!("run `{run_id}` not found")));
    }
    let last_seen =
        sse::parse_last_event_id(headers.get("last-event-id").and_then(|v| v.to_str().ok()));
    Ok(sse_response(replay, broadcast, last_seen))
}

/// `DELETE /threads/{id}/runs/{run_id}` — rollback: delete the checkpoints a
/// finished run created, re-anchoring the thread to the pre-run checkpoint.
///
/// The `Checkpointer` trait has no delete operation, so removal goes
/// through the JSON-file layout directly; on the Postgres backend the
/// endpoint answers 409 rather than silently deleting nothing.
async fn delete_run_checkpoints(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path((thread_id, run_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    require_thread(&state, &tenant, &thread_id).await?;
    let info = state
        .run_deps
        .manager
        .info(&run_id)
        .await
        .ok_or_else(|| ApiError::not_found(format!("run `{run_id}` not found")))?;
    // Cross-tenant runs are invisible (404, not 403).
    if !tenant.owns(&info.thread_id) {
        return Err(ApiError::not_found(format!("run `{run_id}` not found")));
    }
    if info.wire_thread_id != thread_id {
        return Err(ApiError::bad_request(format!(
            "run `{run_id}` does not belong to thread `{thread_id}`"
        )));
    }
    if matches!(info.status, RunStatus::Pending | RunStatus::Running) {
        return Err(ApiError::conflict(
            "run is still active; rollback applies to finished runs".to_string(),
        ));
    }
    if state.config.database_url.is_some() {
        return Err(ApiError::conflict(
            "rollback is not supported with the Postgres checkpointer".to_string(),
        ));
    }

    let internal_id = tenant.scope(&thread_id);
    // Mutual exclusion with scheduling: a queued or newly-started run
    // could be executing from the very checkpoints this endpoint deletes.
    if state.run_deps.manager.thread_busy(&internal_id).await {
        return Err(ApiError::conflict(
            "thread has an active or queued run; rollback applies to idle threads".to_string(),
        ));
    }

    let ids = runs::lock_recover(&info.checkpoint_ids).clone();
    // Rollback is only well-defined when the run's checkpoints are the
    // tail of the current history: deleting mid-history checkpoints would
    // punch holes while the endpoint claims to re-anchor the thread to
    // the pre-run checkpoint.
    let history = state
        .checkpointer
        .list(&internal_id)
        .await
        .map_err(internal_err)?;
    let is_suffix = history.len() >= ids.len()
        && history[history.len() - ids.len()..]
            .iter()
            .map(|cp| cp.id.as_str())
            .eq(ids.iter().map(String::as_str));
    if !is_suffix {
        return Err(ApiError::conflict(
            "the run's checkpoints are not the latest on this thread; \
             rollback would punch holes mid-history"
                .to_string(),
        ));
    }

    let dir = state.config.store_path.join(&internal_id);
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

    // Re-anchor the latest pointer to the newest remaining checkpoint,
    // with the same atomic temp+rename discipline the checkpointer itself
    // uses (a crash mid-write must not leave a truncated pointer).
    let remaining = state
        .checkpointer
        .list(&internal_id)
        .await
        .map_err(internal_err)?;
    let latest_path = dir.join("latest");
    match remaining.last() {
        Some(cp) => atomic_write(&latest_path, cp.id.as_bytes())
            .await
            .map_err(internal_err)?,
        None => match tokio::fs::remove_file(&latest_path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %latest_path.display(), %e, "failed to remove latest pointer")
            }
        },
    }

    Ok(Json(json!({
        "run_id": run_id,
        "thread_id": thread_id,
        "deleted_checkpoints": deleted,
        "remaining_checkpoints": remaining.len(),
    })))
}

/// Write `bytes` to `path` atomically (temp file + rename), mirroring the
/// checkpointer's durability discipline for its `latest` pointer.
async fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

// --------------------------------------------------------------------- //
// Run status polling
// --------------------------------------------------------------------- //

/// `GET /runs/{run_id}` — poll a run's lifecycle status; once terminal, the
/// response carries the run's `output` / `error` / `interrupt` fields.
/// Runs are tenant-scoped through their thread: a run whose thread belongs
/// to another tenant answers 404.
async fn get_run(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let info = state
        .run_deps
        .manager
        .info(&run_id)
        .await
        .ok_or_else(|| ApiError::not_found(format!("run `{run_id}` not found")))?;
    if !tenant.owns(&info.thread_id) {
        return Err(ApiError::not_found(format!("run `{run_id}` not found")));
    }
    let mut body = json!({
        "run_id": run_id,
        "thread_id": info.wire_thread_id,
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

/// `POST /runs/{run_id}/cancel` — propagate cancellation into the run's
/// outstanding durable tasks: every non-terminal task enqueued with this
/// `run_id` in the caller's tenant. Queued and retry-scheduled tasks move
/// to the terminal `cancelled` state (reported under `cancelled`); leased
/// tasks keep their leases with `cancel_requested` set so their holders
/// abort and report (`signalled`). Run resolution and tenant scoping
/// follow `GET /runs/{id}` — unknown or cross-tenant runs answer 404.
///
/// Scope note: this wave wires run cancellation to the *queue*. Stopping
/// the run's in-process executor is the drain half of wave 2; a task
/// enqueued after this call is not retroactively cancelled.
async fn cancel_run(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let info = state
        .run_deps
        .manager
        .info(&run_id)
        .await
        .ok_or_else(|| ApiError::not_found(format!("run `{run_id}` not found")))?;
    if !tenant.owns(&info.thread_id) {
        return Err(ApiError::not_found(format!("run `{run_id}` not found")));
    }
    let outcome = state
        .server_store
        .cancel_run_tasks(tenant.tenant(), &run_id, Utc::now())
        .await
        .map_err(internal_err)?;
    let ids =
        |tasks: Vec<TaskRecord>| -> Vec<String> { tasks.into_iter().map(|t| t.task_id).collect() };
    Ok(Json(json!({
        "run_id": run_id,
        "cancelled": ids(outcome.cancelled),
        "signalled": ids(outcome.signalled),
    })))
}

// --------------------------------------------------------------------- //
// Flight Recorder
// --------------------------------------------------------------------- //

/// A run's Flight Recorder evidence plus the metadata the read endpoints
/// need, resolved from the live run manager while the run lives in this
/// process and from the durable store otherwise.
struct RunEvidence {
    /// The graph the run executed (manager record, or the thread's binding).
    graph: String,
    /// Internal (tenant-scoped) thread id, for checkpoint read-backs.
    internal_thread_id: String,
    /// External thread id — the only form that may appear on the wire.
    wire_thread_id: String,
    /// The run's persisted journal, integrity re-verified on read. `None`
    /// when the run is known but nothing was persisted yet (queued, or
    /// before its first checkpoint boundary).
    journal: Option<JournalSnapshot>,
    /// Ids of the checkpoints the run wrote, in write order (from the
    /// manager's bookkeeping, or recovered from the journal's
    /// `checkpoint_written` events on the store path).
    checkpoint_ids: Vec<String>,
    /// `true` when the served journal is final: the run is terminal per the
    /// manager, or the manager no longer knows the run at all — evicted after
    /// termination or lost with a process restart; either way no live writer
    /// remains, so the persisted snapshot cannot grow.
    complete: bool,
}

/// Re-verify a stored snapshot's chained head hash before it is served or
/// replayed (via [`Journal::from_snapshot`]): tampered or corrupt evidence
/// answers 500 rather than being served as fact.
fn reverify_journal(run_id: &str, snapshot: JournalSnapshot) -> Result<JournalSnapshot, ApiError> {
    Journal::from_snapshot(snapshot.clone(), Clock::System).map_err(|e| {
        ApiError::internal(format!(
            "stored journal for run `{run_id}` failed its integrity check: {e}"
        ))
    })?;
    Ok(snapshot)
}

/// Resolve a run's evidence for the Flight Recorder endpoints.
///
/// Fast path: the in-memory run manager, authoritative while the run lives in
/// this process. Fallback: the server store — journals persist per run id, so
/// the evidence stays fetchable after the run's record was evicted or the
/// process restarted. The fallback's tenant check goes through the journal's
/// external thread id: looking the thread record up under the caller's tenant
/// scope doubles as the ownership proof (a cross-tenant id resolves to
/// nothing → 404, never 403) and yields the graph the run executed. A run
/// known to neither answers 404.
async fn run_evidence(
    state: &AppState,
    tenant: &TenantContext,
    run_id: &str,
) -> Result<RunEvidence, ApiError> {
    if let Some(info) = state.run_deps.manager.info(run_id).await {
        // Cross-tenant runs are invisible (404, not 403).
        if !tenant.owns(&info.thread_id) {
            return Err(ApiError::not_found(format!("run `{run_id}` not found")));
        }
        let journal = state
            .server_store
            .get_journal(run_id)
            .await
            .map_err(internal_err)?
            .map(|snapshot| reverify_journal(run_id, snapshot))
            .transpose()?;
        return Ok(RunEvidence {
            graph: info.graph,
            internal_thread_id: info.thread_id,
            wire_thread_id: info.wire_thread_id,
            journal,
            checkpoint_ids: runs::lock_recover(&info.checkpoint_ids).clone(),
            complete: matches!(
                info.status,
                RunStatus::Success | RunStatus::Interrupted | RunStatus::Error
            ),
        });
    }

    // Store fallback: the run is unknown to this process. A persisted journal
    // is the proof it existed — and the only handle on its ownership.
    let Some(snapshot) = state
        .server_store
        .get_journal(run_id)
        .await
        .map_err(internal_err)?
    else {
        return Err(ApiError::not_found(format!("run `{run_id}` not found")));
    };
    let internal_thread_id = tenant.scope(&snapshot.thread_id);
    let thread = state
        .server_store
        .get_thread(&internal_thread_id)
        .await
        .map_err(internal_err)?
        .ok_or_else(|| ApiError::not_found(format!("run `{run_id}` not found")))?;
    let journal = reverify_journal(run_id, snapshot)?;
    let checkpoint_ids = journal
        .events
        .iter()
        .filter(|event| event.kind == RunEventKind::CheckpointWritten)
        .filter_map(|event| crate::replay::resolve(&journal, event.output.as_ref()))
        .filter_map(|output| output.get("checkpoint_id")?.as_str().map(str::to_owned))
        .collect();
    Ok(RunEvidence {
        graph: thread.graph,
        internal_thread_id,
        wire_thread_id: journal.thread_id.clone(),
        journal: Some(journal),
        checkpoint_ids,
        complete: true,
    })
}

/// `GET /runs/{run_id}/events` — the run's journaled evidence (Flight
/// Recorder), as `{run_id, events, complete}`. `events` are core's
/// `RunEvent`s in `seq` order, in the exact golden-pinned wire shape
/// (`rusty-core/tests/golden/run_event.json`).
///
/// `complete` is `true` once the run is terminal, i.e. the served snapshot
/// is the run's final journal; while the run is active the snapshot trails
/// the live journal by at most one checkpoint boundary (it is flushed per
/// `CheckpointSaved` and at completion), and a queued run serves an empty
/// event list. Unknown and cross-tenant runs answer 404, exactly like
/// `GET /runs/{id}`.
///
/// Reachability ([`run_evidence`]): once the live run record is gone —
/// evicted past the retention cap, or lost with a restart — the events stay
/// fetchable from the persisted journal for as long as the store holds it,
/// served as `complete` (no live writer remains). The stored snapshot's
/// chained head hash is re-verified on every read: tampered or corrupt
/// evidence answers 500 rather than being served as fact.
async fn get_run_events(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let evidence = run_evidence(&state, &tenant, &run_id).await?;
    let events = evidence
        .journal
        .map(|snapshot| snapshot.events)
        .unwrap_or_default();
    Ok(Json(json!({
        "run_id": run_id,
        "events": events,
        "complete": evidence.complete,
    })))
}

/// `GET /runs/{run_id}/fixture` — download the run as a portable
/// [`ReplayFixture`]: the recorded journal (integrity-verified before
/// serving), the graph's topology hash, the run's final checkpoint, and
/// provenance metadata. CI replays the bundle with
/// `ReplayFixture::import`.
///
/// Same 404 / tenant-isolation semantics as `GET /runs/{id}`, and the same
/// store fallback as `GET /runs/{id}/events` ([`run_evidence`]): after run
/// eviction or a restart the fixture stays downloadable, with the final
/// checkpoint recovered from the journal's last `checkpoint_written` event.
/// A run with no persisted journal yet (still queued, or before its first
/// checkpoint boundary) answers `409` — the fixture would be empty evidence.
/// Server runs record under the system clock and OS entropy, so the fixture
/// carries no logical-clock / RNG-seed parameters: `exact_replay` sessions
/// work, byte-identical CI replay requires runs recorded with determinism
/// seams (a later wave's concern).
///
/// The served checkpoint's `thread_id` is rewritten to the external id —
/// the internal tenant-scoped id stored by the checkpointer must never
/// appear in a downloaded fixture.
async fn get_run_fixture(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(run_id): Path<String>,
) -> Result<Json<ReplayFixture>, ApiError> {
    let evidence = run_evidence(&state, &tenant, &run_id).await?;
    let snapshot = evidence.journal.ok_or_else(|| {
        ApiError::conflict(format!(
            "run `{run_id}` has no persisted journal yet (queued or pre-checkpoint)"
        ))
    })?;
    let (graph, _spec) = state.registry.get(&evidence.graph).ok_or_else(|| {
        ApiError::conflict(format!(
            "graph `{}` is no longer registered; cannot capture a fixture for run `{run_id}`",
            evidence.graph
        ))
    })?;

    let final_checkpoint = match evidence.checkpoint_ids.last() {
        Some(id) => state
            .checkpointer
            .get_by_id(&evidence.internal_thread_id, id)
            .await
            .map_err(internal_err)?
            .map(|mut cp| {
                cp.thread_id = evidence.wire_thread_id.clone();
                cp
            }),
        None => None,
    };

    let fixture = ReplayFixture::capture(
        format!("{} run {run_id}", evidence.graph),
        &graph,
        "unversioned",
        snapshot,
        final_checkpoint,
        None,
        None,
    );
    Ok(Json(fixture))
}

/// The effect kinds server-side replay cannot re-drive: journaled outbound
/// calls (model, tool, remote, WASM). Exact replay serves them from the
/// journal in CI via the replaying wrappers; re-executing the registered
/// graph would issue them live, breaking the zero-outbound guarantee.
fn carries_servable_effects(snapshot: &JournalSnapshot) -> bool {
    snapshot.events.iter().any(|event| {
        matches!(
            event.kind,
            RunEventKind::ModelCall
                | RunEventKind::ToolCall
                | RunEventKind::RemoteCall
                | RunEventKind::WasmCall
        )
    })
}

#[derive(Debug, Deserialize)]
struct ReplayRunPayload {
    /// The run to re-drive and verify.
    run_id: String,
}

/// `POST /runs/replay` — re-drive a journaled run server-side and verify the
/// replayed evidence against the recorded journal. Body: `{"run_id": "…"}`.
///
/// The replay runs the graph code registered in this process (not a
/// downloaded copy) against a throwaway in-memory checkpointer — the shared
/// checkpoint log is never touched — and answers exactly:
///
/// ```json
/// { "run_id": "…", "verified": true, "expected_events": 12,
///   "actual_events": 12, "first_divergence": null }
/// ```
///
/// `verified` is the evidence comparison of [`crate::replay`]: same event
/// kinds, nodes, sequences, effect classes, statuses, and payloads, with
/// per-run minted identity (checkpoint ids) and wall-clock measurements
/// excluded. `first_divergence` is the `seq` of the first disagreeing event
/// (or of the first recorded event the replay never produced).
///
/// Statuses: `404` unknown or cross-tenant run; `409` no persisted journal
/// yet (same as `/fixture`), or the run is still executing — replay verifies
/// a final journal; `422` when the run's graph is not registered in this
/// process, when the journal carries recorded model/tool/remote/WASM calls
/// (server-side replay cannot serve them — export the fixture and replay in
/// CI), or when the run resumed from a checkpoint (core's [`ExactReplay`]
/// rejects mid-run evidence).
async fn replay_run(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Json(payload): Json<ReplayRunPayload>,
) -> Result<Json<Value>, ApiError> {
    let run_id = payload.run_id;
    let evidence = run_evidence(&state, &tenant, &run_id).await?;
    let snapshot = evidence.journal.ok_or_else(|| {
        ApiError::conflict(format!(
            "run `{run_id}` has no persisted journal yet (queued or pre-checkpoint)"
        ))
    })?;
    if !evidence.complete {
        return Err(ApiError::conflict(format!(
            "run `{run_id}` is still executing; replay verifies a run's final journal"
        )));
    }
    let (graph, spec) = state.registry.get(&evidence.graph).ok_or_else(|| {
        ApiError::unprocessable(format!(
            "graph `{}` is not registered in this server process; cannot replay run `{run_id}`",
            evidence.graph
        ))
    })?;
    if carries_servable_effects(&snapshot) {
        return Err(ApiError::unprocessable(format!(
            "run `{run_id}` journaled model/tool/remote/WASM calls; server-side replay \
             re-executes node code and cannot serve recorded effects — download the fixture \
             (GET /runs/{run_id}/fixture) and replay it in CI with ReplayFixture"
        )));
    }
    // Pre-check the boundary ExactReplay::new enforces, so unreplayable
    // evidence answers 422 (client-actionable), not a 500.
    if snapshot
        .events
        .first()
        .is_some_and(|event| event.kind == RunEventKind::Resume)
    {
        return Err(ApiError::unprocessable(format!(
            "run `{run_id}` resumed from a checkpoint; its journal begins mid-run against \
             state it does not carry — replay the original run's journal instead"
        )));
    }
    let replay = ExactReplay::new(snapshot.clone()).map_err(|e| {
        ApiError::internal(format!(
            "stored journal for run `{run_id}` failed its integrity check: {e}"
        ))
    })?;

    let initial = crate::replay::initial_state_from(&snapshot);
    let journal = replay.fresh_journal(Clock::System);
    let params = ReplayParams::new(journal.clone(), RngSource::default())
        .with_checkpointer(Arc::new(InMemoryCheckpointer::new()));
    // A replay error (graph code changed and now fails, a reducer rejects an
    // update, …) is divergence evidence, not an HTTP error: whatever the
    // replay journaled before stopping is compared below.
    let _ = replay.run(&graph, &spec, initial, params).await;
    let replayed = journal.snapshot();
    let report = crate::replay::compare_journals(&snapshot, &replayed);
    Ok(Json(json!({
        "run_id": run_id,
        "verified": report.verified,
        "expected_events": snapshot.events.len(),
        "actual_events": replayed.events.len(),
        "first_divergence": report.first_divergence,
    })))
}

#[derive(Debug, Deserialize)]
struct DiffQuery {
    /// Base run id (the branch is diffed against it).
    base: String,
    /// Branch run id.
    branch: String,
}

/// The run's persisted journal for the diff/replay endpoints: 409 when the
/// run is known but nothing was persisted yet.
async fn require_journal(
    state: &AppState,
    tenant: &TenantContext,
    run_id: &str,
) -> Result<JournalSnapshot, ApiError> {
    let evidence = run_evidence(state, tenant, run_id).await?;
    evidence.journal.ok_or_else(|| {
        ApiError::conflict(format!(
            "run `{run_id}` has no persisted journal yet (queued or pre-checkpoint)"
        ))
    })
}

/// `GET /runs/diff?base=<run_id>&branch=<run_id>` — the structural diff of
/// two runs' journals, in core's [`BranchDiff`] serde shape as-is:
/// `first_divergent_seq`, the events `added` (branch) and `removed` (base)
/// at and after the divergence point, per-super-step state-channel
/// `step_diffs`, and token/cost `base_totals` / `branch_totals`. Events
/// compare logically — identity and timing fields excluded — so two branches
/// of one fork show their shared prefix as equal.
///
/// 404 semantics are the usual ones (unknown or cross-tenant run on either
/// side, via [`run_evidence`] — including the post-eviction / post-restart
/// store fallback); `409` when either run has no persisted journal yet.
async fn diff_runs(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<BranchDiff>, ApiError> {
    let base = require_journal(&state, &tenant, &query.base).await?;
    let branch = require_journal(&state, &tenant, &query.branch).await?;
    Ok(Json(BranchDiff::between(&base, &branch)))
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
    Extension(tenant): Extension<TenantContext>,
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
    validate_client_id("assistant_id", &assistant_id)?;

    // Persist under the tenant's internal id; the wire shows the external id.
    let record = AssistantRecord {
        assistant_id: tenant.scope(&assistant_id),
        name: payload.name,
        graph: payload.graph,
        config: payload.config.unwrap_or(Value::Null),
        metadata: payload.metadata.unwrap_or(Value::Null),
        created_at: Utc::now(),
    };
    let created = state
        .server_store
        .create_assistant(&record)
        .await
        .map_err(internal_err)?;
    if !created {
        return Err(ApiError::conflict(format!(
            "assistant `{assistant_id}` already exists"
        )));
    }
    let mut wire = record;
    wire.assistant_id = assistant_id;
    Ok((StatusCode::CREATED, Json(wire)))
}

async fn list_assistants(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
) -> Result<Json<Value>, ApiError> {
    let records = state
        .server_store
        .list_assistants()
        .await
        .map_err(internal_err)?;
    // Only this tenant's assistants, reported with their external ids.
    let mut records: Vec<AssistantRecord> = records
        .into_iter()
        .filter_map(|mut record| {
            let external = tenant.unscope(&record.assistant_id)?.to_string();
            record.assistant_id = external;
            Some(record)
        })
        .collect();
    records.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.assistant_id.cmp(&b.assistant_id))
    });
    Ok(Json(json!(records)))
}

async fn get_assistant(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(assistant_id): Path<String>,
) -> Result<Json<AssistantRecord>, ApiError> {
    state
        .server_store
        .get_assistant(&tenant.scope(&assistant_id))
        .await
        .map_err(internal_err)?
        .map(|mut record| {
            record.assistant_id = assistant_id.clone();
            Json(record)
        })
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
    Extension(tenant): Extension<TenantContext>,
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
    validate_client_id("cron_id", &cron_id)?;

    // Persist under the tenant's internal id (same scoping as assistants);
    // the wire shows the external id and the scheduler derives the owning
    // tenant back from the prefix.
    let record = CronRecord {
        cron_id: tenant.scope(&cron_id),
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
    let created = state
        .server_store
        .create_cron(&record)
        .await
        .map_err(internal_err)?;
    if !created {
        return Err(ApiError::conflict(format!(
            "cron `{cron_id}` already exists"
        )));
    }
    let mut wire = record;
    wire.cron_id = cron_id;
    Ok((StatusCode::CREATED, Json(wire)))
}

async fn list_crons(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
) -> Result<Json<Value>, ApiError> {
    let records = state
        .server_store
        .list_crons()
        .await
        .map_err(internal_err)?;
    // Only this tenant's crons, reported with their external ids.
    let mut records: Vec<CronRecord> = records
        .into_iter()
        .filter_map(|mut record| {
            let external = tenant.unscope(&record.cron_id)?.to_string();
            record.cron_id = external;
            Some(record)
        })
        .collect();
    records.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.cron_id.cmp(&b.cron_id))
    });
    Ok(Json(json!(records)))
}

async fn delete_cron(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(cron_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state
        .server_store
        .delete_cron(&tenant.scope(&cron_id))
        .await
        .map_err(internal_err)?
    {
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
    Extension(tenant): Extension<TenantContext>,
    Path((namespace, key)): Path<(String, String)>,
    Json(value): Json<Value>,
) -> Result<(StatusCode, Json<store::StoreItem>), ApiError> {
    store::validate_segment("namespace", &namespace)?;
    store::validate_segment("key", &key)?;
    // KV namespaces are tenant-scoped: the internal namespace carries the
    // `{tenant}/` prefix, the wire item reports the external namespace.
    let (mut item, created) = state
        .server_store
        .kv_put(&tenant.scope(&namespace), &key, value)
        .await
        .map_err(internal_err)?;
    item.namespace = namespace;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(item)))
}

async fn get_store_item(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path((namespace, key)): Path<(String, String)>,
) -> Result<Json<store::StoreItem>, ApiError> {
    store::validate_segment("namespace", &namespace)?;
    store::validate_segment("key", &key)?;
    state
        .server_store
        .kv_get(&tenant.scope(&namespace), &key)
        .await
        .map_err(internal_err)?
        .map(|mut item| {
            item.namespace = namespace.clone();
            Json(item)
        })
        .ok_or_else(|| ApiError::not_found(format!("no store item at `{namespace}/{key}`")))
}

async fn delete_store_item(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path((namespace, key)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    store::validate_segment("namespace", &namespace)?;
    store::validate_segment("key", &key)?;
    if state
        .server_store
        .kv_delete(&tenant.scope(&namespace), &key)
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
    Extension(tenant): Extension<TenantContext>,
    Path(namespace): Path<String>,
) -> Result<Json<Value>, ApiError> {
    store::validate_segment("namespace", &namespace)?;
    let items = state
        .server_store
        .kv_list(&tenant.scope(&namespace))
        .await
        .map_err(internal_err)?;
    let items: Vec<store::StoreItem> = items
        .into_iter()
        .map(|mut item| {
            item.namespace = namespace.clone();
            item
        })
        .collect();
    Ok(Json(json!(items)))
}

// --------------------------------------------------------------------- //
// Durable task queue (R0.6)
// --------------------------------------------------------------------- //

#[derive(Debug, Deserialize)]
struct EnqueueTaskPayload {
    /// Work classification the worker fleet dispatches on (free-form).
    kind: String,
    /// Work payload: any JSON value, stored verbatim.
    payload: Value,
    /// Named pool (default `default`); workers claim from named pools.
    #[serde(default)]
    pool: Option<String>,
    /// Attempt ceiling before dead-lettering (default 3, max 100).
    #[serde(default)]
    max_attempts: Option<u32>,
    /// Dedup key, unique per tenant across live tasks: re-enqueueing with
    /// the same key returns the existing task (`deduplicated: true`).
    #[serde(default)]
    idempotency_key: Option<String>,
    /// Declared effect classification of the work (`pure` / `read_only` /
    /// `idempotent` / `compensatable` / `non_idempotent`, the Flight
    /// Recorder taxonomy). The retry policy's effect gate: a declared
    /// non-repeatable effect is never silently retried. Optional — when
    /// absent, the worker's per-attempt `retryable` flag decides.
    #[serde(default)]
    effect: Option<String>,
    /// Run linkage: the run this task belongs to.
    /// `POST /runs/{run_id}/cancel` cancels every non-terminal task
    /// carrying its run id — the run-level half of cancellation
    /// propagation. Optional; the outbox wave sets this from the run
    /// itself.
    #[serde(default)]
    run_id: Option<String>,
    /// Thread linkage (companion to `run_id`).
    #[serde(default)]
    thread_id: Option<String>,
    /// Whole-task deadline (RFC 3339), across attempts. Past it the claim
    /// path finalizes the task as cancelled instead of leasing it, and a
    /// worker that sees it pass mid-attempt reports the attempt cancelled.
    #[serde(default)]
    deadline: Option<String>,
}

/// `POST /tasks` — enqueue a durable task. `201 {task_id, deduplicated:
/// false}` on creation, `200 {task_id, deduplicated: true}` when the
/// idempotency key already names a live task in this tenant.
async fn enqueue_task(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Json(payload): Json<EnqueueTaskPayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    tasks::validate_label("kind", &payload.kind, 256).map_err(ApiError::bad_request)?;
    let pool = payload
        .pool
        .unwrap_or_else(|| tasks::DEFAULT_POOL.to_string());
    tasks::validate_pool(&pool).map_err(ApiError::bad_request)?;
    let max_attempts = payload.max_attempts.unwrap_or(tasks::DEFAULT_MAX_ATTEMPTS);
    if !(1..=tasks::MAX_ATTEMPTS_LIMIT).contains(&max_attempts) {
        return Err(ApiError::bad_request(format!(
            "`max_attempts` must be within 1..={}",
            tasks::MAX_ATTEMPTS_LIMIT
        )));
    }
    if let Some(key) = &payload.idempotency_key {
        tasks::validate_label("idempotency_key", key, 256).map_err(ApiError::bad_request)?;
    }
    let effect = payload
        .effect
        .as_deref()
        .map(tasks::parse_effect)
        .transpose()
        .map_err(ApiError::bad_request)?;
    if let Some(run_id) = &payload.run_id {
        tasks::validate_label("run_id", run_id, 256).map_err(ApiError::bad_request)?;
    }
    if let Some(thread_id) = &payload.thread_id {
        tasks::validate_label("thread_id", thread_id, 256).map_err(ApiError::bad_request)?;
    }
    let deadline = payload
        .deadline
        .as_deref()
        .map(|raw| {
            chrono::DateTime::parse_from_rfc3339(raw)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|_| {
                    ApiError::bad_request(format!(
                        "`deadline` must be an RFC 3339 timestamp (got `{raw}`)"
                    ))
                })
        })
        .transpose()?;

    let record = TaskRecord::new(
        tasks::NewTask {
            task_id: uuid::Uuid::new_v4().to_string(),
            tenant: tenant.tenant().to_string(),
            kind: payload.kind,
            payload: payload.payload,
            pool,
            max_attempts,
            idempotency_key: payload.idempotency_key,
            effect,
            run_id: payload.run_id,
            thread_id: payload.thread_id,
            deadline,
        },
        Utc::now(),
    );
    let (task, deduplicated) = state
        .server_store
        .enqueue_task(&record)
        .await
        .map_err(internal_err)?;
    let status = if deduplicated {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(json!({
            "task_id": task.task_id,
            "deduplicated": deduplicated,
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct ClaimTaskPayload {
    /// Stable worker identity; only this id may heartbeat/settle the lease.
    worker_id: String,
    /// Pools to claim from (default `["default"]`); an explicit empty list
    /// is a 400 — it could never match a task.
    #[serde(default)]
    pools: Option<Vec<String>>,
    /// Visibility timeout in milliseconds (100..=3_600_000).
    lease_ms: u64,
}

/// `POST /tasks/claim` — take the oldest claimable task: `200 {"task": {…}}`
/// with a fresh lease, or `204` (empty body) when nothing is claimable.
/// Claimable means queued, failed past its backoff schedule, or leased past
/// its visibility timeout (safe reassignment after worker loss).
async fn claim_task(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Json(payload): Json<ClaimTaskPayload>,
) -> Result<Response, ApiError> {
    tasks::validate_label("worker_id", &payload.worker_id, 256).map_err(ApiError::bad_request)?;
    tasks::validate_lease_ms(payload.lease_ms).map_err(ApiError::bad_request)?;
    let pools = payload
        .pools
        .unwrap_or_else(|| vec![tasks::DEFAULT_POOL.to_string()]);
    if pools.is_empty() {
        return Err(ApiError::bad_request(
            "`pools` must name at least one pool".to_string(),
        ));
    }
    for pool in &pools {
        tasks::validate_pool(pool).map_err(ApiError::bad_request)?;
    }

    let claimed = state
        .server_store
        .claim_task(
            tenant.tenant(),
            &payload.worker_id,
            &pools,
            payload.lease_ms,
            Utc::now(),
        )
        .await
        .map_err(internal_err)?;
    Ok(match claimed {
        Some(task) => Json(json!({ "task": task.wire() })).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

#[derive(Debug, Deserialize)]
struct HeartbeatTaskPayload {
    worker_id: String,
    /// New visibility timeout in milliseconds, from now.
    lease_ms: u64,
}

#[derive(Debug, Deserialize)]
struct CompleteTaskPayload {
    worker_id: String,
    /// The task's result: any JSON value, stored on the record.
    result: Value,
}

#[derive(Debug, Deserialize)]
struct FailTaskPayload {
    worker_id: String,
    /// Free-form error classification (`timeout`, `rate_limit`, `bug`, …),
    /// stored for DLQ triage.
    error_class: String,
    /// The failure message, stored as the task's `last_error`.
    message: String,
    /// The worker's permanence judgment: `false` dead-letters immediately,
    /// regardless of remaining attempts.
    retryable: bool,
}

/// Shared 404/409 mapping for the lease-guarded mutations: 404 when the task
/// is unknown to this tenant, 409 when it exists but the caller does not
/// hold its lease (never leased, already settled, or reclaimed by another
/// worker after the visibility timeout expired).
fn lease_outcome(
    outcome: MutationOutcome,
    task_id: &str,
    worker_id: &str,
) -> Result<TaskRecord, ApiError> {
    match outcome {
        MutationOutcome::Applied(task) => Ok(*task),
        MutationOutcome::LeaseLost => Err(ApiError::conflict(format!(
            "task `{task_id}` is not leased to worker `{worker_id}` (lost, expired and reclaimed, or already settled)"
        ))),
        MutationOutcome::Unknown => {
            Err(ApiError::not_found(format!("task `{task_id}` not found")))
        }
    }
}

/// `POST /tasks/{id}/heartbeat` — extend the held lease → `200
/// {"lease_expires_at": "…", "cancel_requested": bool}`; `409` when the
/// lease is lost. `cancel_requested` is the cancellation hint: the holder
/// should abort the attempt and report it as `cancelled` through the fail
/// path (a holder that never asks is finalized by the claim path once its
/// lease lapses).
async fn heartbeat_task(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(task_id): Path<String>,
    Json(payload): Json<HeartbeatTaskPayload>,
) -> Result<Json<Value>, ApiError> {
    tasks::validate_label("worker_id", &payload.worker_id, 256).map_err(ApiError::bad_request)?;
    tasks::validate_lease_ms(payload.lease_ms).map_err(ApiError::bad_request)?;
    let outcome = state
        .server_store
        .heartbeat_task(
            tenant.tenant(),
            &task_id,
            &payload.worker_id,
            payload.lease_ms,
            Utc::now(),
        )
        .await
        .map_err(internal_err)?;
    let task = lease_outcome(outcome, &task_id, &payload.worker_id)?;
    let expires_at = task.lease.as_ref().map(|lease| lease.expires_at);
    Ok(Json(json!({
        "lease_expires_at": expires_at,
        "cancel_requested": task.cancel_requested,
    })))
}

/// `POST /tasks/{id}/complete` — settle the held lease successfully, storing
/// `result` → `200` with the updated task record; `409` when the lease is
/// lost.
async fn complete_task(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(task_id): Path<String>,
    Json(payload): Json<CompleteTaskPayload>,
) -> Result<Json<Value>, ApiError> {
    tasks::validate_label("worker_id", &payload.worker_id, 256).map_err(ApiError::bad_request)?;
    let outcome = state
        .server_store
        .complete_task(
            tenant.tenant(),
            &task_id,
            &payload.worker_id,
            payload.result,
            Utc::now(),
        )
        .await
        .map_err(internal_err)?;
    let task = lease_outcome(outcome, &task_id, &payload.worker_id)?;
    Ok(Json(task.wire()))
}

/// `POST /tasks/{id}/fail` — record a failed attempt → `200 {requeued,
/// next_attempt_at, dead}`. The decision is core's shared `classify_retry`
/// policy: a retryable failure with attempts left requeues with exponential
/// backoff + full jitter (cap 5 min, scheduled at `next_attempt_at`);
/// exhausting the attempt budget dead-letters; a non-retryable class — or
/// work not safe to re-drive (the worker's `retryable: false`, or a declared
/// non-repeatable `effect` on the task) — fails outright (terminal, *not*
/// dead-lettered: `requeued: false, dead: false, next_attempt_at: null`).
/// `400` for an `error_class` outside the shared taxonomy; `409` when the
/// lease is lost.
async fn fail_task(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(task_id): Path<String>,
    Json(payload): Json<FailTaskPayload>,
) -> Result<Json<Value>, ApiError> {
    tasks::validate_label("worker_id", &payload.worker_id, 256).map_err(ApiError::bad_request)?;
    let error_class =
        tasks::parse_error_class(&payload.error_class).map_err(ApiError::bad_request)?;
    tasks::validate_label("message", &payload.message, 4096).map_err(ApiError::bad_request)?;
    let outcome = state
        .server_store
        .fail_task(
            tenant.tenant(),
            &task_id,
            &payload.worker_id,
            tasks::FailureReport {
                error_class,
                message: payload.message,
                retryable: payload.retryable,
            },
            Utc::now(),
        )
        .await
        .map_err(internal_err)?;
    let task = lease_outcome(outcome, &task_id, &payload.worker_id)?;
    Ok(Json(json!({
        // A retry is outstanding exactly when a next attempt is scheduled;
        // a `failed` task with a null schedule failed outright.
        "requeued": task.status == TaskStatus::Failed && task.next_attempt_at.is_some(),
        "next_attempt_at": task.next_attempt_at,
        "dead": task.status == TaskStatus::Dead,
    })))
}

/// `POST /tasks/{id}/cancel` — cancel a non-terminal task → `200` with the
/// updated record. Queued and retry-scheduled tasks move to the terminal
/// `cancelled` state immediately (never retried, never dead-lettered,
/// never re-queued); a leased task keeps its lease with
/// `cancel_requested` set, so the holder learns on its next heartbeat and
/// reports the attempt as `cancelled` through the fail path. Cancellation
/// is a hint for promptness — lease expiry stays the correctness
/// mechanism: a holder that never asks is finalized as cancelled by the
/// claim path once its lease lapses. `409` when the task is already
/// terminal, `404` for unknown or cross-tenant ids.
async fn cancel_task(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let outcome = state
        .server_store
        .cancel_task(tenant.tenant(), &task_id, Utc::now())
        .await
        .map_err(internal_err)?;
    match outcome {
        CancelOutcome::Applied(task) => Ok(Json(task.wire())),
        CancelOutcome::Terminal(status) => Err(ApiError::conflict(format!(
            "task `{task_id}` is already terminal ({}) and cannot be cancelled",
            status.as_str()
        ))),
        CancelOutcome::Unknown => Err(ApiError::not_found(format!("task `{task_id}` not found"))),
    }
}

/// `GET /tasks/{id}` — the task record (tenant-scoped; unknown or
/// cross-tenant ids answer 404).
async fn get_task(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state
        .server_store
        .get_task(tenant.tenant(), &task_id)
        .await
        .map_err(internal_err)?
        .map(|task| Json(task.wire()))
        .ok_or_else(|| ApiError::not_found(format!("task `{task_id}` not found")))
}

#[derive(Debug, Deserialize)]
struct ListTasksQuery {
    /// Filter to one lifecycle status; `status=dead` is the DLQ listing.
    #[serde(default)]
    status: Option<String>,
}

/// `GET /tasks?status=…` — the tenant's tasks, oldest first, optionally
/// filtered by status. An unknown status answers 400 rather than silently
/// returning everything.
async fn list_tasks(
    AxumState(state): AxumState<Arc<AppState>>,
    Extension(tenant): Extension<TenantContext>,
    Query(query): Query<ListTasksQuery>,
) -> Result<Json<Value>, ApiError> {
    let status = query
        .status
        .as_deref()
        .map(|s| {
            TaskStatus::parse(s).ok_or_else(|| {
                ApiError::bad_request(format!(
                    "unknown task status `{s}` (expected queued|leased|failed|completed|dead|cancelled)"
                ))
            })
        })
        .transpose()?;
    let tasks = state
        .server_store
        .list_tasks(tenant.tenant(), status)
        .await
        .map_err(internal_err)?;
    let wire: Vec<Value> = tasks.iter().map(TaskRecord::wire).collect();
    Ok(Json(json!(wire)))
}
