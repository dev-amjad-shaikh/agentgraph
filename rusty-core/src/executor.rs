//! The executor: a Pregel/BSP-inspired super-step run loop.
//!
//! Execution proceeds in discrete **super-steps** (Google Pregel /
//! Bulk-Synchronous-Parallel), each super-step being:
//!
//! 1. **Plan** — determine the active node set (entry point on step 0;
//!    afterwards the routing result of the previous step, including
//!    [`crate::node::Command::goto`] overrides and [`crate::graph::Send`]
//!    fan-outs).
//! 2. **Compute** — run all active nodes concurrently in a
//!    `tokio::task::JoinSet`, each receiving an **immutable snapshot** of
//!    the state as of the start of the step. No node can observe another's
//!    in-progress writes.
//! 3. **Barrier** — wait for all active nodes. The step is *transactional*:
//!    if any node fails, the step's writes are discarded. An
//!    [`RustyError::Interrupt`] suspends the whole run instead.
//! 4. **Merge** — apply all node updates to the state via
//!    [`crate::state::StateSpec::apply_super_step`] (per-channel reducers +
//!    `LastValue` single-write validation).
//! 5. **Route** — evaluate outgoing edges / commands against the
//!    post-barrier state to determine the next active set; `Route::End` (or
//!    an empty next set) terminates the run.
//! 6. **Checkpoint** — persist a [`crate::checkpoint::Checkpoint`] recording
//!    step, state, and next nodes, and emit [`GraphEvent`]s for streaming.
//!
//! A graph *cycle* (e.g. the ReAct loop `agent → tools → agent`) is not
//! call-stack recursion — it is nodes being re-scheduled across super-steps,
//! which is why the guard is `max_steps`, not a stack limit.
//!
//! # Observability
//!
//! The executor emits `tracing` telemetry throughout a run (no subscriber is
//! installed by the library — the application chooses one):
//!
//! - `rusty.run` (INFO span) — one per [`Executor::run`] call, carrying
//!   `thread_id` and `max_steps`; parent of everything below.
//! - `rusty.super_step` (DEBUG span) — one per super-step, carrying
//!   `step` and `active_nodes`; covers plan → barrier → merge → route.
//! - `rusty.node` (INFO span) — one per spawned node task, carrying
//!   `node` and `step`; attached to the `JoinSet` task via `.instrument()`.
//! - DEBUG event on each barrier merge (channels written), INFO events on
//!   interrupt and run completion (`steps`, `duration_ms`), WARN events on
//!   node failure (with a `retryable` classification).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::Instrument;

use crate::checkpoint::{Checkpoint, Checkpointer};
use crate::error::{Result, RustyError};
use crate::graph::{Edge, Graph, Route};
use crate::node::{Command, NodeConfig, NodeContext, NodeOutput};
use crate::state::{State, StateSpec};

/// How a run ended.
#[derive(Debug)]
pub enum ExecutionOutcome {
    /// The run terminated normally (routing reached `Route::End` or no
    /// nodes remained active). Carries the final state.
    Done(State),

    /// A node called `interrupt(payload)`: the run is suspended and
    /// resumable. Carry on by calling [`Executor::run`] again with the same
    /// `thread_id` and `RunConfig::resume` set.
    ///
    /// Suspension is run-wide, not node-local. The in-flight super-step is
    /// transactional: every write of the step is discarded (including writes
    /// from sibling nodes that completed before the interrupt was observed
    /// at the barrier), still-running siblings are aborted, and the
    /// suspension checkpoint re-schedules **every** node of the step. On
    /// resume all of them re-execute from their start, so node logic must be
    /// idempotent.
    Interrupted {
        /// The payload passed to `interrupt()` (surfaced to the caller,
        /// e.g. a human-approval request).
        value: Value,
        /// The state as of the suspension point.
        state: State,
        /// The checkpoint persisted at the suspension point, for resuming
        /// or time travel. When the executor has no checkpointer attached,
        /// the run still suspends but nothing is persisted: the id is then
        /// only an opaque handle and can never be replayed.
        checkpoint_id: String,
    },
}

impl ExecutionOutcome {
    /// The final (or suspension-point) state, regardless of variant.
    pub fn state(&self) -> &State {
        match self {
            ExecutionOutcome::Done(s) => s,
            ExecutionOutcome::Interrupted { state, .. } => state,
        }
    }

    /// Whether the run ended in [`ExecutionOutcome::Interrupted`] (suspended,
    /// resumable) rather than [`ExecutionOutcome::Done`].
    pub fn is_interrupted(&self) -> bool {
        matches!(self, ExecutionOutcome::Interrupted { .. })
    }
}

/// Per-run configuration (the LangGraph `RunnableConfig` analog).
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Thread (session) id. Stable across interrupt/resume; namespaces all
    /// checkpoints for this run. Required for persistence and resume.
    pub thread_id: String,

    /// Maximum number of super-steps before the run aborts with
    /// [`crate::error::RustyError::Graph`] (the LangGraph
    /// `recursion_limit` / `GraphRecursionError` guard). Default: 1000.
    pub max_steps: usize,

    /// Resume value for continuing an interrupted run. When set, the
    /// executor restores the latest checkpoint for `thread_id` and
    /// re-executes the checkpointed next-node set with
    /// [`crate::node::NodeContext::resume_value`] returning this value.
    ///
    /// The value is **broadcast**: every node scheduled in the first
    /// super-step after the resume observes it, not only the node that
    /// originally interrupted (a suspension checkpoint re-schedules the
    /// whole active set — see [`ExecutionOutcome::Interrupted`]). Nodes that
    /// should react only when they themselves were resumed must key off
    /// their own state, not the presence of a resume value.
    pub resume: Option<Value>,

    /// Replay/time-travel handle: the id of a specific checkpoint of
    /// `thread_id` to resume from. When set, the executor loads **that**
    /// checkpoint (not the latest) and continues the run from its state and
    /// next-node set. Requires a checkpointer on the executor.
    ///
    /// Combines with `resume`: `checkpoint_id` selects **where** the run
    /// restarts, `resume` (when also set) is delivered as the resume value to
    /// the first super-step, exactly as in interrupt/resume.
    ///
    /// Safe pattern: replaying on the *same* thread appends new history on
    /// top of the old timeline, so prefer forking first —
    /// [`crate::checkpoint::Checkpointer::fork_thread`] the thread into a new
    /// thread id, then run the fork with `checkpoint_id` set. Direct replay
    /// on the original thread is supported for cases where appended history
    /// is acceptable.
    pub checkpoint_id: Option<String>,

    /// Optional event sink for streaming: the executor emits [`GraphEvent`]s
    /// as the run progresses (node start/end, state updates, checkpoints,
    /// super-step boundaries). Consumers implement LangGraph's stream modes
    /// (`values` / `updates` / `tasks` / ...) as filters over this stream.
    pub event_tx: Option<mpsc::Sender<GraphEvent>>,
}

impl Default for RunConfig {
    /// A config with an empty `thread_id` and the default step limit —
    /// identical to `RunConfig::new("")`. Derived `Default` would zero
    /// `max_steps`, so any `default()`-built run would instantly trip the
    /// step guard; keep this impl in sync with [`RunConfig::new`].
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl RunConfig {
    /// A config for `thread_id` with the default step limit.
    pub fn new(thread_id: impl Into<String>) -> Self {
        Self {
            thread_id: thread_id.into(),
            max_steps: DEFAULT_MAX_STEPS,
            resume: None,
            checkpoint_id: None,
            event_tx: None,
        }
    }

    /// Builder-style: override the step limit.
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Builder-style: set the resume value.
    pub fn with_resume(mut self, value: Value) -> Self {
        self.resume = Some(value);
        self
    }

    /// Builder-style: replay from a specific checkpoint of `thread_id`
    /// (time travel). See the [`RunConfig::checkpoint_id`] field docs for
    /// semantics and the fork-first safe pattern.
    pub fn with_checkpoint_id(mut self, checkpoint_id: impl Into<String>) -> Self {
        self.checkpoint_id = Some(checkpoint_id.into());
        self
    }

    /// Builder-style: attach a streaming event sink.
    pub fn with_event_tx(mut self, tx: mpsc::Sender<GraphEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// A clone of the event sink sender, for wiring into nodes that stream
    /// [`GraphEvent::Token`] deltas (LangGraph's `messages` stream mode).
    ///
    /// Typical flow: create the channel, call `config.token_tx()` to obtain
    /// the clone a node closure captures, then hand `config` to
    /// [`Executor::run`]. `None` when no sink is attached. See the
    /// [`crate::llm::ChatModel`] rustdoc for the full pattern.
    pub fn token_tx(&self) -> Option<mpsc::Sender<GraphEvent>> {
        self.event_tx.clone()
    }
}

/// Default super-step limit. Deliberately far above LangGraph's default
/// `recursion_limit` of 25: ReAct-style loops burn one super-step per
/// agent/tool hop, so long tool chains legitimately exceed 25.
pub const DEFAULT_MAX_STEPS: usize = 1000;

/// Streaming events emitted by the executor during a run. All of LangGraph's
/// stream modes are views over this single typed event stream.
///
/// The enum is serializable so event streams can cross process / FFI
/// boundaries (e.g. a WebSocket bridge or a persisted event log).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GraphEvent {
    /// A node began executing.
    NodeStart {
        /// Node name.
        node: String,
        /// Super-step index.
        step: usize,
    },
    /// A node finished executing (successfully).
    NodeEnd {
        /// Node name.
        node: String,
        /// Super-step index.
        step: usize,
    },
    /// A single LLM token delta (the LangGraph `messages` stream mode).
    ///
    /// The executor itself never emits this variant: tokens originate inside
    /// nodes that call [`crate::llm::ChatModel::chat_stream`] and forward
    /// each [`crate::llm::TokenChunk`] into the run's event channel (see the
    /// `ChatModel` rustdoc for the wiring pattern and
    /// [`RunConfig::token_tx`] / [`Executor::token_tx`] for sender handles).
    Token {
        /// Node that produced the token.
        node: String,
        /// Incremental text produced since the previous token.
        delta: String,
    },
    /// State was updated at a super-step barrier (`updates` stream mode).
    StateUpdate {
        /// Super-step index at which the update was applied.
        step: usize,
        /// Every channel written in this step mapped to its **post-reducer**
        /// value (e.g. the full appended list for an `Append` channel), read
        /// back from the merged state — not the raw per-node partials.
        updates: serde_json::Map<String, Value>,
    },
    /// A checkpoint was persisted at a super-step boundary.
    CheckpointSaved {
        /// The checkpoint id.
        checkpoint_id: String,
        /// Super-step index at the boundary.
        step: usize,
    },
    /// A super-step began; lists the nodes activated in it.
    SuperStep {
        /// Super-step index.
        step: usize,
        /// Nodes active in this step.
        active_nodes: Vec<String>,
    },
}

/// The graph executor. Holds an optional checkpointer; stateless with
/// respect to individual runs, so one `Executor` can drive many concurrent
/// runs (each with its own `thread_id`).
#[derive(Default)]
pub struct Executor {
    checkpointer: Option<Arc<dyn Checkpointer>>,
    token_tx: Option<mpsc::Sender<GraphEvent>>,
}

impl Executor {
    /// An executor without persistence (runs cannot be interrupted/resumed
    /// durably; interrupts will still surface but resume requires a
    /// checkpointer).
    pub fn new() -> Self {
        Self::default()
    }

    /// An executor persisting checkpoints through `checkpointer`.
    pub fn with_checkpointer(checkpointer: Arc<dyn Checkpointer>) -> Self {
        Self {
            checkpointer: Some(checkpointer),
            token_tx: None,
        }
    }

    /// Builder-style: hold a token broadcast sender that nodes can clone to
    /// publish [`GraphEvent::Token`] deltas (LangGraph's `messages` stream
    /// mode).
    ///
    /// The executor never emits `Token` events itself — tokens originate in
    /// nodes calling [`crate::llm::ChatModel::chat_stream`]. This is a
    /// convenience handle so node factories built around an `Executor` can
    /// fetch the sink via [`Executor::token_tx`] and capture a clone in each
    /// node closure. When the same channel should also receive the
    /// executor's own events, attach it to the run via
    /// [`RunConfig::with_event_tx`] instead (or as well).
    pub fn with_token_tx(mut self, token_tx: mpsc::Sender<GraphEvent>) -> Self {
        self.token_tx = Some(token_tx);
        self
    }

    /// The token broadcast sender, if one was attached via
    /// [`Executor::with_token_tx`]. Clone it into node closures to stream
    /// [`GraphEvent::Token`]s from within nodes.
    pub fn token_tx(&self) -> Option<&mpsc::Sender<GraphEvent>> {
        self.token_tx.as_ref()
    }

    /// The configured checkpointer, if any. Shared (not consumed) so one
    /// `Executor` can drive many concurrent runs over the same store.
    pub fn checkpointer(&self) -> Option<&Arc<dyn Checkpointer>> {
        self.checkpointer.as_ref()
    }

    /// Run a compiled graph to completion (or interruption).
    ///
    /// - `graph`: the compiled, frozen graph topology.
    /// - `spec`: the state schema (channels + reducers) used to merge node
    ///   updates at each barrier.
    /// - `initial_state`: the starting state. When `config.resume` is set and
    ///   a checkpoint exists for `config.thread_id`, the checkpointed state
    ///   and next-node set take precedence over this argument. When
    ///   `config.checkpoint_id` is set, that specific checkpoint (rather than
    ///   the latest) is restored — replay/time travel; forking into a fresh
    ///   thread first via
    ///   [`crate::checkpoint::Checkpointer::fork_thread`] is the safe pattern,
    ///   since replaying on the same thread appends new history.
    /// - `config`: run configuration (thread id, step limit, resume value,
    ///   streaming sink).
    ///
    /// # Super-step semantics
    ///
    /// Each loop iteration runs one super-step as a transaction: the active
    /// nodes execute in parallel over an immutable start-of-step snapshot;
    /// the barrier discards the whole step's writes on any node failure and
    /// suspends the run on an interrupt; only then are writes merged via the
    /// channel reducers, routing computed against the post-barrier state,
    /// and a boundary checkpoint persisted. The module-level docs enumerate
    /// the six phases; `execute_super_step` is the implementation.
    ///
    /// The loop returns [`ExecutionOutcome::Done`] when routing yields an
    /// empty next set, [`ExecutionOutcome::Interrupted`] when a node
    /// interrupts, and an [`RustyError::Graph`] error once
    /// `config.max_steps` super-steps have run without termination.
    pub async fn run(
        &self,
        graph: &Graph,
        spec: &StateSpec,
        initial_state: State,
        config: RunConfig,
    ) -> Result<ExecutionOutcome> {
        // Run-level span: every super-step, node, and checkpoint trace in the
        // run attaches to it. Attached via `.instrument()` (never `.enter()`)
        // so no span guard is held across `.await` points and the returned
        // future stays `Send`.
        let run_span = tracing::info_span!(
            "rusty.run",
            thread_id = %config.thread_id,
            max_steps = config.max_steps,
            resume = config.resume.is_some(),
            replay = config.checkpoint_id.is_some(),
        );
        self.run_inner(graph, spec, initial_state, config)
            .instrument(run_span)
            .await
    }

    /// The instrumented body of [`Executor::run`]; see that method's docs for
    /// the super-step algorithm.
    async fn run_inner(
        &self,
        graph: &Graph,
        spec: &StateSpec,
        initial_state: State,
        config: RunConfig,
    ) -> Result<ExecutionOutcome> {
        let started = std::time::Instant::now();

        // ---- initialization / resume ----
        //
        // On resume the checkpointed state and next-node set take precedence
        // over `initial_state`; the resume value is delivered to the first
        // super-step (whose active set is the checkpointed next-node set —
        // after an interrupt, every node of the suspended step) via
        // `NodeContext::resume_value()`.
        //
        // Time travel: when `config.checkpoint_id` is set, THAT checkpoint is
        // restored instead of the latest — this is replay from an arbitrary
        // history point. The two knobs compose: `checkpoint_id` selects WHERE
        // the run restarts, `resume` (when also set) supplies the resume value
        // for the first super-step.
        let mut state = initial_state;
        let mut active: Vec<ActiveTask>;
        let mut step: usize = 0;
        let mut pending_resume: Option<Value> = None;

        if config.checkpoint_id.is_some() || config.resume.is_some() {
            let checkpointer = self.checkpointer.as_ref().ok_or_else(|| {
                RustyError::Checkpoint(
                    "RunConfig.checkpoint_id/resume is set but no checkpointer is configured \
                     on the executor"
                        .into(),
                )
            })?;
            let checkpoint = match &config.checkpoint_id {
                Some(id) => checkpointer
                    .get_by_id(&config.thread_id, id)
                    .await?
                    .ok_or_else(|| {
                        RustyError::Checkpoint(format!(
                            "cannot replay thread `{}`: checkpoint `{id}` not found",
                            config.thread_id
                        ))
                    })?,
                None => checkpointer
                    .get_latest(&config.thread_id)
                    .await?
                    .ok_or_else(|| {
                        RustyError::Checkpoint(format!(
                            "cannot resume thread `{}`: no checkpoint found",
                            config.thread_id
                        ))
                    })?,
            };
            state = checkpoint.state;
            step = checkpoint.step;
            active = checkpoint
                .next_nodes
                .into_iter()
                .map(|name| ActiveTask { name, scoped: None })
                .collect();
            pending_resume = config.resume.clone();
            if active.is_empty() {
                tracing::info!(
                    steps = 0,
                    duration_ms = started.elapsed().as_millis() as u64,
                    "run complete"
                );
                return Ok(ExecutionOutcome::Done(state));
            }
        } else {
            active = vec![ActiveTask {
                name: graph.entry_point().to_owned(),
                scoped: None,
            }];
        }

        // ---- super-step loop ----
        let mut steps_run: usize = 0;
        loop {
            if steps_run >= config.max_steps {
                return Err(RustyError::Graph(format!(
                    "max_steps ({}) exceeded: the graph did not terminate within the step \
                     budget (possible infinite cycle; raise RunConfig::max_steps or add a \
                     terminating route)",
                    config.max_steps
                )));
            }

            // The step body runs in a dedicated method so the whole body is
            // one instrumented future under the per-step span.
            let step_span =
                tracing::debug_span!("rusty.super_step", step = step, active_nodes = active.len(),);

            let transition = self
                .execute_super_step(
                    graph,
                    spec,
                    &config,
                    &mut state,
                    &active,
                    step,
                    &mut pending_resume,
                )
                .instrument(step_span)
                .await?;

            match transition {
                StepTransition::Next(next) => {
                    active = next;
                    step += 1;
                    steps_run += 1;
                }
                StepTransition::Finish(outcome) => {
                    if !outcome.is_interrupted() {
                        tracing::info!(
                            steps = steps_run + 1,
                            duration_ms = started.elapsed().as_millis() as u64,
                            "run complete"
                        );
                    }
                    return Ok(outcome);
                }
            }
        }
    }

    /// Executes one super-step: plan -> compute -> barrier -> merge -> route
    /// -> boundary checkpoint. Returns the next active set, or
    /// [`StepTransition::Finish`] with the terminal outcome when the run ends
    /// (`Done`) or suspends (`Interrupted`).
    #[allow(clippy::too_many_arguments)]
    async fn execute_super_step(
        &self,
        graph: &Graph,
        spec: &StateSpec,
        config: &RunConfig,
        state: &mut State,
        active: &[ActiveTask],
        step: usize,
        pending_resume: &mut Option<Value>,
    ) -> Result<StepTransition> {
        // -- plan.
        Self::emit(
            config,
            GraphEvent::SuperStep {
                step,
                active_nodes: active.iter().map(|t| t.name.clone()).collect(),
            },
        );

        // -- compute. Scoped (Send) state is overlaid onto each invocation's
        //    private copy of the start-of-step snapshot, so fan-out items
        //    never collide in the shared state.
        let snapshot = state.clone();
        let mut join_set: JoinSet<(String, Result<NodeOutput>)> = JoinSet::new();

        for task in active {
            let node = graph.node(&task.name).ok_or_else(|| {
                RustyError::Graph(format!("routing activated unknown node `{}`", task.name))
            })?;

            let mut node_state = snapshot.clone();
            if let Some(scoped) = &task.scoped {
                match scoped {
                    Value::Object(map) => {
                        for (channel, value) in map {
                            node_state.insert(channel.clone(), value.clone());
                        }
                    }
                    other => {
                        return Err(RustyError::InvalidUpdate(format!(
                            "Send scoped state for node `{}` must be a JSON object, \
                             got {other}",
                            task.name
                        )));
                    }
                }
            }

            let ctx = NodeContext::new(
                node_state,
                NodeConfig {
                    thread_id: config.thread_id.clone(),
                    step,
                    resume: pending_resume.clone(),
                    extra: HashMap::new(),
                },
            );
            let name = task.name.clone();
            Self::emit(
                config,
                GraphEvent::NodeStart {
                    node: name.clone(),
                    step,
                },
            );
            // A JoinSet polls tasks independently of the spawning task's
            // context, so the per-node span is attached to each spawned
            // future explicitly via `.instrument()`.
            let node_span = tracing::info_span!("rusty.node", node = %name, step = step);
            join_set.spawn(async move { (name, node.run(ctx).await) }.instrument(node_span));
        }
        // The resume value is consumed by the first super-step after a resume.
        *pending_resume = None;

        // -- barrier: collect every node result. The step is
        //    transactional: on any failure the JoinSet is dropped
        //    (aborting stragglers) and the step's writes are discarded.
        let mut writes: Vec<(String, HashMap<String, Value>)> = Vec::new();
        let mut commands: Vec<Command> = Vec::new();
        let mut ran_nodes: Vec<String> = Vec::new();
        let mut interrupted: Option<(String, Value)> = None;

        while let Some(joined) = join_set.join_next().await {
            let (name, result) = joined.map_err(|e| {
                RustyError::Node(format!(
                    "node task failed to join (panic or cancellation): {e}"
                ))
            })?;
            match result {
                Ok(output) => {
                    Self::emit(
                        config,
                        GraphEvent::NodeEnd {
                            node: name.clone(),
                            step,
                        },
                    );
                    if let Some(command) = output.command {
                        if !command.goto.is_empty() {
                            commands.push(command);
                        }
                    }
                    ran_nodes.push(name.clone());
                    writes.push((name, output.updates));
                }
                Err(RustyError::Interrupt { value }) => {
                    // Record the suspension and stop the barrier loop; the
                    // JoinSet is dropped below to abort stragglers.
                    interrupted = Some((name, value));
                    break;
                }
                Err(e) => {
                    // LLM and tool failures are the transient, retryable
                    // error classes; everything else is a hard failure.
                    let retryable = matches!(e, RustyError::Llm(_) | RustyError::Tool(_));
                    tracing::warn!(
                        node = %name,
                        step = step,
                        error = %e,
                        retryable = retryable,
                        "node failed; super-step aborted and its writes discarded"
                    );
                    return Err(RustyError::Node(format!(
                        "node `{name}` failed at super-step {step}: {e}"
                    )));
                }
            }
        }

        if let Some((name, value)) = interrupted {
            // Suspend the run. The step is transactional, so no write of
            // this step survived — not even from siblings that completed
            // before the interrupt reached the barrier. The suspension
            // checkpoint therefore re-schedules the ENTIRE active set (the
            // interrupting node plus all siblings), otherwise completed
            // siblings' discarded writes would be silently lost and aborted
            // siblings would never re-run. Dropping the JoinSet first aborts
            // stragglers, preserving the transactional suspension point.
            drop(join_set);
            tracing::info!(
                node = %name,
                step = step,
                "node interrupted; run suspended (resumable via RunConfig::resume)"
            );
            let pending: Vec<String> = active.iter().map(|t| t.name.clone()).collect();
            let checkpoint =
                Checkpoint::new(config.thread_id.clone(), step, state.clone(), pending);
            let checkpoint_id = checkpoint.id.clone();
            if let Some(checkpointer) = &self.checkpointer {
                checkpointer.put(checkpoint).await?;
                Self::emit(
                    config,
                    GraphEvent::CheckpointSaved {
                        checkpoint_id: checkpoint_id.clone(),
                        step,
                    },
                );
            }
            return Ok(StepTransition::Finish(ExecutionOutcome::Interrupted {
                value,
                state: state.clone(),
                checkpoint_id,
            }));
        }

        // -- merge: reducers + LastValue single-write validation. On
        //    error the mutated state is dropped with the run
        //    (transactional super-step).
        let written_channels: HashSet<String> = writes
            .iter()
            .flat_map(|(_, updates)| updates.keys().cloned())
            .collect();
        spec.apply_super_step(state, writes)?;
        // The event carries the post-reducer values read back out of the
        // merged state: when several nodes write the same channel in one
        // step (the normal Append fan-in case), reporting the raw partials
        // would keep only the last write and hide the rest.
        let mut merged_updates = serde_json::Map::new();
        for channel in &written_channels {
            if let Some(value) = state.get(channel) {
                merged_updates.insert(channel.clone(), value.clone());
            }
        }
        let channels_written: Vec<&str> = merged_updates.keys().map(String::as_str).collect();
        tracing::debug!(
            step = step,
            channels = ?channels_written,
            "merged node updates at super-step barrier"
        );
        if !merged_updates.is_empty() {
            Self::emit(
                config,
                GraphEvent::StateUpdate {
                    step,
                    updates: merged_updates,
                },
            );
        }

        // -- route: Command::goto overrides the static edge set;
        //    otherwise evaluate outgoing edges of every node that ran
        //    against the post-barrier state.
        let mut next: Vec<ActiveTask> = Vec::new();
        let mut planned: HashSet<String> = HashSet::new();

        if !commands.is_empty() {
            for command in &commands {
                for target in &command.goto {
                    if !graph.has_node(target) {
                        return Err(RustyError::Graph(format!(
                            "Command::goto references unknown node `{target}`"
                        )));
                    }
                    if planned.insert(target.clone()) {
                        next.push(ActiveTask {
                            name: target.clone(),
                            scoped: None,
                        });
                    }
                }
            }
        } else {
            let mut evaluated: HashSet<String> = HashSet::new();
            for name in &ran_nodes {
                // Fan-out invocations of the same node share one edge set;
                // evaluate it once.
                if !evaluated.insert(name.clone()) {
                    continue;
                }
                for edge in graph.outgoing_edges(name) {
                    match edge {
                        Edge::Direct { to, .. } => {
                            if planned.insert(to.clone()) {
                                next.push(ActiveTask {
                                    name: to.clone(),
                                    scoped: None,
                                });
                            }
                        }
                        Edge::Conditional { router, .. } => {
                            match router(state.clone()).await? {
                                Route::Node(target) => {
                                    if !graph.has_node(&target) {
                                        return Err(RustyError::Graph(format!(
                                            "conditional router from `{name}` returned \
                                             unknown node `{target}`"
                                        )));
                                    }
                                    if planned.insert(target.clone()) {
                                        next.push(ActiveTask {
                                            name: target,
                                            scoped: None,
                                        });
                                    }
                                }
                                Route::Send(sends) => {
                                    for send in sends {
                                        if !graph.has_node(&send.node) {
                                            return Err(RustyError::Graph(format!(
                                                "Route::Send from `{name}` targets unknown \
                                                 node `{}`",
                                                send.node
                                            )));
                                        }
                                        // Each Send is its own invocation with its own
                                        // scoped state, even when several target the
                                        // same node.
                                        next.push(ActiveTask {
                                            name: send.node,
                                            scoped: Some(send.state),
                                        });
                                    }
                                }
                                Route::End => {}
                            }
                        }
                    }
                }
            }
        }

        // -- checkpoint at the super-step boundary.
        if let Some(checkpointer) = &self.checkpointer {
            let next_names: Vec<String> = next.iter().map(|t| t.name.clone()).collect();
            let checkpoint =
                Checkpoint::new(config.thread_id.clone(), step, state.clone(), next_names);
            let checkpoint_id = checkpoint.id.clone();
            checkpointer.put(checkpoint).await?;
            Self::emit(
                config,
                GraphEvent::CheckpointSaved {
                    checkpoint_id,
                    step,
                },
            );
        }

        // -- terminate or schedule the next super-step.
        if next.is_empty() {
            return Ok(StepTransition::Finish(ExecutionOutcome::Done(
                state.clone(),
            )));
        }
        Ok(StepTransition::Next(next))
    }

    /// Best-effort event emission: a full or closed channel never aborts a run.
    fn emit(config: &RunConfig, event: GraphEvent) {
        if let Some(tx) = &config.event_tx {
            let _ = tx.try_send(event);
        }
    }
}

/// One scheduled node invocation within a super-step. `scoped` carries the
/// per-invocation input of a [`crate::graph::Send`] fan-out, overlaid onto
/// that invocation's private state snapshot before the node runs.
struct ActiveTask {
    name: String,
    scoped: Option<Value>,
}

/// The control-flow result of a single super-step: either the next active
/// set (the loop continues) or the terminal run outcome (the loop breaks).
enum StepTransition {
    Next(Vec<ActiveTask>),
    Finish(ExecutionOutcome),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::InMemoryCheckpointer;
    use crate::graph::GraphBuilder;
    use crate::llm::ChatModel;
    use crate::state::Reducer;
    use serde_json::json;
    use std::sync::Mutex;

    #[tokio::test]
    async fn linear_two_node_graph_executes_in_order() {
        let spec = StateSpec::new().channel("log", Reducer::Append);

        let mut builder = GraphBuilder::new();
        builder.add_node("first", |_ctx: NodeContext| async {
            Ok(NodeOutput::update("log", json!("first")))
        });
        builder.add_node("second", |ctx: NodeContext| async move {
            // The next super-step observes the previous step's merged writes.
            assert_eq!(ctx.state().get("log"), Some(&json!(["first"])));
            assert_eq!(ctx.step(), 1);
            Ok(NodeOutput::update("log", json!("second")))
        });
        builder.set_entry_point("first");
        builder.add_edge("first", "second");
        let graph = builder.compile().unwrap();

        let outcome = Executor::new()
            .run(&graph, &spec, State::new(), RunConfig::new("t-linear"))
            .await
            .unwrap();

        match outcome {
            ExecutionOutcome::Done(state) => {
                assert_eq!(state.get("log"), Some(&json!(["first", "second"])));
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parallel_fan_in_merges_via_reducer() {
        let spec = StateSpec::new().channel("results", Reducer::Append);

        let mut builder = GraphBuilder::new();
        builder.add_node("start", |_ctx: NodeContext| async {
            Ok(NodeOutput::empty())
        });
        builder.add_node("worker_a", |ctx: NodeContext| async move {
            // Snapshot isolation: parallel workers cannot see each other.
            assert!(!ctx.state().contains("results"));
            Ok(NodeOutput::update("results", json!("a")))
        });
        builder.add_node("worker_b", |ctx: NodeContext| async move {
            assert!(!ctx.state().contains("results"));
            Ok(NodeOutput::update("results", json!("b")))
        });
        builder.set_entry_point("start");
        builder.add_edge("start", "worker_a");
        builder.add_edge("start", "worker_b");
        let graph = builder.compile().unwrap();

        let outcome = Executor::new()
            .run(&graph, &spec, State::new(), RunConfig::new("t-fan-in"))
            .await
            .unwrap();

        match outcome {
            ExecutionOutcome::Done(state) => {
                let results = state
                    .get("results")
                    .and_then(Value::as_array)
                    .expect("results channel must exist")
                    .clone();
                // Completion order across the JoinSet is nondeterministic.
                let mut items: Vec<&str> = results.iter().map(|v| v.as_str().unwrap()).collect();
                items.sort_unstable();
                assert_eq!(items, ["a", "b"]);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn interrupt_returns_interrupted_outcome_and_resume_completes() {
        let spec = StateSpec::new().channel("answer", Reducer::Overwrite);

        let mut builder = GraphBuilder::new();
        builder.add_node("gate", |ctx: NodeContext| async move {
            match ctx.resume_value() {
                Some(v) => Ok(NodeOutput::update("answer", v.clone())),
                None => Err(ctx.interrupt(json!({"question": "approve?"}))),
            }
        });
        builder.set_entry_point("gate");
        let graph = builder.compile().unwrap();

        let checkpointer = Arc::new(InMemoryCheckpointer::new());
        let executor = Executor::with_checkpointer(checkpointer.clone());

        // First run: the gate node interrupts and the run suspends.
        let outcome = executor
            .run(&graph, &spec, State::new(), RunConfig::new("t-hitl"))
            .await
            .unwrap();

        let checkpoint_id = match outcome {
            ExecutionOutcome::Interrupted {
                value,
                checkpoint_id,
                ..
            } => {
                assert_eq!(value, json!({"question": "approve?"}));
                assert!(!checkpoint_id.is_empty());
                checkpoint_id
            }
            other => panic!("expected Interrupted, got {other:?}"),
        };

        // The suspension point was persisted and schedules the gate node.
        let stored = checkpointer.get_latest("t-hitl").await.unwrap().unwrap();
        assert_eq!(stored.id, checkpoint_id);
        assert_eq!(stored.next_nodes, vec!["gate".to_string()]);

        // Resume: the gate node re-runs with the resume value, writes its
        // answer, and the run terminates.
        let outcome = executor
            .run(
                &graph,
                &spec,
                State::new(),
                RunConfig::new("t-hitl").with_resume(json!(true)),
            )
            .await
            .unwrap();

        match outcome {
            ExecutionOutcome::Done(state) => {
                assert_eq!(state.get("answer"), Some(&json!(true)));
            }
            other => panic!("expected Done after resume, got {other:?}"),
        }
    }

    #[test]
    fn run_config_default_uses_default_step_limit() {
        // Regression: a derived `Default` would zero `max_steps`, making
        // every `RunConfig::default()` run fail immediately.
        let config = RunConfig::default();
        assert_eq!(config.max_steps, DEFAULT_MAX_STEPS);
        assert!(config.thread_id.is_empty());
        assert!(config.resume.is_none() && config.checkpoint_id.is_none());
    }

    #[tokio::test]
    async fn interrupt_reschedules_entire_active_set() {
        // Regression: the suspension checkpoint used to schedule only the
        // interrupting node, silently dropping parallel siblings — including
        // ones that had already completed, whose writes are discarded with
        // the aborted step.
        let spec = StateSpec::new()
            .channel("log", Reducer::Append)
            .channel("answer", Reducer::Overwrite);

        let mut builder = GraphBuilder::new();
        builder.add_node("start", |_ctx: NodeContext| async {
            Ok(NodeOutput::empty())
        });
        builder.add_node("gate", |ctx: NodeContext| async move {
            match ctx.resume_value() {
                Some(v) => Ok(NodeOutput::update("answer", v.clone())),
                None => Err(ctx.interrupt(json!({"question": "approve?"}))),
            }
        });
        // Completes immediately in the interrupted step; its write is
        // discarded with the step and must be recomputed on resume.
        builder.add_node("fast", |_ctx: NodeContext| async {
            Ok(NodeOutput::update("log", json!("fast")))
        });
        // Still in flight when the interrupt hits; aborted, re-run on resume.
        builder.add_node("slow", |ctx: NodeContext| async move {
            if ctx.resume_value().is_none() {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
            Ok(NodeOutput::update("log", json!("slow")))
        });
        builder.set_entry_point("start");
        builder.add_edge("start", "gate");
        builder.add_edge("start", "slow");
        builder.add_edge("start", "fast");
        let graph = builder.compile().unwrap();

        let checkpointer = Arc::new(InMemoryCheckpointer::new());
        let executor = Executor::with_checkpointer(checkpointer.clone());

        let outcome = executor
            .run(&graph, &spec, State::new(), RunConfig::new("t-par-hitl"))
            .await
            .unwrap();
        match &outcome {
            ExecutionOutcome::Interrupted { state, .. } => {
                // Transactional suspension: fast's completed write was
                // discarded with the rest of the step.
                assert_eq!(state.get("log"), None);
            }
            other => panic!("expected Interrupted, got {other:?}"),
        }

        // The suspension checkpoint re-schedules every node of the step.
        let stored = checkpointer
            .get_latest("t-par-hitl")
            .await
            .unwrap()
            .unwrap();
        let mut scheduled = stored.next_nodes.clone();
        scheduled.sort_unstable();
        assert_eq!(scheduled, ["fast", "gate", "slow"]);

        // Resume: all three re-run (the resume value is broadcast to the
        // whole step); fast's write lands exactly once.
        let outcome = executor
            .run(
                &graph,
                &spec,
                State::new(),
                RunConfig::new("t-par-hitl").with_resume(json!(true)),
            )
            .await
            .unwrap();
        match outcome {
            ExecutionOutcome::Done(state) => {
                assert_eq!(state.get("answer"), Some(&json!(true)));
                let mut log: Vec<String> = state
                    .get("log")
                    .and_then(Value::as_array)
                    .expect("log channel must exist")
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                log.sort_unstable();
                assert_eq!(log, ["fast", "slow"]);
            }
            other => panic!("expected Done after resume, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn state_update_event_reports_post_reducer_values() {
        // Regression: with several writers on one channel, the event used to
        // carry raw per-node partials collapsed by last-write-wins, hiding
        // all but one write behind its documented "merged" contract.
        let spec = StateSpec::new().channel("results", Reducer::Append);

        let mut builder = GraphBuilder::new();
        builder.add_node("start", |_ctx: NodeContext| async {
            Ok(NodeOutput::empty())
        });
        builder.add_node("worker_a", |_ctx: NodeContext| async {
            Ok(NodeOutput::update("results", json!("a")))
        });
        builder.add_node("worker_b", |_ctx: NodeContext| async {
            Ok(NodeOutput::update("results", json!("b")))
        });
        builder.set_entry_point("start");
        builder.add_edge("start", "worker_a");
        builder.add_edge("start", "worker_b");
        let graph = builder.compile().unwrap();

        let (tx, mut rx) = mpsc::channel::<GraphEvent>(64);
        let outcome = Executor::new()
            .run(
                &graph,
                &spec,
                State::new(),
                RunConfig::new("t-event").with_event_tx(tx),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ExecutionOutcome::Done(_)));

        let mut merged: Option<Vec<String>> = None;
        while let Ok(event) = rx.try_recv() {
            if let GraphEvent::StateUpdate { step: 1, updates } = event {
                let values = updates
                    .get("results")
                    .and_then(Value::as_array)
                    .expect("StateUpdate must carry the results channel")
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                merged = Some(values);
            }
        }
        let mut merged = merged.expect("expected a StateUpdate event for step 1");
        merged.sort_unstable();
        // Both partial writes are visible in the single post-reducer value.
        assert_eq!(merged, ["a", "b"]);
    }

    #[tokio::test]
    async fn max_steps_guard_aborts_infinite_cycles() {
        let spec = StateSpec::new().channel("x", Reducer::Overwrite);
        let mut builder = GraphBuilder::new();
        builder.add_node("loop_node", |_ctx: NodeContext| async {
            Ok(NodeOutput::empty())
        });
        builder.set_entry_point("loop_node");
        builder.add_edge("loop_node", "loop_node");
        let graph = builder.compile().unwrap();

        let err = Executor::new()
            .run(
                &graph,
                &spec,
                State::new(),
                RunConfig::new("t-loop").with_max_steps(5),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, RustyError::Graph(_)));
    }

    #[test]
    fn token_event_serde_roundtrip() {
        let event = GraphEvent::Token {
            node: "agent".into(),
            delta: "Hello".into(),
        };
        let wire = serde_json::to_string(&event).unwrap();
        // Internally tagged: the variant name travels on the wire.
        assert!(
            wire.contains("\"type\":\"token\""),
            "unexpected wire: {wire}"
        );
        let back: GraphEvent = serde_json::from_str(&wire).unwrap();
        assert_eq!(event, back);

        // The other variants still roundtrip under the new serde derives.
        let step_event = GraphEvent::SuperStep {
            step: 3,
            active_nodes: vec!["a".into()],
        };
        let back: GraphEvent =
            serde_json::from_str(&serde_json::to_string(&step_event).unwrap()).unwrap();
        assert_eq!(step_event, back);
    }

    /// A mock model whose `chat_stream` override emits real multi-chunk
    /// deltas, proving the accumulation contract outside any HTTP client.
    struct StreamingMock;

    #[async_trait::async_trait]
    impl crate::llm::ChatModel for StreamingMock {
        async fn chat(
            &self,
            _messages: &[crate::llm::ChatMessage],
            _tools: &[Value],
        ) -> Result<crate::llm::ChatResponse> {
            Ok(crate::llm::ChatResponse {
                message: crate::llm::ChatMessage::assistant("Hello"),
                model: None,
                usage: None,
            })
        }

        async fn chat_stream(
            &self,
            messages: &[crate::llm::ChatMessage],
            tools: &[Value],
            on_token: &mut (dyn FnMut(crate::llm::TokenChunk) + Send),
        ) -> Result<crate::llm::ChatResponse> {
            for piece in ["Hel", "lo"] {
                on_token(crate::llm::TokenChunk {
                    delta: piece.into(),
                    finish: false,
                    raw: None,
                });
            }
            on_token(crate::llm::TokenChunk {
                delta: String::new(),
                finish: true,
                raw: None,
            });
            self.chat(messages, tools).await
        }
    }

    #[tokio::test]
    async fn node_streams_token_events_through_event_sink() {
        let spec = StateSpec::new().channel("answer", Reducer::Overwrite);

        let (tx, mut rx) = mpsc::channel::<GraphEvent>(64);
        // The wiring pattern from the ChatModel rustdoc: the node closure
        // captures a clone of the run's event sender and forwards chunks.
        let node_tx = tx.clone();

        let mut builder = GraphBuilder::new();
        builder.add_node("agent", move |_ctx: NodeContext| {
            let node_tx = node_tx.clone();
            async move {
                let model = StreamingMock;
                let mut full = String::new();
                model
                    .chat_stream(&[], &[], &mut |chunk| {
                        if !chunk.delta.is_empty() {
                            full.push_str(&chunk.delta);
                            let _ = node_tx.try_send(GraphEvent::Token {
                                node: "agent".into(),
                                delta: chunk.delta,
                            });
                        }
                    })
                    .await
                    .unwrap();
                Ok(NodeOutput::update("answer", json!(full)))
            }
        });
        builder.set_entry_point("agent");
        let graph = builder.compile().unwrap();

        let config = RunConfig::new("t-tokens").with_event_tx(tx);
        // The RunConfig helper hands out the same sender for node wiring.
        assert!(config.token_tx().is_some());
        // The Executor builder/accessor pair stores a broadcast handle.
        let executor = Executor::new().with_token_tx(config.token_tx().unwrap());
        assert!(executor.token_tx().is_some());

        let outcome = executor
            .run(&graph, &spec, State::new(), config)
            .await
            .unwrap();
        match outcome {
            ExecutionOutcome::Done(state) => {
                assert_eq!(state.get("answer"), Some(&json!("Hello")))
            }
            other => panic!("expected Done, got {other:?}"),
        }

        // Drain the event stream: token deltas interleave with executor events.
        let mut deltas = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let GraphEvent::Token { node, delta } = event {
                assert_eq!(node, "agent");
                deltas.push(delta);
            }
        }
        assert_eq!(deltas, ["Hel", "lo"]);
    }

    /// A minimal `tracing::Subscriber` that records formatted event fields
    /// (`name=value` pairs) into a shared buffer, so tests can assert on the
    /// executor's instrumentation. Implemented directly against the
    /// `tracing` crate's own `Subscriber` trait (re-exported from
    /// `tracing-core`) — no `tracing-subscriber` dependency required.
    #[derive(Clone, Default)]
    struct EventCapture {
        events: Arc<Mutex<Vec<String>>>,
    }

    /// Formats an event's fields as `"name=value "` pairs.
    struct FieldVisitor(String);

    impl tracing::field::Visit for FieldVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write as _;
            let _ = write!(self.0, "{}={:?} ", field.name(), value);
        }
    }

    impl tracing::Subscriber for EventCapture {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            // Spans are irrelevant to these assertions; one id serves all.
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut visitor = FieldVisitor(String::new());
            event.record(&mut visitor);
            self.events.lock().unwrap().push(visitor.0);
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// The instrumentation must be observability-only: installing a
    /// subscriber changes nothing about the run's outcome, and the expected
    /// telemetry (merge debug event, run-completion info event) is emitted.
    #[tokio::test]
    async fn instrumentation_emits_events_without_changing_outcome() {
        let capture = EventCapture::default();
        let events = capture.events.clone();
        // Global default subscriber, deliberately NOT a thread-local
        // `set_default`: callsite interest is cached process-wide and lazily
        // (re)registered from whichever thread first hits a callsite, so a
        // thread-local subscriber races with the other tests in this binary
        // that run graphs concurrently (they rebuild the cache against the
        // no-subscriber global default and our events get silently dropped).
        // A global default makes every thread's rebuild see this subscriber.
        // Setting it is additive — other tests neither set nor assert on
        // subscribers, and captured events from concurrent runs only help the
        // `any()` assertions below. `set_global_default` may only be called
        // once per process; this is the only test that installs a subscriber.
        tracing::subscriber::set_global_default(capture)
            .expect("no other test may install a global tracing subscriber");

        let spec = StateSpec::new().channel("log", Reducer::Append);
        let mut builder = GraphBuilder::new();
        builder.add_node("only", |_ctx: NodeContext| async {
            Ok(NodeOutput::update("log", json!("x")))
        });
        builder.set_entry_point("only");
        let graph = builder.compile().unwrap();

        let outcome = Executor::new()
            .run(&graph, &spec, State::new(), RunConfig::new("t-tracing"))
            .await
            .unwrap();

        // Identical semantics: the run completes with the expected state.
        match outcome {
            ExecutionOutcome::Done(state) => {
                assert_eq!(state.get("log"), Some(&json!(["x"])));
            }
            other => panic!("expected Done, got {other:?}"),
        }

        let captured = events.lock().unwrap();
        assert!(
            captured.iter().any(|e| e.contains("channels")),
            "expected a merge debug event listing written channels, got: {captured:?}"
        );
        assert!(
            captured
                .iter()
                .any(|e| e.contains("steps") && e.contains("duration_ms")),
            "expected a run-completion info event with steps and duration_ms, got: {captured:?}"
        );
    }

    /// A 3-node linear graph (`a -> b -> c`) appending each node name to the
    /// `log` channel.
    fn linear_three_node_graph() -> (Graph, StateSpec) {
        let spec = StateSpec::new().channel("log", Reducer::Append);
        let mut builder = GraphBuilder::new();
        for name in ["a", "b", "c"] {
            builder.add_node(name, move |_ctx: NodeContext| async move {
                Ok(NodeOutput::update("log", json!(name)))
            });
        }
        builder.set_entry_point("a");
        builder.add_edge("a", "b");
        builder.add_edge("b", "c");
        (builder.compile().unwrap(), spec)
    }

    #[tokio::test]
    async fn run_with_checkpoint_id_replays_from_earlier_state() {
        let (graph, spec) = linear_three_node_graph();
        let checkpointer = Arc::new(InMemoryCheckpointer::new());
        let executor = Executor::with_checkpointer(checkpointer.clone());

        // Full run on the source thread: checkpoints at steps 0, 1, 2.
        let outcome = executor
            .run(&graph, &spec, State::new(), RunConfig::new("t-src"))
            .await
            .unwrap();
        match outcome {
            ExecutionOutcome::Done(state) => {
                assert_eq!(state.get("log"), Some(&json!(["a", "b", "c"])));
            }
            other => panic!("expected Done, got {other:?}"),
        }

        let history = checkpointer.list("t-src").await.unwrap();
        assert_eq!(history.len(), 3);
        // The step-1 checkpoint: `a` and `b` have run, `c` is scheduled next.
        let step1 = history[1].clone();
        assert_eq!(step1.step, 1);
        assert_eq!(step1.state.get("log"), Some(&json!(["a", "b"])));
        assert_eq!(step1.next_nodes, vec!["c".to_string()]);

        // Safe pattern: fork the thread at the step-1 checkpoint, then replay
        // the fork from that checkpoint (not the fork's latest — here the cut
        // point IS the latest, but `checkpoint_id` is what selects it).
        let copied = checkpointer
            .fork_thread("t-src", "t-fork", Some(&step1.id))
            .await
            .unwrap();
        assert_eq!(copied, 2);

        let outcome = executor
            .run(
                &graph,
                &spec,
                State::new(),
                RunConfig::new("t-fork").with_checkpoint_id(step1.id.clone()),
            )
            .await
            .unwrap();

        match outcome {
            ExecutionOutcome::Done(state) => {
                // Execution continued from the step-1 state: only `c` ran,
                // `b` was not re-executed.
                assert_eq!(state.get("log"), Some(&json!(["a", "b", "c"])));
            }
            other => panic!("expected Done, got {other:?}"),
        }

        // The replay appended its own boundary checkpoint to the fork only.
        assert_eq!(checkpointer.list("t-src").await.unwrap().len(), 3);
        assert_eq!(checkpointer.list("t-fork").await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn run_with_checkpoint_id_plus_resume_combined() {
        let spec = StateSpec::new().channel("answer", Reducer::Overwrite);

        let mut builder = GraphBuilder::new();
        builder.add_node("gate", |ctx: NodeContext| async move {
            match ctx.resume_value() {
                Some(v) => Ok(NodeOutput::update("answer", v.clone())),
                None => Err(ctx.interrupt(json!({"question": "approve?"}))),
            }
        });
        builder.set_entry_point("gate");
        let graph = builder.compile().unwrap();

        let checkpointer = Arc::new(InMemoryCheckpointer::new());
        let executor = Executor::with_checkpointer(checkpointer);

        // Suspend at the gate and capture the suspension checkpoint id.
        let outcome = executor
            .run(&graph, &spec, State::new(), RunConfig::new("t-hitl"))
            .await
            .unwrap();
        let checkpoint_id = match outcome {
            ExecutionOutcome::Interrupted { checkpoint_id, .. } => checkpoint_id,
            other => panic!("expected Interrupted, got {other:?}"),
        };

        // checkpoint_id selects WHERE (the suspension checkpoint), resume
        // supplies the value delivered to the re-running gate node.
        let outcome = executor
            .run(
                &graph,
                &spec,
                State::new(),
                RunConfig::new("t-hitl")
                    .with_checkpoint_id(checkpoint_id)
                    .with_resume(json!(true)),
            )
            .await
            .unwrap();

        match outcome {
            ExecutionOutcome::Done(state) => {
                assert_eq!(state.get("answer"), Some(&json!(true)));
            }
            other => panic!("expected Done after replay+resume, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_with_checkpoint_id_errors_without_checkpointer_or_unknown_id() {
        let (graph, spec) = linear_three_node_graph();

        // No checkpointer configured: replay is impossible.
        let err = Executor::new()
            .run(
                &graph,
                &spec,
                State::new(),
                RunConfig::new("t-x").with_checkpoint_id("some-id"),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, RustyError::Checkpoint(_)));

        // Checkpointer present but the id does not exist on the thread.
        let checkpointer = Arc::new(InMemoryCheckpointer::new());
        let executor = Executor::with_checkpointer(checkpointer.clone());
        executor
            .run(&graph, &spec, State::new(), RunConfig::new("t-x"))
            .await
            .unwrap();
        let err = executor
            .run(
                &graph,
                &spec,
                State::new(),
                RunConfig::new("t-x").with_checkpoint_id("no-such-checkpoint"),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, RustyError::Checkpoint(_)));
    }
}
