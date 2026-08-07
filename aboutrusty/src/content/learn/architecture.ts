import type { Article } from "./types";

export const architecture: Article = {
  slug: "architecture",
  title: "The anatomy of a run",
  description:
    "State channels and the four reducers, the Pregel/BSP super-step loop, versioned checkpoints, routing — and the ten failure modes Rusty is built to kill.",
  readingTime: "15 min read",
  blocks: [
    {
      type: "paragraph",
      text: "Rusty is the durable agent runtime built in Rust — a full-Rust, LangGraph-style agentic platform. Its core mental model fits in one sentence:",
    },
    {
      type: "callout",
      variant: "quote",
      text: "An agent is a graph over shared state, executed in super-steps.",
    },
    {
      type: "paragraph",
      text: "Four primitives make up that model — typed state channels with reducers, nodes, the super-step loop, and versioned checkpoints. Each one exists to kill a specific failure class of agent systems.",
    },

    { type: "heading", level: 2, text: "State channels and reducers" },
    {
      type: "paragraph",
      text: "“Typed state” means **schema-declared JSON state with runtime validation**, not Rust-level typing. Nodes never call each other and never return whole state. Every state key is a **channel** whose `Reducer` defines how partial updates merge. There are four reducers:",
    },
    {
      type: "table",
      head: ["Reducer", "Semantics"],
      rows: [
        ["`Overwrite`", "LangGraph's `LastValue`."],
        ["`Append`", "Multi-write reducer."],
        ["`DeepMerge`", "Multi-write reducer."],
        [
          "`AddMessages`",
          "ID-aware message upsert — LangGraph's `add_messages`. A node can correct a message it wrote earlier by `\"id\"` while parallel tool results append alongside it.",
        ],
      ],
    },
    {
      type: "paragraph",
      text: "The `StateSpec` is the complete schema: a write to an **undeclared channel is an error**, and a **second write to a single-write channel within one super-step is an error** — `InvalidUpdate` at the barrier, naming both writers.",
    },
    {
      type: "callout",
      variant: "quote",
      text: "In a parallel graph, two nodes silently clobbering the same key is otherwise the default outcome, and it surfaces only as a corrupted conversation three steps later. Here it is an `InvalidUpdate` error at the barrier, naming both writers.",
    },
    {
      type: "code",
      language: "rust",
      title: "The single-write rule — rusty-core/src/state.rs",
      code: `if *count > 1 && !reducer.allows_multiple_writes() {
    return Err(RustyError::InvalidUpdate(format!(
        "channel \`{channel}\` can receive only one value per super-step \\
         (reducer: {reducer}); already written by node \`{}\`, second write from \\
         node \`{node}\`. Use a multi-write reducer (Append/DeepMerge/\\
         AddMessages) to handle concurrent writes.",
        first_writer[channel.as_str()],
    )));
}`,
    },
    {
      type: "list",
      items: [
        "**Validation is all-or-nothing** — every channel is checked *before* a single mutation is applied, so a failed step leaves state untouched.",
        "**Fan-in is deterministic** — writes are sorted by node name (`collected.sort_by(|a, b| a.0.cmp(&b.0))`) before merging, so checkpoints are stable run-to-run.",
      ],
    },

    { type: "heading", level: 2, text: "Nodes" },
    {
      type: "paragraph",
      text: "A node is an async function — any `Fn(NodeContext) -> impl Future<Output = Result<NodeOutput>>` implements the `Node` trait via a **blanket impl**. It receives an **immutable snapshot** of state as of super-step start and returns a **partial update** plus an optional routing `Command`.",
    },
    {
      type: "callout",
      variant: "quote",
      text: "Snapshot isolation is structural, not conventional: two nodes in the same super-step physically cannot observe each other's writes.",
    },
    {
      type: "paragraph",
      text: "The snapshot is cloned per invocation — isolation is a property of the data structure, not a convention nodes are asked to follow.",
    },

    { type: "heading", level: 2, text: "The super-step loop (Pregel / BSP)" },
    {
      type: "paragraph",
      text: "Execution follows **Google Pregel / bulk-synchronous-parallel (BSP)**. Each iteration — one super-step — has six beats:",
    },
    {
      type: "list",
      ordered: true,
      items: [
        "**Plan** — compute the active set: the entry point on a fresh run, the checkpointed next-node set on resume or replay.",
        "**Run the active set in parallel** — each node is spawned on a `tokio::task::JoinSet` with its own immutable snapshot.",
        "**Barrier** — wait until every active node has finished; the only moment writes become visible.",
        "**Merge** — reducers fold the collected partial updates into state, all-or-nothing.",
        "**Route** — static edges, `Command::goto`, or a conditional router decides the next active set.",
        "**Checkpoint** — persist the step index, full channel state, and next-node set.",
      ],
    },
    {
      type: "paragraph",
      text: "The barrier makes shared-state parallelism safe and makes each step **transactional**: if any node fails or interrupts, the step's writes are discarded wholesale.",
    },
    {
      type: "paragraph",
      text: "A graph cycle — the ReAct loop `agent → tools → agent` — is **not call-stack recursion**. It is nodes being **re-scheduled across super-steps**. That is why the runaway-loop guard is a **step budget** (`max_steps`, default **1000**), not a stack limit.",
    },
    {
      type: "code",
      language: "rust",
      title: "One task per node, each with its own tracing span",
      code: `let node_span = tracing::info_span!("rusty.node", node = %name, step = step);
join_set.spawn(async move { (name, node.run(ctx).await) }.instrument(node_span));`,
    },

    { type: "heading", level: 2, text: "Versioned checkpoints" },
    {
      type: "paragraph",
      text: "At **every super-step boundary** the executor persists a `Checkpoint`: step index, full channel state, and the next-node set.",
    },
    {
      type: "callout",
      variant: "quote",
      text: "One primitive yields four features that are usually four subsystems: durable execution (resume after crash), human-in-the-loop (suspend, serialize, approve, resume), time travel (load any historical checkpoint, fork alternate timelines), and partial-failure recovery.",
    },
    {
      type: "paragraph",
      text: "Checkpoints happen **at boundaries, never mid-node** — resume re-executes a node from its start, so **node logic must be idempotent**.",
    },
    {
      type: "callout",
      variant: "quote",
      text: "That idempotency contract is the price of durability, and the engine states it plainly rather than hiding it.",
    },
    {
      type: "paragraph",
      text: "The `Checkpointer` trait is five methods — `put`, `get_latest`, `list`, `get_by_id`, `fork_thread` — with three savers implemented:",
    },
    {
      type: "list",
      items: [
        "`InMemoryCheckpointer` — dev/test.",
        "`JsonFileCheckpointer` — one JSON file per checkpoint under `{dir}/{thread_id}/`, atomic temp-file-then-rename writes, a `latest` pointer file, per-thread put serialization.",
        "`PostgresCheckpointer` — feature `postgres`, `sqlx`-backed.",
      ],
    },
    { type: "heading", level: 3, text: "Time travel is two operations" },
    {
      type: "paragraph",
      text: "`fork_thread(src, dst, at_checkpoint_id)` copies a thread's history (oldest first, full or truncated) into a new thread id; `RunConfig::with_checkpoint_id(id)` starts a run from that checkpoint's state and next-node set.",
    },
    {
      type: "callout",
      variant: "note",
      title: "The safe pattern",
      text: "**Fork first, replay on the fork.** Replaying on the original thread appends new history on top of the old timeline — legal (`get_latest` defines recency by **insertion order, not step number**) but usually not what you want.",
    },

    {
      type: "heading",
      level: 2,
      text: "Graph building — invalid topologies fail at compile(), not mid-run",
    },
    {
      type: "paragraph",
      text: "`GraphBuilder` is deliberately thin: register named nodes, add static edges (`from → to`; all destinations of multiple edges activate **in parallel**), add **at most one conditional edge per source** (an async router reading post-barrier state), and set the entry point. `compile()` freezes the graph into an immutable, `Arc`-shared `Graph` and rejects, **before any node or paid LLM call runs**:",
    },
    {
      type: "list",
      items: [
        "an empty graph",
        "a missing or dangling entry point",
        "edges referencing unknown nodes",
        "reserved node names (`__end__` and anything `__`-prefixed)",
        "duplicate static edges",
        "multiple conditional edges from one node",
        "**mixed routing** — static and conditional edges from the same source node:",
      ],
    },
    {
      type: "code",
      language: "rust",
      title: "Mixed routing is a compile-time error",
      code: `if let Some(from) = direct_sources.intersection(&conditional_sources).next() {
    return Err(RustyError::Graph(format!(
        "node \`{from}\` has both static and conditional edges; routing would \\
         be ambiguous — use one kind per source node"
    )));
}`,
    },
    {
      type: "paragraph",
      text: "Conditional router targets and `Send` node names are validated at execution time instead — they are data-dependent by design.",
    },

    { type: "heading", level: 2, text: "Routing — three kinds of “what runs next”" },
    {
      type: "paragraph",
      text: "The conditional router's vocabulary is three values:",
    },
    {
      type: "code",
      language: "rust",
      title: "Route",
      code: `pub enum Route {
    /// Activate exactly one node next.
    Node(String),
    /// Dynamic fan-out (LangGraph \`Send\` API): activate one node invocation
    /// per item, each with its own scoped input state. The canonical
    /// map-reduce pattern: items are generated at runtime, each mapped
    /// through a node, results fan back in through multi-write reducers.
    Send(Vec<Send>),
    /// Terminate the run.
    End,
}`,
    },
    {
      type: "paragraph",
      text: "`Route::Send` is the **map-reduce primitive**: items generated at runtime, each mapped through one node invocation with the item overlaid as scoped state; results fan back in through multi-write reducers. A node's own `Command::goto` output **overrides the static edge set entirely**; unknown targets (routers, `Send`s, commands) are executor errors naming the offending node. An empty next set ends the run.",
    },

    { type: "heading", level: 2, text: "The run, end to end" },
    {
      type: "paragraph",
      text: "`Executor::run` restores-or-seeds state, then loops `execute_super_step` until routing yields an empty next set (`Done`), a node interrupts (`Interrupted`), or `max_steps` trips (error). Terminal outcomes: `ExecutionOutcome::Done(state)` or `ExecutionOutcome::Interrupted { value, state, checkpoint_id }`.",
    },

    { type: "heading", level: 2, text: "Named failure modes" },
    {
      type: "paragraph",
      text: "Agent systems fail in a small number of characteristic ways. Each row names one, and Rusty's response:",
    },
    {
      type: "table",
      head: ["Failure mode", "Rusty's response"],
      rows: [
        [
          "**A node fails mid-step**",
          "The super-step is transactional: the JoinSet is dropped, stragglers abort, every write of the step is discarded, and the run errors naming the node and step. No half-applied state.",
        ],
        [
          "**Two parallel nodes write the same `LastValue` channel**",
          "`InvalidUpdate` at the barrier, before any mutation, naming both writers and prescribing a multi-write reducer.",
        ],
        [
          "**LLM endpoint returns 429 / 5xx / times out**",
          "Classified retryable; capped, jittered exponential backoff with `Retry-After` as a floor. Other 4xx are permanent and surface immediately. Node-level, LLM and tool errors are the retryable classes in executor telemetry.",
        ],
        [
          "**A tool throws or panics**",
          "Contained per call: the batch returns an `ERROR:` tool message in that call's slot, in order, and the model sees the failure as data.",
        ],
        [
          "**A second run arrives on a busy thread**",
          "One active run per thread, enforced by the `RunManager`: `reject` answers 409; `enqueue` (default) queues FIFO up to the configured depth, then 409.",
        ],
        [
          "**Replay leaves a stale “latest” head**",
          "Recency is insertion order, not step number: replay appends a new timeline and resume follows it; deterministic `(step, created_at, id)` listing keeps fork truncation stable across backends. The safe pattern is fork first, replay on the fork.",
        ],
        [
          "**A runaway graph cycle**",
          "A cycle is re-scheduling, not recursion, so the guard is a step budget: `max_steps` (default 1000) aborts with an error naming the likely infinite cycle.",
        ],
        [
          "**A guest WASM module loops forever or eats memory**",
          "Fuel metering traps the loop; a `ResourceLimiter` rejects memory growth past the cap; the guest has no imports at all — no WASI, no host functions.",
        ],
        [
          "**A hostile MCP server declares a giant frame**",
          "Inbound frames are capped at 16 MiB *before* any length-driven allocation; per-request timeouts bound waiting.",
        ],
        [
          "**A client probes another tenant's thread**",
          "Tenant isolation is id namespacing: the foreign thread does not exist in your scope, so the answer is 404 (never 403 — existence is not leaked); malformed client ids are rejected 400.",
        ],
      ],
    },

    { type: "heading", level: 2, text: "Glossary" },
    {
      type: "table",
      head: ["Term", "Definition"],
      rows: [
        [
          "**Channel**",
          "One key of the shared state, with a `Reducer` defining its merge semantics.",
        ],
        [
          "**Reducer**",
          "The per-channel merge function applied at the barrier (`Overwrite`, `Append`, `DeepMerge`, `AddMessages`).",
        ],
        [
          "**Super-step**",
          "One iteration of the executor: plan, parallel compute over immutable snapshots, barrier, merge, route, checkpoint. Transactional as a whole.",
        ],
        [
          "**Barrier**",
          "The point where all active nodes of a step have finished; the only moment writes become visible.",
        ],
        [
          "**Checkpoint**",
          "A versioned snapshot of one thread at a super-step boundary: step, state, next-node set.",
        ],
        [
          "**Thread**",
          "A session id that namespaces checkpoints; stable across interrupts, resumes, and replays.",
        ],
        [
          "**Interrupt**",
          "A node-initiated suspension of the whole run, resumable via a checkpoint and a resume value.",
        ],
        [
          "**Send**",
          "A routing instruction that fans one node out over runtime-generated items, each with scoped input state.",
        ],
        ["**Active set**", "The nodes scheduled to run in a super-step."],
      ],
    },
  ],
};
