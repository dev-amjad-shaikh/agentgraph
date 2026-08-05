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
//!    [`AgentGraphError::Interrupt`] suspends the whole run instead.
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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::checkpoint::{Checkpoint, Checkpointer};
use crate::error::{AgentGraphError, Result};
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
    Interrupted {
        /// The payload passed to `interrupt()` (surfaced to the caller,
        /// e.g. a human-approval request).
        value: Value,
        /// The state as of the suspension point.
        state: State,
        /// The checkpoint persisted at the suspension point, for resuming
        /// or time travel.
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

    /// `true` if the run was interrupted.
    pub fn is_interrupted(&self) -> bool {
        matches!(self, ExecutionOutcome::Interrupted { .. })
    }
}

/// Per-run configuration (the LangGraph `RunnableConfig` analog).
#[derive(Debug, Clone, Default)]
pub struct RunConfig {
    /// Thread (session) id. Stable across interrupt/resume; namespaces all
    /// checkpoints for this run. Required for persistence and resume.
    pub thread_id: String,

    /// Maximum number of super-steps before the run aborts with
    /// [`crate::error::AgentGraphError::Graph`] (the LangGraph
    /// `recursion_limit` / `GraphRecursionError` guard). Default: 1000.
    pub max_steps: usize,

    /// Resume value for continuing an interrupted run. When set, the
    /// executor restores the latest checkpoint for `thread_id` and the
    /// interrupted node re-executes with
    /// [`crate::node::NodeContext::resume_value`] returning this value.
    pub resume: Option<Value>,

    /// Optional event sink for streaming: the executor emits [`GraphEvent`]s
    /// as the run progresses (node start/end, state updates, checkpoints,
    /// super-step boundaries). Consumers implement LangGraph's stream modes
    /// (`values` / `updates` / `tasks` / ...) as filters over this stream.
    pub event_tx: Option<mpsc::Sender<GraphEvent>>,
}

impl RunConfig {
    /// A config for `thread_id` with the default step limit.
    pub fn new(thread_id: impl Into<String>) -> Self {
        Self {
            thread_id: thread_id.into(),
            max_steps: DEFAULT_MAX_STEPS,
            resume: None,
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

/// Default super-step limit (matches LangGraph's default `recursion_limit`).
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
        /// The merged partial updates (`channel -> new value`).
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

    /// The configured checkpointer, if any.
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
    ///   and next-node set take precedence over this argument.
    /// - `config`: run configuration (thread id, step limit, resume value,
    ///   streaming sink).
    ///
    /// # Super-step algorithm (implementation plan)
    ///
    /// ```text
    /// state := initial_state
    /// active := if config.resume.is_some() && checkpoint exists {
    ///               (state, next_nodes) := checkpointer.get_latest(thread_id)
    ///           } else { [graph.entry_point()] }
    /// loop over step in 0..config.max_steps {
    ///     emit GraphEvent::SuperStep { step, active }
    ///     snapshot := state.clone()                      // immutable for the step
    ///     join_set := JoinSet::new()
    ///     for node_name in active {
    ///         node := graph.node(node_name)              // Arc<dyn Node>
    ///         ctx := NodeContext::new(snapshot.clone(), NodeConfig {
    ///                   thread_id, step, resume: (only for the resumed node), .. })
    ///         join_set.spawn(node.run(ctx))              // parallel compute
    ///         emit GraphEvent::NodeStart { node, step }
    ///     }
    ///     // ---- barrier: collect results; any failure aborts the step ----
    ///     writes: Vec<(node_name, updates)> ; commands: Vec<Command>
    ///     for result in join_set.join_all().await {
    ///         match result {
    ///             Ok(NodeOutput { updates, command }) => { writes.push(..); collect command }
    ///             Err(Interrupt { value }) => {
    ///                 checkpoint := put(step, state, next_nodes = [interrupting node])
    ///                 return Ok(ExecutionOutcome::Interrupted { value, state, checkpoint_id })
    ///             }
    ///             Err(e) => return Err(e)                // step discarded (transactional)
    ///         }
    ///         emit GraphEvent::NodeEnd { node, step }
    ///     }
    ///     // ---- merge: reducers + LastValue single-write validation ----
    ///     spec.apply_super_step(&mut state, writes)?     // InvalidUpdate on conflict
    ///     emit GraphEvent::StateUpdate { step, updates }
    ///     // ---- route: commands override edges; else evaluate outgoing edges ----
    ///     //   Command::goto / Route::Node      -> activate named nodes
    ///     //   Route::Send(sends)               -> apply each send.state via spec, activate send.node
    ///     //   Route::End / empty next set      -> checkpoint & return Done(state)
    ///     // ---- checkpoint at the boundary ----
    ///     checkpoint := Checkpoint { id: uuid v4, thread_id, step, state, next_nodes }
    ///     checkpointer.put(checkpoint).await?            // when configured
    ///     emit GraphEvent::CheckpointSaved { checkpoint_id, step }
    /// }
    /// Err(Graph("max_steps exceeded"))                   // recursion limit guard
    /// ```
    pub async fn run(
        &self,
        graph: &Graph,
        spec: &StateSpec,
        initial_state: State,
        config: RunConfig,
    ) -> Result<ExecutionOutcome> {
        // ---- initialization / resume ----
        //
        // On resume the checkpointed state and next-node set take precedence
        // over `initial_state`; the resume value is delivered to the first
        // super-step (whose active set is exactly the interrupted node set)
        // via `NodeContext::resume_value()`.
        let mut state = initial_state;
        let mut active: Vec<ActiveTask>;
        let mut step: usize = 0;
        let mut pending_resume: Option<Value> = None;

        if config.resume.is_some() {
            let checkpointer = self.checkpointer.as_ref().ok_or_else(|| {
                AgentGraphError::Checkpoint(
                    "RunConfig.resume is set but no checkpointer is configured on the executor"
                        .into(),
                )
            })?;
            let checkpoint = checkpointer
                .get_latest(&config.thread_id)
                .await?
                .ok_or_else(|| {
                    AgentGraphError::Checkpoint(format!(
                        "cannot resume thread `{}`: no checkpoint found",
                        config.thread_id
                    ))
                })?;
            state = checkpoint.state;
            step = checkpoint.step;
            active = checkpoint
                .next_nodes
                .into_iter()
                .map(|name| ActiveTask { name, scoped: None })
                .collect();
            pending_resume = config.resume.clone();
            if active.is_empty() {
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
                return Err(AgentGraphError::Graph(format!(
                    "max_steps ({}) exceeded: the graph did not terminate within the step \
                     budget (possible infinite cycle; raise RunConfig::max_steps or add a \
                     terminating route)",
                    config.max_steps
                )));
            }

            // -- plan: the active set is fully determined at this point.
            Self::emit(
                &config,
                GraphEvent::SuperStep {
                    step,
                    active_nodes: active.iter().map(|t| t.name.clone()).collect(),
                },
            );

            // -- compute: run every active node concurrently over an
            //    immutable snapshot of the start-of-step state. Scoped
            //    (Send) state is overlaid onto that invocation's private
            //    copy of the snapshot, so fan-out items never collide in
            //    the shared state.
            let snapshot = state.clone();
            let mut join_set: JoinSet<(String, Result<NodeOutput>)> = JoinSet::new();

            for task in &active {
                let node = graph.node(&task.name).ok_or_else(|| {
                    AgentGraphError::Graph(format!(
                        "routing activated unknown node `{}`",
                        task.name
                    ))
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
                            return Err(AgentGraphError::InvalidUpdate(format!(
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
                    &config,
                    GraphEvent::NodeStart {
                        node: name.clone(),
                        step,
                    },
                );
                join_set.spawn(async move { (name, node.run(ctx).await) });
            }
            // The resume value is consumed by the first super-step after a resume.
            pending_resume = None;

            // -- barrier: collect every node result. The step is
            //    transactional: on any failure the JoinSet is dropped
            //    (aborting stragglers) and the step's writes are discarded.
            let mut writes: Vec<(String, HashMap<String, Value>)> = Vec::new();
            let mut commands: Vec<Command> = Vec::new();
            let mut ran_nodes: Vec<String> = Vec::new();

            while let Some(joined) = join_set.join_next().await {
                let (name, result) = joined.map_err(|e| {
                    AgentGraphError::Node(format!(
                        "node task failed to join (panic or cancellation): {e}"
                    ))
                })?;
                match result {
                    Ok(output) => {
                        Self::emit(
                            &config,
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
                    Err(AgentGraphError::Interrupt { value }) => {
                        // Suspend the run: discard the in-flight step and
                        // persist a checkpoint scheduling the interrupted
                        // node to re-run on resume.
                        let checkpoint = Checkpoint::new(
                            config.thread_id.clone(),
                            step,
                            state.clone(),
                            vec![name],
                        );
                        let checkpoint_id = checkpoint.id.clone();
                        if let Some(checkpointer) = &self.checkpointer {
                            checkpointer.put(checkpoint).await?;
                            Self::emit(
                                &config,
                                GraphEvent::CheckpointSaved {
                                    checkpoint_id: checkpoint_id.clone(),
                                    step,
                                },
                            );
                        }
                        return Ok(ExecutionOutcome::Interrupted {
                            value,
                            state,
                            checkpoint_id,
                        });
                    }
                    Err(e) => {
                        return Err(AgentGraphError::Node(format!(
                            "node `{name}` failed at super-step {step}: {e}"
                        )));
                    }
                }
            }

            // -- merge: reducers + LastValue single-write validation. On
            //    error the mutated state is dropped with the run
            //    (transactional super-step).
            let mut merged_updates = serde_json::Map::new();
            for (_node, updates) in &writes {
                for (channel, value) in updates {
                    merged_updates.insert(channel.clone(), value.clone());
                }
            }
            spec.apply_super_step(&mut state, writes)?;
            if !merged_updates.is_empty() {
                Self::emit(
                    &config,
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
                            return Err(AgentGraphError::Graph(format!(
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
                                            return Err(AgentGraphError::Graph(format!(
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
                                                return Err(AgentGraphError::Graph(format!(
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
                    &config,
                    GraphEvent::CheckpointSaved {
                        checkpoint_id,
                        step,
                    },
                );
            }

            // -- terminate or schedule the next super-step.
            if next.is_empty() {
                return Ok(ExecutionOutcome::Done(state));
            }
            active = next;
            step += 1;
            steps_run += 1;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::InMemoryCheckpointer;
    use crate::graph::GraphBuilder;
    use crate::llm::ChatModel;
    use crate::state::Reducer;
    use serde_json::json;

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
        assert!(matches!(err, AgentGraphError::Graph(_)));
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
}
