//! Run scheduling, execution, and bookkeeping.
//!
//! A run goes: *schedule* (strategy check + handle insert) → *execute*
//! (drive [`Executor`] in a spawned task, forwarding [`GraphEvent`]s to a
//! per-run SSE frame log + broadcast channel) → *terminate* (terminal status
//! + JSON recorded, waiters woken, next queued run for the thread spawned).
//!
//! Multitask: there is always at most one **active** run per thread. The
//! `reject` strategy returns 409 when the thread is busy; `enqueue` appends
//! to an in-memory per-thread FIFO queue (depth-capped by
//! `ServerConfig::max_concurrent_runs_per_thread`) that drains automatically
//! as runs finish.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use agentgraph::checkpoint::Checkpointer;
use agentgraph::error::AgentGraphError;
use agentgraph::executor::{ExecutionOutcome, Executor, GraphEvent, RunConfig};
use agentgraph::state::State;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, watch, Mutex};

use crate::error::ApiError;
use crate::GraphRegistry;

// --------------------------------------------------------------------- //
// Run payload (accepted by all three run endpoints)
// --------------------------------------------------------------------- //

/// The `command` field of a run payload: `{ "resume": <value> }` continues
/// an interrupted thread via [`RunConfig::with_resume`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommandPayload {
    /// Resume value delivered to the interrupted node.
    #[serde(default)]
    pub resume: Option<Value>,
}

/// The `config` field of a run payload.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunConfigPayload {
    /// Maps to [`RunConfig::with_max_steps`] (LangGraph `recursion_limit`).
    #[serde(default)]
    pub recursion_limit: Option<usize>,
}

/// The `checkpoint` field of a run payload: `{ "checkpoint_id": "…" }`
/// replays the thread from that checkpoint (time travel) instead of the
/// latest, via [`RunConfig::with_checkpoint_id`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CheckpointPayload {
    /// Id of a checkpoint of this thread (see `POST /threads/{id}/history`).
    pub checkpoint_id: String,
}

/// The payload accepted by `POST /threads/{id}/runs{,/wait,/stream}`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunPayload {
    /// Initial state (must be a JSON object). Ignored when resuming: the
    /// checkpointed state takes precedence.
    #[serde(default)]
    pub input: Option<Value>,

    /// `{ "resume": <value> }` — the human-in-the-loop channel.
    #[serde(default)]
    pub command: Option<CommandPayload>,

    /// `{ "recursion_limit": n }`.
    #[serde(default)]
    pub config: Option<RunConfigPayload>,

    /// `{ "checkpoint_id": "…" }` — time travel: replay the run from that
    /// checkpoint of this thread instead of the latest (`404` when the
    /// checkpoint is unknown). Prefer forking first
    /// (`POST /threads/{id}/fork`) and replaying on the fork.
    #[serde(default)]
    pub checkpoint: Option<CheckpointPayload>,

    /// Free-form run metadata (stored, not interpreted).
    #[serde(default)]
    pub metadata: Option<Value>,

    /// Which frame families to emit on the SSE stream. Default:
    /// `["values", "updates"]`. `metadata`, `error`, and `end` frames are
    /// always emitted.
    #[serde(default)]
    pub stream_mode: Option<Vec<String>>,

    /// `"reject"` (409 when the thread is busy) or `"enqueue"` (default:
    /// queue onto the per-thread run queue).
    #[serde(default)]
    pub multitask_strategy: Option<String>,

    /// Run through a named assistant (see `POST /assistants`). The
    /// assistant must be bound to the same graph as the thread; its
    /// `config.recursion_limit` applies when the payload does not set one.
    #[serde(default)]
    pub assistant_id: Option<String>,
}

/// How a second run on a busy thread is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultitaskStrategy {
    /// Queue behind the active run (default).
    Enqueue,
    /// Fail immediately with 409.
    Reject,
}

impl MultitaskStrategy {
    /// Parse the wire value (`None` defaults to `enqueue`).
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw {
            None | Some("enqueue") => Ok(Self::Enqueue),
            Some("reject") => Ok(Self::Reject),
            Some(other) => Err(format!(
                "unknown multitask_strategy `{other}` (expected `enqueue` or `reject`)"
            )),
        }
    }
}

// --------------------------------------------------------------------- //
// Run bookkeeping
// --------------------------------------------------------------------- //

/// Lifecycle status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Queued behind another run on the same thread.
    Pending,
    /// Currently executing.
    Running,
    /// Terminated normally.
    Success,
    /// Suspended on an interrupt; resumable via `command.resume`.
    Interrupted,
    /// Failed.
    Error,
}

impl RunStatus {
    /// The wire representation of the status.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Pending => "pending",
            RunStatus::Running => "running",
            RunStatus::Success => "success",
            RunStatus::Interrupted => "interrupted",
            RunStatus::Error => "error",
        }
    }
}

/// One SSE frame as recorded in the per-run event log and broadcast live.
/// `id` follows the design doc's `{checkpoint_id}:{step}:{seq}` format.
#[derive(Debug, Clone)]
pub struct SseFrame {
    /// Frame id: `{checkpoint_id}:{step}:{seq}`.
    pub id: String,
    /// SSE event name (`metadata`, `updates`, `values`, `error`, `end`).
    pub event: String,
    /// JSON payload.
    pub data: Value,
    /// Per-run monotonically increasing sequence number (1-based).
    pub seq: u64,
}

/// Shared frame producer for one run: assigns sequence numbers, appends to
/// the bounded event log, and fans out over the broadcast channel.
#[derive(Clone)]
pub(crate) struct FrameSink {
    log: Arc<StdMutex<VecDeque<SseFrame>>>,
    bcast: broadcast::Sender<SseFrame>,
    seq: Arc<AtomicU64>,
    last_checkpoint: Arc<StdMutex<String>>,
    last_step: Arc<AtomicU64>,
    capacity: usize,
}

impl FrameSink {
    fn new(capacity: usize, bcast: broadcast::Sender<SseFrame>) -> Self {
        Self {
            log: Arc::new(StdMutex::new(VecDeque::new())),
            bcast,
            seq: Arc::new(AtomicU64::new(0)),
            last_checkpoint: Arc::new(StdMutex::new("-".to_string())),
            last_step: Arc::new(AtomicU64::new(0)),
            capacity,
        }
    }

    /// Record and broadcast one frame.
    pub(crate) fn push(&self, event: &str, step: usize, data: Value) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        self.last_step.store(step as u64, Ordering::Relaxed);
        let checkpoint = self
            .last_checkpoint
            .lock()
            .expect("frame sink lock poisoned")
            .clone();
        let frame = SseFrame {
            id: format!("{checkpoint}:{step}:{seq}"),
            event: event.to_string(),
            data,
            seq,
        };
        {
            let mut log = self.log.lock().expect("frame sink lock poisoned");
            if log.len() >= self.capacity {
                log.pop_front();
            }
            log.push_back(frame.clone());
        }
        // No live subscribers is normal (background runs); not an error.
        let _ = self.bcast.send(frame);
    }

    /// Point subsequent frame ids at a freshly persisted checkpoint.
    pub(crate) fn note_checkpoint(&self, checkpoint_id: &str) {
        *self
            .last_checkpoint
            .lock()
            .expect("frame sink lock poisoned") = checkpoint_id.to_string();
    }

    /// The super-step of the most recently pushed frame.
    pub(crate) fn current_step(&self) -> usize {
        self.last_step.load(Ordering::Relaxed) as usize
    }
}

/// Everything the executor task needs, snapshotted from a [`RunHandle`].
pub(crate) struct RunSnapshot {
    /// Internal (tenant-scoped) thread id: used for the checkpointer, the
    /// executor config, and RunManager bookkeeping.
    pub thread_id: String,
    /// External thread id as the client knows it — the only form that may
    /// appear on the wire (SSE frames, terminal JSON).
    pub wire_thread_id: String,
    pub graph: String,
    pub attempt: usize,
    pub payload: RunPayload,
    pub sink: FrameSink,
    pub checkpoint_ids: Arc<StdMutex<Vec<String>>>,
}

/// Public-ish view of a run (used by the rollback and status endpoints).
pub(crate) struct RunInfo {
    /// Internal (tenant-scoped) thread id — handlers check tenant ownership
    /// against it before revealing anything about the run.
    pub thread_id: String,
    /// External thread id for wire responses.
    pub wire_thread_id: String,
    pub graph: String,
    pub attempt: usize,
    pub status: RunStatus,
    /// The terminal JSON once the run has finished (`None` while active).
    pub terminal: Option<Value>,
    pub checkpoint_ids: Arc<StdMutex<Vec<String>>>,
}

/// Handle for one scheduled run, owned by the [`RunManager`].
pub struct RunHandle {
    /// Run id (UUID v4).
    pub run_id: String,
    /// Internal (tenant-scoped) thread id this run executes against.
    pub thread_id: String,
    /// External thread id reported on the wire.
    pub wire_thread_id: String,
    /// Registered graph name.
    pub graph: String,
    /// 1-based attempt counter for the thread.
    pub attempt: usize,
    /// Lifecycle status.
    pub status: RunStatus,
    /// Original run payload.
    pub payload: RunPayload,
    sink: FrameSink,
    terminal: watch::Sender<Option<Value>>,
    checkpoint_ids: Arc<StdMutex<Vec<String>>>,
}

impl RunHandle {
    /// Subscribe to the live frame stream.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<SseFrame> {
        self.sink.bcast.subscribe()
    }

    /// A point-in-time copy of the event log (for replay).
    pub(crate) fn log_snapshot(&self) -> Vec<SseFrame> {
        self.sink
            .log
            .lock()
            .expect("frame sink lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

/// What [`RunManager::insert`] decided for a freshly scheduled run.
pub(crate) enum ScheduleDecision {
    /// The thread slot was free; the run must be spawned now.
    Started,
    /// The run was queued behind the active run.
    Queued,
}

#[derive(Default)]
struct RunManagerInner {
    runs: HashMap<String, RunHandle>,
    active_by_thread: HashMap<String, String>,
    queues: HashMap<String, VecDeque<String>>,
    attempts: HashMap<String, usize>,
}

/// Registry of all runs, plus per-thread scheduling state. Cheap to clone
/// (shared inner); deliberately `Mutex<HashMap<run_id, RunHandle>>`-based —
/// no external locking crates.
#[derive(Default, Clone)]
pub struct RunManager {
    inner: Arc<Mutex<RunManagerInner>>,
}

impl RunManager {
    /// An empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a new run under the given multitask strategy, assigning its
    /// per-thread attempt number.
    pub(crate) async fn insert(
        &self,
        mut handle: RunHandle,
        strategy: MultitaskStrategy,
        queue_cap: usize,
    ) -> Result<ScheduleDecision, ApiError> {
        let mut inner = self.inner.lock().await;
        let busy = inner.active_by_thread.contains_key(&handle.thread_id);
        let attempt = {
            let counter = inner.attempts.entry(handle.thread_id.clone()).or_insert(0);
            *counter += 1;
            *counter
        };
        handle.attempt = attempt;

        match strategy {
            MultitaskStrategy::Reject if busy => Err(ApiError::conflict(format!(
                "thread `{}` already has an active run",
                handle.thread_id
            ))),
            _ if busy => {
                let queue = inner.queues.entry(handle.thread_id.clone()).or_default();
                if queue.len() >= queue_cap {
                    return Err(ApiError::conflict(format!(
                        "thread `{}` run queue is full (cap {queue_cap})",
                        handle.thread_id
                    )));
                }
                queue.push_back(handle.run_id.clone());
                inner.runs.insert(handle.run_id.clone(), handle);
                Ok(ScheduleDecision::Queued)
            }
            _ => {
                inner
                    .active_by_thread
                    .insert(handle.thread_id.clone(), handle.run_id.clone());
                handle.status = RunStatus::Running;
                inner.runs.insert(handle.run_id.clone(), handle);
                Ok(ScheduleDecision::Started)
            }
        }
    }

    /// Snapshot everything the executor task needs for `run_id`.
    pub(crate) async fn snapshot(&self, run_id: &str) -> Option<RunSnapshot> {
        let inner = self.inner.lock().await;
        inner.runs.get(run_id).map(|h| RunSnapshot {
            thread_id: h.thread_id.clone(),
            wire_thread_id: h.wire_thread_id.clone(),
            graph: h.graph.clone(),
            attempt: h.attempt,
            payload: h.payload.clone(),
            sink: h.sink.clone(),
            checkpoint_ids: Arc::clone(&h.checkpoint_ids),
        })
    }

    /// Read-only run info for API endpoints.
    pub(crate) async fn info(&self, run_id: &str) -> Option<RunInfo> {
        let inner = self.inner.lock().await;
        inner.runs.get(run_id).map(|h| RunInfo {
            thread_id: h.thread_id.clone(),
            wire_thread_id: h.wire_thread_id.clone(),
            graph: h.graph.clone(),
            attempt: h.attempt,
            status: h.status,
            terminal: h.terminal.borrow().clone(),
            checkpoint_ids: Arc::clone(&h.checkpoint_ids),
        })
    }

    /// Record the terminal status + JSON, wake waiters, release the thread
    /// slot, and return the next queued run id for the thread (if any), now
    /// marked active.
    pub(crate) async fn finish(
        &self,
        run_id: &str,
        status: RunStatus,
        terminal: Value,
    ) -> Option<String> {
        let mut inner = self.inner.lock().await;
        let handle = inner.runs.get_mut(run_id)?;
        handle.status = status;
        // `send_replace` (not `send`) so the terminal JSON is stored even
        // when no waiter holds a receiver (background runs); status polling
        // via `info` reads it back through `watch::Sender::borrow`.
        handle.terminal.send_replace(Some(terminal));
        let thread_id = handle.thread_id.clone();

        if inner
            .active_by_thread
            .get(&thread_id)
            .is_some_and(|active| active == run_id)
        {
            inner.active_by_thread.remove(&thread_id);
        }

        let next = inner
            .queues
            .get_mut(&thread_id)
            .and_then(VecDeque::pop_front);
        if let Some(next_id) = &next {
            if let Some(h) = inner.runs.get_mut(next_id) {
                h.status = RunStatus::Running;
            }
            inner.active_by_thread.insert(thread_id, next_id.clone());
        }
        next
    }
}

/// Everything the run machinery needs from the application: registry,
/// checkpointer, manager, and caps. Cheap to clone.
#[derive(Clone)]
pub(crate) struct RunDeps {
    pub registry: GraphRegistry,
    pub checkpointer: Arc<dyn Checkpointer>,
    pub manager: RunManager,
    pub queue_cap: usize,
    pub log_capacity: usize,
}

/// The result of successfully scheduling a run: everything an endpoint
/// needs to answer (background ack, wait, or stream).
pub(crate) struct Scheduled {
    pub run_id: String,
    pub status: RunStatus,
    pub terminal: watch::Receiver<Option<Value>>,
    pub broadcast: broadcast::Receiver<SseFrame>,
    pub replay: Vec<SseFrame>,
}

/// Create a run handle, apply the multitask strategy, and spawn execution
/// immediately when the thread slot is free.
///
/// `thread_id` is the internal (tenant-scoped) id used for the checkpointer,
/// executor, and RunManager bookkeeping; `wire_thread_id` is the external id
/// reported in SSE frames and terminal JSON.
pub(crate) async fn schedule(
    deps: &RunDeps,
    thread_id: &str,
    wire_thread_id: &str,
    graph: &str,
    payload: RunPayload,
    strategy: MultitaskStrategy,
) -> Result<Scheduled, ApiError> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let (bcast_tx, _bcast_rx) = broadcast::channel(256);
    let (terminal_tx, terminal_rx) = watch::channel(None);
    let handle = RunHandle {
        run_id: run_id.clone(),
        thread_id: thread_id.to_string(),
        wire_thread_id: wire_thread_id.to_string(),
        graph: graph.to_string(),
        attempt: 0, // assigned by RunManager::insert
        status: RunStatus::Pending,
        payload,
        sink: FrameSink::new(deps.log_capacity, bcast_tx),
        terminal: terminal_tx,
        checkpoint_ids: Arc::new(StdMutex::new(Vec::new())),
    };
    // Subscribe/snapshot before any execution can emit frames.
    let replay = handle.log_snapshot();
    let broadcast = handle.subscribe();

    let decision = deps
        .manager
        .insert(handle, strategy, deps.queue_cap)
        .await?;
    let status = match decision {
        ScheduleDecision::Started => {
            spawn_execute(deps.clone(), run_id.clone());
            RunStatus::Running
        }
        ScheduleDecision::Queued => RunStatus::Pending,
    };

    Ok(Scheduled {
        run_id,
        status,
        terminal: terminal_rx,
        broadcast,
        replay,
    })
}

/// Drive one run to its terminal state and chain the next queued run.
async fn execute(deps: RunDeps, run_id: String) {
    let Some(snap) = deps.manager.snapshot(&run_id).await else {
        tracing::warn!(%run_id, "scheduled run vanished before execution");
        return;
    };
    let sink = snap.sink.clone();
    sink.push(
        "metadata",
        0,
        json!({
            "run_id": run_id,
            "thread_id": snap.wire_thread_id,
            "graph": snap.graph,
            "attempt": snap.attempt,
            "metadata": snap.payload.metadata,
        }),
    );

    let Some((graph, spec)) = deps.registry.get(&snap.graph) else {
        let message = format!("graph `{}` is no longer registered", snap.graph);
        tracing::error!(%run_id, %message);
        sink.push(
            "error",
            0,
            json!({"error": "unknown_graph", "message": message}),
        );
        sink.push("end", 0, json!({"status": "error"}));
        let terminal = json!({
            "run_id": run_id,
            "thread_id": snap.wire_thread_id,
            "status": "error",
            "error": "unknown_graph",
            "message": message,
        });
        terminate(&deps, &run_id, RunStatus::Error, terminal).await;
        return;
    };

    let modes: Vec<String> = snap
        .payload
        .stream_mode
        .clone()
        .unwrap_or_else(|| vec!["values".to_string(), "updates".to_string()]);
    let (evt_tx, evt_rx) = mpsc::channel::<GraphEvent>(256);
    let forwarder = tokio::spawn(forward_events(
        evt_rx,
        sink.clone(),
        Arc::clone(&deps.checkpointer),
        snap.thread_id.clone(),
        Arc::clone(&snap.checkpoint_ids),
        modes,
    ));

    let mut config = RunConfig::new(snap.thread_id.clone()).with_event_tx(evt_tx);
    if let Some(command) = &snap.payload.command {
        if let Some(resume) = &command.resume {
            config = config.with_resume(resume.clone());
        }
    }
    if let Some(checkpoint) = &snap.payload.checkpoint {
        config = config.with_checkpoint_id(checkpoint.checkpoint_id.clone());
    }
    if let Some(run_cfg) = &snap.payload.config {
        if let Some(limit) = run_cfg.recursion_limit {
            config = config.with_max_steps(limit);
        }
    }
    let initial = snap
        .payload
        .input
        .clone()
        .and_then(|v| State::from_value(v).ok())
        .unwrap_or_default();

    let result = Executor::with_checkpointer(Arc::clone(&deps.checkpointer))
        .run(&graph, &spec, initial, config)
        .await;
    // `config` (holding the only sender) is dropped with the run; the
    // forwarder drains what remains and exits.
    let _ = forwarder.await;

    let step = sink.current_step();
    let (status, terminal) = match result {
        Ok(ExecutionOutcome::Done(state)) => {
            sink.push("end", step, json!({"status": "success"}));
            let terminal = json!({
                "run_id": run_id,
                "thread_id": snap.wire_thread_id,
                "status": "success",
                "output": state.to_value(),
            });
            (RunStatus::Success, terminal)
        }
        Ok(ExecutionOutcome::Interrupted {
            value,
            state,
            checkpoint_id,
        }) => {
            sink.push(
                "end",
                step,
                json!({"status": "interrupted", "interrupt": value}),
            );
            let terminal = json!({
                "run_id": run_id,
                "thread_id": snap.wire_thread_id,
                "status": "interrupted",
                "interrupt": value,
                "checkpoint_id": checkpoint_id,
                "state": state.to_value(),
            });
            (RunStatus::Interrupted, terminal)
        }
        Err(error) => {
            let kind = error_kind(&error);
            let message = error.to_string();
            tracing::warn!(%run_id, %error, "run failed");
            sink.push("error", step, json!({"error": kind, "message": message}));
            sink.push("end", step, json!({"status": "error"}));
            let terminal = json!({
                "run_id": run_id,
                "thread_id": snap.wire_thread_id,
                "status": "error",
                "error": kind,
                "message": message,
            });
            (RunStatus::Error, terminal)
        }
    };
    terminate(&deps, &run_id, status, terminal).await;
}

/// Map executor events to SSE frames per the design doc's §4 table.
async fn forward_events(
    mut rx: mpsc::Receiver<GraphEvent>,
    sink: FrameSink,
    checkpointer: Arc<dyn Checkpointer>,
    thread_id: String,
    checkpoint_ids: Arc<StdMutex<Vec<String>>>,
    modes: Vec<String>,
) {
    while let Some(event) = rx.recv().await {
        match event {
            GraphEvent::StateUpdate { step, updates } => {
                if modes.iter().any(|m| m == "updates") {
                    sink.push("updates", step, json!({"step": step, "updates": updates}));
                }
            }
            GraphEvent::Token { node, delta } => {
                if modes.iter().any(|m| m == "messages") {
                    let step = sink.current_step();
                    sink.push("messages", step, json!({"node": node, "delta": delta}));
                }
            }
            GraphEvent::CheckpointSaved {
                checkpoint_id,
                step,
            } => {
                checkpoint_ids
                    .lock()
                    .expect("checkpoint ids lock poisoned")
                    .push(checkpoint_id.clone());
                sink.note_checkpoint(&checkpoint_id);
                if modes.iter().any(|m| m == "values") {
                    match read_back_state(&*checkpointer, &thread_id, &checkpoint_id).await {
                        Ok(Some(values)) => sink.push("values", step, values),
                        Ok(None) => {
                            tracing::debug!(%checkpoint_id, "checkpoint not found for values frame")
                        }
                        Err(error) => {
                            tracing::warn!(%checkpoint_id, %error, "values frame read-back failed")
                        }
                    }
                }
            }
            // Reserved for the future `tasks` / `debug` stream modes.
            GraphEvent::SuperStep { .. }
            | GraphEvent::NodeStart { .. }
            | GraphEvent::NodeEnd { .. } => {}
        }
    }
}

/// `values` frames carry the full state persisted at a super-step boundary,
/// read back from the checkpoint log (design doc §4).
async fn read_back_state(
    checkpointer: &dyn Checkpointer,
    thread_id: &str,
    checkpoint_id: &str,
) -> agentgraph::error::Result<Option<Value>> {
    let all = checkpointer.list(thread_id).await?;
    Ok(all
        .into_iter()
        .find(|cp| cp.id == checkpoint_id)
        .map(|cp| cp.state.to_value()))
}

/// Spawn `execute` for a run. The future is boxed behind a trait object to
/// break the `execute → terminate → spawn(execute)` type cycle, which would
/// otherwise make `Send` inference recursive and fail.
fn spawn_execute(deps: RunDeps, run_id: String) {
    let fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(async move { execute(deps, run_id).await });
    tokio::spawn(fut);
}

/// Record the terminal state and spawn the next queued run, if any.
async fn terminate(deps: &RunDeps, run_id: &str, status: RunStatus, terminal: Value) {
    if let Some(next) = deps.manager.finish(run_id, status, terminal).await {
        spawn_execute(deps.clone(), next);
    }
}

/// Stable error-kind labels for the wire.
fn error_kind(error: &AgentGraphError) -> &'static str {
    match error {
        AgentGraphError::Graph(_) => "graph_error",
        AgentGraphError::Node(_) => "node_error",
        AgentGraphError::Interrupt { .. } => "interrupted",
        AgentGraphError::Checkpoint(_) => "checkpoint_error",
        AgentGraphError::Llm(_) => "llm_error",
        AgentGraphError::Tool(_) => "tool_error",
        AgentGraphError::Serialization(_) => "serialization_error",
        AgentGraphError::InvalidUpdate(_) => "invalid_update",
    }
}
