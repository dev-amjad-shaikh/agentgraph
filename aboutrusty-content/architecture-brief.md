# Content Brief — Rusty Architecture & Roadmap (R1_Architecture)

**Audience:** web developers building aboutrusty.com who have NOT read the source docs.
**Sources:** `docs/architecture.md`, `docs/roadmap.md` (repo: github.com/dev-amjad-shaikh/rusty).
**Rule:** every fact, identifier, and code snippet below is verbatim or directly traceable to those two files. Do not invent beyond this.

---

## 0. One-line positioning (copy-pastable)

> **Rusty is the durable agent runtime built in Rust** — a full-Rust, LangGraph-style agentic platform.

Five pieces (use these exact names on the site):

| Piece | What it is | Crate / package |
|---|---|---|
| **Rusty Core** | The engine — no HTTP, no server deps | `rusty-agent-runtime` crate, in `rusty-core/` |
| **Rusty Server** | axum HTTP/SSE server, Agent-Protocol subset, one static binary | `rusty-server` |
| **Rusty Worker** | Worker SDK for remote nodes | `rusty-worker` |
| **rusty-otel** | One-call `tracing` subscriber + optional OTLP (OpenTelemetry 0.32, HTTP/protobuf) | `rusty-otel` |
| **Rusty SDK** | Zero-dependency Python + TypeScript clients | PyPI `rusty-agent-runtime` (imported as `rusty_client`), npm `@rusty-runtime/client` |
| **Rusty Studio** | Zero-build, single-file debug UI (`studio/index.html`) | `studio/` |

Dual-licensed **MIT OR Apache-2.0**.

Quickstart snippet (verbatim from the docs):

```bash
git clone https://github.com/dev-amjad-shaikh/rusty.git
cd rusty/rusty-server
cargo run --example server_demo   # serves a scripted ReAct agent on http://127.0.0.1:8100
# then open studio/index.html in a browser and connect to 127.0.0.1:8100
```

---

## 1. The anatomy of a run (the core mental model)

**Headline model:** *an agent is a graph over shared state, executed in super-steps.* Four primitives, each of which "exists to kill a specific failure class of agent systems."

### Primitive 1 — Typed state channels with reducers

- "Typed state" means **schema-declared JSON state with runtime validation**, not Rust-level typing.
- Nodes never call each other and never return whole state. Every state key is a **channel** whose `Reducer` defines how partial updates merge.
- The four reducers (verbatim names): `Overwrite` (LangGraph's `LastValue`), `Append`, `DeepMerge`, `AddMessages` (ID-aware message upsert — LangGraph's `add_messages`; a node can correct a message it wrote earlier by `"id"` while parallel tool results append alongside it).
- The `StateSpec` is the complete schema:
  - a write to an **undeclared channel is an error**;
  - a **second write to a single-write channel within one super-step is an error** (`InvalidUpdate` at the barrier, naming both writers).
- **Pull-quote:** *"In a parallel graph, two nodes silently clobbering the same key is otherwise the default outcome, and it surfaces only as a corrupted conversation three steps later. Here it is an `InvalidUpdate` error at the barrier, naming both writers."*
- Merge properties worth featuring:
  - **Validation is all-or-nothing** — every channel is checked *before* a single mutation is applied, so a failed step leaves state untouched.
  - **Fan-in is deterministic** — writes are sorted by node name (`collected.sort_by(|a, b| a.0.cmp(&b.0))`) before merging, so checkpoints are stable run-to-run.

The single-write rule, verbatim code (from `rusty-core/src/state.rs`):

```rust
if *count > 1 && !reducer.allows_multiple_writes() {
    return Err(RustyError::InvalidUpdate(format!(
        "channel `{channel}` can receive only one value per super-step \
         (reducer: {reducer}); already written by node `{}`, second write from \
         node `{node}`. Use a multi-write reducer (Append/DeepMerge/\
         AddMessages) to handle concurrent writes.",
        first_writer[channel.as_str()],
    )));
}
```

### Primitive 2 — Nodes

- A node is an async function — any `Fn(NodeContext) -> impl Future<Output = Result<NodeOutput>>` implements the `Node` trait via a **blanket impl**.
- It receives an **immutable snapshot** of state as of super-step start and returns a **partial update** plus an optional routing `Command`.
- **Pull-quote:** *"Snapshot isolation is structural, not conventional: two nodes in the same super-step physically cannot observe each other's writes"* (the snapshot is cloned per invocation).

### Primitive 3 — The super-step loop (Pregel / BSP)

- Execution model: **Google Pregel / bulk-synchronous-parallel (BSP)**.
- The six-beat loop (use this exact phrasing as a stepper/graphic): **plan → run the active set in parallel → barrier → merge → route → checkpoint**.
- The barrier makes shared-state parallelism safe and makes each step **transactional**: if any node fails or interrupts, the step's writes are discarded wholesale.
- **Key teaching point:** a graph cycle (the ReAct loop `agent → tools → agent`) is **not call-stack recursion** — it is nodes being **re-scheduled across super-steps**. That is why the runaway-loop guard is a **step budget** (`max_steps`, default **1000**), not a stack limit.
- Compute is a `tokio::task::JoinSet`; each node is spawned with its own tracing span:

```rust
let node_span = tracing::info_span!("rusty.node", node = %name, step = step);
join_set.spawn(async move { (name, node.run(ctx).await) }.instrument(node_span));
```

### Primitive 4 — Versioned checkpoints

- At **every super-step boundary** the executor persists a `Checkpoint`: step index, full channel state, and the next-node set.
- **Pull-quote:** *"One primitive yields four features that are usually four subsystems"*: durable execution (resume after crash), human-in-the-loop (suspend, serialize, approve, resume), time travel (load any historical checkpoint, fork alternate timelines), partial-failure recovery.
- Checkpoints happen **at boundaries, never mid-node** — resume re-executes a node from its start, so **node logic must be idempotent**. *"That idempotency contract is the price of durability, and the engine states it plainly rather than hiding it."*
- The `Checkpointer` trait is five methods: `put`, `get_latest`, `list`, `get_by_id`, `fork_thread`.
- Three savers implemented:
  - `InMemoryCheckpointer` (dev/test)
  - `JsonFileCheckpointer` — one JSON file per checkpoint under `{dir}/{thread_id}/`, atomic temp-file-then-rename writes, a `latest` pointer file, per-thread put serialization
  - `PostgresCheckpointer` (feature `postgres`, `sqlx`-backed)
- **Time travel = two operations:** `fork_thread(src, dst, at_checkpoint_id)` copies a thread's history (oldest first, full or truncated) into a new thread id; `RunConfig::with_checkpoint_id(id)` starts a run from that checkpoint's state and next-node set.
- **The safe pattern (memorable rule): "Fork first, replay on the fork."** Replaying on the original thread appends new history on top of the old timeline — legal (`get_latest` defines recency by **insertion order, not step number**) but usually not what you want.

### The run, end to end

`Executor::run` restores-or-seeds state, then loops `execute_super_step` until routing yields an empty next set (`Done`), a node interrupts (`Interrupted`), or `max_steps` trips (error). Terminal outcomes: `ExecutionOutcome::Done(state)` or `ExecutionOutcome::Interrupted { value, state, checkpoint_id }`.

### Graph building — invalid topologies fail at `compile()`, not mid-run

`GraphBuilder` is deliberately thin: register named nodes, add static edges (`from → to`; all destinations of multiple edges activate **in parallel**), add **at most one conditional edge per source** (an async router reading post-barrier state), set the entry point. `compile()` freezes the graph into an immutable, `Arc`-shared `Graph` and rejects, **before any node or paid LLM call runs**:

- an empty graph
- a missing or dangling entry point
- edges referencing unknown nodes
- reserved node names (`__end__` and anything `__`-prefixed)
- duplicate static edges
- multiple conditional edges from one node
- **mixed routing** (verbatim error):

```rust
if let Some(from) = direct_sources.intersection(&conditional_sources).next() {
    return Err(RustyError::Graph(format!(
        "node `{from}` has both static and conditional edges; routing would \
         be ambiguous — use one kind per source node"
    )));
}
```

Conditional router targets and `Send` node names are validated at execution time instead — they are data-dependent by design.

### Routing — three kinds of "what runs next"

The conditional router's vocabulary is three values (verbatim):

```rust
pub enum Route {
    /// Activate exactly one node next.
    Node(String),
    /// Dynamic fan-out (LangGraph `Send` API): activate one node invocation
    /// per item, each with its own scoped input state. The canonical
    /// map-reduce pattern: items are generated at runtime, each mapped
    /// through a node, results fan back in through multi-write reducers.
    Send(Vec<Send>),
    /// Terminate the run.
    End,
}
```

- `Route::Send` is the **map-reduce primitive**: items generated at runtime, each mapped through one node invocation with the item overlaid as scoped state; results fan back in through multi-write reducers.
- A node's own `Command::goto` output **overrides the static edge set entirely**; unknown targets (routers, `Send`s, commands) are executor errors naming the offending node. An empty next set ends the run.

### Human-in-the-loop — "an interrupt is a transaction abort with a receipt"

- A node suspends the run by returning `Err(ctx.interrupt(payload))` (`NodeContext::interrupt`).
- The suspension is **run-wide**: the in-flight step's writes are discarded — **including writes from sibling nodes that already completed** — still-running siblings are aborted, and the suspension checkpoint **re-schedules the entire active set** of the step, not just the interrupting node. *"Anything less would silently lose the siblings' discarded work."*
- Caller receives `ExecutionOutcome::Interrupted { value, state, checkpoint_id }`.
- Resume: same `thread_id` + `RunConfig::with_resume(value)`. Every node of the suspended set re-executes from its start; the resume value is **broadcast to all of them for the first super-step**, so a resumable node checks `ctx.resume_value()` **first** and must be idempotent in everything it did before interrupting.

### LLM and tools — "the model is one node, the loop is the graph"

- `ChatModel` trait: `chat(messages, tool_schemas)` in, one assistant `ChatMessage` (text and/or `tool_calls`) out; `chat_stream` adds a token-delta callback.
- One client: `OpenAiCompatibleClient` — works against **OpenAI, vLLM, Ollama, LM Studio** and compatible gateways.
- Failure classification: connect errors, timeouts, HTTP 5xx, 408 and 429 are **retryable** with capped, jittered exponential backoff (`Retry-After` floors the delay); other 4xx are **permanent**.
- Tools: `Tool` trait, `ToolRegistry` (emits OpenAI-format schemas), `ToolExecutor::execute_batch` — dispatches a batch concurrently, **preserves call order, isolates failures**: a failing or even panicking tool becomes an `ERROR:` tool message the model can read and recover from, never a batch abort:

```rust
match result {
    Ok(Ok(content)) => ChatMessage::tool_result(&call.id, content),
    Ok(Err(e)) => ChatMessage::tool_result(&call.id, format!("ERROR: {e}")),
```

- `create_react_agent` assembles the classic loop as a **two-node cyclic graph** over a single `messages` channel with the `AddMessages` reducer: `agent` node (calls model, appends assistant message), `tools` node (dispatches pending tool calls), conditional edge `agent → tools | End`, static edge `tools → agent`. **Pull-quote:** *"The cycle is super-steps, not recursion — each hop is a full plan/barrier/merge/route/checkpoint pass, so a ReAct agent gets durability and HITL for free."*
- `create_react_agent_streaming` forwards token deltas as `GraphEvent::Token` — the LangGraph `messages` stream mode.

### Three ways code enters a graph

The `Node` trait is the only seam; the engine runs three kinds of code "without being able to tell them apart":

1. **Native nodes** — async closures (above).
2. **Remote nodes** (`RemoteNode`) — serialize the invocation (protocol version, node name, the same immutable super-step snapshot, `NodeConfig`) and POST to a worker's `/execute` endpoint (protocol v1). Reply carries exactly one of `output`, `error`, or `interrupt`; an interrupt surfaces locally as `RustyError::Interrupt`, so a remote node suspends/resumes exactly like a local one. **HITL interrupts cross the wire.** Retries are deliberately narrow: only transport-class failures (connect, timeout, 5xx/408/429), **never worker-reported errors** — "the worker already made a definitive decision." The `rusty-worker` crate serves the other end.
3. **WASM nodes** (`WasmNode`, feature `wasm`) — run untrusted/community modules via **Wasmtime** behind a JSON-in/JSON-out ABI. **The sandbox is three walls:** fuel metering aborts infinite loops with a trap, a `ResourceLimiter` caps memory growth, and the guest instantiates with an **empty `Linker` — no WASI, no host functions, no ambient authority**:

```rust
impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            fuel: 10_000_000,
            max_memory_bytes: 16 * 1024 * 1024, // 16 MiB
        }
    }
}
```

**MCP tools are not nodes at all:** the `mcp` module is a JSON-RPC client over stdio (newline-delimited or `Content-Length` framing, per-request timeouts, a 16 MiB frame cap against hostile length prefixes); `McpClient::into_tools()` lists a server's tools and returns them as `Arc<dyn Tool>` for direct registration in a `ToolRegistry` — same `ToolExecutor`, same ReAct graph, **zero graph changes**.

### The server around the engine

- *"`rusty-server` adds nothing to the execution semantics; it exposes them."* You call `rusty_server::serve(registry, config)` from your own `main.rs` and deploy the result as **one static binary**.
- Resource model (use as a feature table): **Threads** (session bound to a registered graph; namespaces all checkpoints; `GET /threads/{id}/state`, `POST .../history`, `POST .../fork`), **Runs** (three submission modes: background `202 + run_id`, blocking `runs/wait`, streaming `runs/stream`), **Assistants** (named graph aliases with config metadata; `assistant_id` inherits `recursion_limit`), **Crons** (interval or 5-field cron expression), **KV store** (namespaced JSON documents, `PUT/GET/DELETE /store/{ns}/{key}`).
- **Concurrency is one rule: at most one active run per thread** (`RunManager`). A second submission on a busy thread: `multitask_strategy: "reject"` → 409, or default `enqueue` → per-thread FIFO capped by `ServerConfig::max_concurrent_runs_per_thread`.
- SSE frame ids: `{checkpoint_id}:{step}:{seq}`; attach endpoint `GET /runs/{id}/stream` honors `Last-Event-ID` (replay bounded event log, then follow live broadcast).
- **Multi-tenancy is namespacing, not filtering.** `ServerConfig::with_tenant_key(tenant, key)` maps `X-Api-Key` to tenants; every tenant's resources live under a `{tenant}/` id prefix — **cross-tenant access answers 404, never 403** (existence is not leaked). No keys configured → open dev mode, byte-identical, everything in the `default` tenant. `ServerConfig::with_postgres(url)` (feature `postgres`) moves checkpoints and the whole platform surface into auto-migrated Postgres tables.

### Observability

The executor emits `tracing` telemetry; the library installs no subscriber — the application chooses one. Span taxonomy mirrors the loop:

- `rusty.run` (INFO) — one per `Executor::run`; fields `thread_id`, `max_steps`, `resume`, `replay`. Parent of everything below.
- `rusty.super_step` (DEBUG) — fields `step`, `active_nodes`.
- `rusty.node` (INFO) — fields `node`, `step`.
- Events: DEBUG on each barrier merge; INFO on interrupt and run completion (`steps`, `duration_ms`); WARN on node failure with a `retryable` classification.

The `rusty-otel` crate turns this on in one call — a run shows up in your collector as a run span with super-step and node children, no instrumentation code of your own.

---

## 2. Named failure modes (verbatim table — ideal as a site section)

Agent systems fail in a small number of characteristic ways. Each row names one, and Rusty's response:

| Failure mode | Rusty's response |
|---|---|
| **A node fails mid-step** | The super-step is transactional: the JoinSet is dropped, stragglers abort, every write of the step is discarded, and the run errors naming the node and step. No half-applied state. |
| **Two parallel nodes write the same `LastValue` channel** | `InvalidUpdate` at the barrier, before any mutation, naming both writers and prescribing a multi-write reducer. |
| **LLM endpoint returns 429 / 5xx / times out** | Classified retryable; capped, jittered exponential backoff with `Retry-After` as a floor. Other 4xx are permanent and surface immediately. Node-level, LLM and tool errors are the retryable classes in executor telemetry. |
| **A tool throws or panics** | Contained per call: the batch returns an `ERROR:` tool message in that call's slot, in order, and the model sees the failure as data. |
| **A second run arrives on a busy thread** | One active run per thread, enforced by the `RunManager`: `reject` answers 409; `enqueue` (default) queues FIFO up to the configured depth, then 409. |
| **Replay leaves a stale "latest" head** | Recency is insertion order, not step number: replay appends a new timeline and resume follows it; deterministic `(step, created_at, id)` listing keeps fork truncation stable across backends. The safe pattern is fork first, replay on the fork. |
| **A runaway graph cycle** | A cycle is re-scheduling, not recursion, so the guard is a step budget: `max_steps` (default 1000) aborts with an error naming the likely infinite cycle. |
| **A guest WASM module loops forever or eats memory** | Fuel metering traps the loop; a `ResourceLimiter` rejects memory growth past the cap; the guest has no imports at all — no WASI, no host functions. |
| **A hostile MCP server declares a giant frame** | Inbound frames are capped at 16 MiB *before* any length-driven allocation; per-request timeouts bound waiting. |
| **A client probes another tenant's thread** | Tenant isolation is id namespacing: the foreign thread does not exist in your scope, so the answer is 404 (never 403 — existence is not leaked); malformed client ids are rejected 400. |

---

## 3. The eight diagrams — described in words for a designer

The source docs contain exactly eight mermaid diagrams. Rebuild each as a web visual as follows:

### D1. Platform map ("Orientation")
A left-to-right architecture map. Center-left: a container box labeled **"Rusty Core — no HTTP"** holding three linked internals: **Executor** → **State + Reducers**, Executor → **Graph**, Executor → **Checkpointer**. Around it: **Rusty Server** (axum HTTP + SSE) feeds into the Executor; the Executor calls out to **Rusty Worker** ("HTTP, protocol v1"); **rusty-otel** attaches via a dashed line ("tracing spans"); **Rusty SDK** (Python + TS clients) and **Rusty Studio** (debug UI) both connect to the Server via "HTTP + SSE"; the Executor also reaches outward to an **OpenAI-compatible LLM endpoint** ("ChatModel") and **MCP tool servers** ("MCP over stdio"). Visual message: *everything hangs off one crate; Core has no HTTP.*

### D2. One run, end to end (sequence)
A five-actor sequence diagram: **Caller → Executor → Nodes (JoinSet) → StateSpec reducers → Checkpointer**. Flow: Caller sends `run(graph, spec, state, RunConfig)`; Executor plans the active set (entry point, resume, or replay); spawns one task per active node, each with an immutable snapshot; barrier collects `NodeOutput`, failure, or Interrupt; Executor calls `apply_super_step(writes)` on the reducers, which return merged, single-write-validated state; Executor routes (static edges, `Command`, `Route` or `Send`); Executor `put(Checkpoint)` at the step boundary; finally returns `Done(state)` or `Interrupted(payload, checkpoint_id)` to the Caller.

### D3. Routing decision tree
A top-down flowchart starting at **"Barrier merged — post-step state"**. First diamond: "Any `Command` goto?" → yes: "Activate goto targets, deduped". No → second diamond: "Outgoing edges of nodes that ran" → "Direct" activates the target; "Conditional" goes to a third diamond "Router returns" with three branches: **Route Node** (activate target), **Route Send** ("One invocation per item, scoped state"), **Route End** ("Terminate the run"). All activation branches converge on **"Next active set"**.

### D4. Time travel (sequence)
Three actors: **Caller, Checkpointer, Executor**. Caller → `fork_thread(t1, t2, at checkpoint_id)`; Checkpointer replies "copied N checkpoints, oldest first"; Caller → `run(t2, with_checkpoint_id)`; Executor → `get_by_id(t2, checkpoint_id)`; Checkpointer returns "state + step + next_nodes"; Executor continues from that boundary and puts new checkpoints onto t2; returns `ExecutionOutcome`. Visual message: *fork the timeline, then replay on the fork.*

### D5. Human-in-the-loop interrupt (sequence)
Four actors: **Caller, Executor, approve node, Checkpointer**. Caller runs thread t; Executor runs the approve node with `resume_value None`; node replies `Err(Interrupt(payload))`; Executor discards step writes and aborts siblings; Checkpointer stores a checkpoint whose next set is the **entire active set**; Caller receives `Interrupted(payload, checkpoint_id)`; Caller re-runs thread t `with_resume(decision)`; Executor re-runs the node from the start with `resume_value Some`; node returns `NodeOutput(approval)`; run ends `Done(state)`.

### D6. The ReAct loop (sequence)
Six actors: **Executor, agent node, ChatModel, tools node, ToolExecutor, event channel**. Super-step 1: Executor runs `agent`, which calls `chat(messages, tool schemas)` on the model; model returns an assistant message with `tool_calls`; (streaming variant: `GraphEvent` Token deltas go to the event channel); agent updates the `messages` channel; Executor routes — tool_calls present. Super-step 2: Executor runs `tools`, which calls `execute_batch(tool_calls)`; ToolExecutor returns results in call order, ERROR isolated; tools appends tool messages; Executor loops back to `agent` for the next super-step. Visual message: *a two-node cyclic graph — `agent → tools → agent` — each hop a full super-step.*

### D7. Three ways code enters a graph (flowchart)
Left: **"Graph, compiled"** branches to three node kinds: **Native closure Node**, **RemoteNode**, **WasmNode (wasm feature)**. RemoteNode connects to **Rusty Worker handler** ("POST /execute, protocol v1"); WasmNode connects to **guest module, no imports** ("wasmtime — fuel + memory caps"). Separately below: **MCP server over stdio** → ("`McpClient into_tools`") → **ToolRegistry** → **tools node, ToolExecutor**. Visual message: *one `Node` seam, three kinds of code behind it — plus MCP, which enters as tools, not nodes.*

### D8. The server around the engine (sequence)
Five actors: **Client, axum routes + auth, RunManager, Executor + Checkpointer, SSE stream**. Client `POST /threads/id/runs/stream`; routes map `X-Api-Key` to tenant scope; RunManager `insert(run, enqueue or reject)` → "Started — one active run per thread"; routes spawn `run(graph, spec, RunConfig)` on the Executor; super-steps and checkpoints happen; frames (`metadata`, `updates`, `values`, `messages`) flow to the SSE stream; an end frame carries terminal status; the Client can replay via `GET /runs/id/stream` + `Last-Event-ID`.

---

## 4. Roadmap — phases, status, rejections

**Naming system (use these on the site):** named releases **R0.1 — Ignition**, **R0.2 — Persistence**, **R0.3 — Interop**, **R0.4 — Time Travel** (all implemented), and **R1.0 — Unleashed** (upcoming). Crates are versioned independently; phases group work across the monorepo.

### Status table (verbatim facts)

| Release | Phase | Contents | Version target | Status | Date |
|---|---|---|---|---|---|
| **R0.1 — Ignition** | Core kernel | State channels + reducers, Pregel/BSP executor, checkpoints, HITL interrupts, `Send` fan-out, `ChatModel`/`ToolExecutor`, prebuilt ReAct agent | `rusty-agent-runtime` v0.1.0 | ✅ Implemented | 2026-07-31 |
| **R0.2 — Persistence** | Durability & streaming + server Phase A | Postgres checkpointer, token streaming (`messages` mode), live example; axum server: threads, runs, SSE, auth | `rusty-agent-runtime` v0.2.0, `rusty-server` v0.1.0 | ✅ Implemented | 2026-08-05 |
| **R0.3 — Interop** | Interop & distribution | MCP client, remote nodes + worker SDK, server API completion, executor tracing | `rusty-agent-runtime` v0.3.0, `rusty-server` v0.2.0, `rusty-worker` v0.1.0 | ✅ Implemented | 2026-08-05 |
| **R0.4 — Time Travel** | Production hardening | WASM nodes, time-travel core + server API, Postgres server store, OpenTelemetry export, Studio UI, permissive CORS | `rusty-agent-runtime` v0.4.0, `rusty-server` v0.3.0, `rusty-otel` v0.1.0 | ✅ Implemented | 2026-08-05 |
| **v0.5 (pre-1.0)** | SDKs & tenancy | Python SDK (stdlib-only), TypeScript SDK (zero-dep ESM), multi-tenant auth with full isolation, live-LLM validation + calculator fix | `rusty-server` v0.4.0, `sdks/*` v0.1.0 | ✅ Implemented | 2026-08-05 |
| **R1.0 — Unleashed** | Platform ambitions | Hosted multi-tenant service (tenant isolation implemented in v0.5 — the first brick), WASM target, edge runtimes | TBD | 🚧 Upcoming | — |

### Phase detail (one-liners per phase, faithful to the docs)

- **R0.1 — Ignition:** The LangGraph execution model rebuilt on tokio: channels with per-key `Reducer`s over schema-declared, runtime-validated JSON state; `GraphBuilder::compile()` validation; the Pregel/BSP super-step executor; versioned thread-scoped checkpoints (in-memory + JSON-file); interrupt/resume HITL; `Route::Send` dynamic fan-out; typed `GraphEvent` streaming; minimal `ChatModel` with an OpenAI-compatible client; parallel `ToolExecutor`; `react::create_react_agent`.
- **R0.2 — Persistence:** `sqlx`-backed `PostgresCheckpointer` (`postgres` feature); real token streaming (`ChatModel::chat_stream` → `GraphEvent::Token`, the LangGraph `messages` stream mode); live-agent example against any OpenAI-compatible endpoint; new `rusty-server` crate (Phase A: threads, background/blocking/SSE runs, checkpoint history, per-thread run queue, API-key auth — 10 integration tests green).
- **R0.3 — Interop (four concurrent workstreams):** MCP client (stdio transport, plugs into `ToolRegistry`/`ToolExecutor` like native tools — the prebuilt ReAct agent drives them with no graph changes); remote nodes + `rusty-worker` (`RemoteNode` POSTs execution to workers; HITL interrupts cross the wire); server API completion (`GET /runs/{id}`, assistants, crons, KV store — 20 integration tests green); executor tracing (spans per super-step, node, checkpoint).
- **R0.4 — Time Travel (five concurrent workstreams):** WASM nodes (`WasmNode` via Wasmtime — untrusted-code isolation behind the same `Node` trait, without a separate worker fleet); time travel (`Checkpointer::get_by_id`/`fork_thread`, `RunConfig::with_checkpoint_id`; server: `POST /threads/{id}/fork`, `"checkpoint": {"checkpoint_id": …}` replay on all three run endpoints — *fork first, replay on the fork*); Postgres server store (`ServerConfig::with_postgres(url)` — `server_*` tables, auto-migrated, migrations serialize on a transaction-scoped advisory lock so concurrent cold boots are safe); OpenTelemetry export (new `rusty-otel` crate); Studio (zero-build single-file UI; server layers permissive CORS in `router()` — restrict in production).
- **v0.5 — SDKs & tenancy (pre-1.0):** Python SDK (zero-dependency, stdlib-only — `urllib.request` + `json`; full thread/run/SSE/time-travel/assistant/cron/KV surface; e2e suite boots the real `server_demo` binary; "the polyglot path the rejected PyO3/napi-rs bindings were traded for"); TypeScript SDK (zero-dependency ESM, Node ≥ 18 and browsers, global `fetch`, async-generator `runStream`, hand-written type declarations); multi-tenant auth (`ServerConfig::with_tenant_key(tenant, key)`, `{tenant}/` id prefixing, 404-on-cross-tenant, open/dev mode byte-identical — **"the first brick of the hosted control plane"**); live-LLM validation against real Ollama models + a calculator arg-parsing fix (quoted numeric args like `"128"` failed `as_f64()` and silently computed `0 op 0`; now coerces numeric strings and alias keys, with 5 unit tests).

### Explicitly rejected (verbatim — a great "convictions" section)

- **napi-rs / PyO3 bindings — REJECTED:** *"they'd freeze a trait surface that's still moving and split maintenance across three ecosystems; the HTTP/SSE server is the polyglot interop layer instead."*
- **`cdylib` / C ABI — REJECTED:** *"a C ABI over async tokio graphs leaks runtime-ownership and panic-safety problems across the boundary for near-zero demand; embed the Rust crate directly or talk HTTP."*

Site-ready summary line: **"The server is the polyglot interop layer by design."**

### R1.0 — Unleashed (upcoming — "directional, not scheduled")

Three ambitions:
1. **Hosted multi-tenant service** — the server crate operated as a managed platform: tenant isolation, durable queues, autoscaling workers. **Partially started:** v0.5 implemented the tenant-isolation brick (per-tenant API keys, namespaced storage, 404-on-cross-tenant semantics) in `rusty-server` v0.4.0; durable queues and autoscaling remain open.
2. **WASM target** — run graphs themselves in the browser or edge runtimes (sans native checkpointers).
3. **Edge deployment** — single-digit-MB agent services on edge runtimes, leaning on Rust's footprint and the static-binary story.

---

## 5. Copy-paste assets for the site

### "The whole thing, in one page of code" (full HITL example — hero code block)

```rust
use rusty_agent_runtime::prelude::*;
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. State schema: three channels, one writer each — LastValue semantics.
    let spec = StateSpec::new()
        .channel("draft", Reducer::Overwrite)
        .channel("approval", Reducer::Overwrite)
        .channel("published", Reducer::Overwrite);

    // 2. Nodes: async closures implement Node via a blanket impl.
    let mut builder = GraphBuilder::new();
    builder.add_node("draft", |_ctx: NodeContext| async move {
        Ok(NodeOutput::update("draft", json!("Ship the anatomy README")))
    });
    // The resumable node: check resume_value() FIRST; interrupt only when
    // no human decision exists yet. On resume it re-runs from the top.
    builder.add_node("approve", |ctx: NodeContext| async move {
        match ctx.resume_value() {
            Some(decision) => Ok(NodeOutput::update("approval", decision.clone())),
            None => Err(ctx.interrupt(json!({"prompt": "Approve this draft?"}))),
        }
    });
    builder.add_node("publish", |ctx: NodeContext| async move {
        let draft = ctx.state().get("draft").cloned().unwrap_or(Value::Null);
        Ok(NodeOutput::update("published", json!({"draft": draft})))
    });

    // 3. Edges: draft -> approve statically; approve routes on post-barrier state.
    builder.set_entry_point("draft");
    builder.add_edge("draft", "approve");
    builder.add_conditional_edges("approve", |state| async move {
        let approved = state
            .get("approval")
            .and_then(|a| a.get("approved"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(if approved { Route::Node("publish".into()) } else { Route::End })
    });
    let graph = builder.compile()?;   // topology validated here, before any node runs

    // 4. Executor with checkpoints: one per super-step boundary.
    let executor = Executor::with_checkpointer(Arc::new(InMemoryCheckpointer::new()));
    let thread = "anatomy-demo";      // the thread id is the resume handle

    // Phase 1: runs draft, then approve interrupts — the step is discarded,
    // a suspension checkpoint re-scheduling `approve` is persisted.
    let outcome = executor
        .run(&graph, &spec, State::new(), RunConfig::new(thread))
        .await?;
    assert!(outcome.is_interrupted());

    // Phase 2: same thread id + a resume value. The checkpointed state takes
    // precedence over the State::new() argument; approve re-executes with
    // resume_value() == Some(decision), routes to publish, and terminates.
    let decision = json!({"approved": true, "reviewer": "alice"});
    let outcome = executor
        .run(&graph, &spec, State::new(), RunConfig::new(thread).with_resume(decision))
        .await?;
    match outcome {
        ExecutionOutcome::Done(state) => println!("{}", state.to_value()),
        ExecutionOutcome::Interrupted { .. } => unreachable!("already approved"),
    }
    Ok(())
}
```

Runnable example names (for links): `react_agent`, `parallel_fanout`, `human_in_loop`, `live_agent` — in `rusty-core/examples/`.

### Glossary (site glossary page, verbatim)

- **Channel** — one key of the shared state, with a `Reducer` defining its merge semantics.
- **Reducer** — the per-channel merge function applied at the barrier (`Overwrite`, `Append`, `DeepMerge`, `AddMessages`).
- **Super-step** — one iteration of the executor: plan, parallel compute over immutable snapshots, barrier, merge, route, checkpoint. Transactional as a whole.
- **Barrier** — the point where all active nodes of a step have finished; the only moment writes become visible.
- **Checkpoint** — a versioned snapshot of one thread at a super-step boundary: step, state, next-node set.
- **Thread** — a session id that namespaces checkpoints; stable across interrupts, resumes, and replays.
- **Interrupt** — a node-initiated suspension of the whole run, resumable via a checkpoint and a resume value.
- **Send** — a routing instruction that fans one node out over runtime-generated items, each with scoped input state.
- **Active set** — the nodes scheduled to run in a super-step.

### Memorable pull-quotes (marketing copy, all verbatim)

1. "An agent is a graph over shared state, executed in super-steps."
2. "One primitive yields four features that are usually four subsystems." (checkpoints → durability, HITL, time travel, partial-failure recovery)
3. "An interrupt is a transaction abort with a receipt."
4. "The model is one node, the loop is the graph."
5. "Snapshot isolation is structural, not conventional."
6. "The cycle is super-steps, not recursion — a ReAct agent gets durability and HITL for free."
7. "`rusty-server` adds nothing to the execution semantics; it exposes them."
8. "Multi-tenancy is namespacing, not filtering."
9. "Fork first, replay on the fork."
10. "That idempotency contract is the price of durability, and the engine states it plainly rather than hiding it."

### Key numbers

- `max_steps` default: **1000**
- WASM `SandboxLimits` default: **fuel 10,000,000**, memory **16 MiB**
- MCP inbound frame cap: **16 MiB**
- Node.js floor for TS SDK: **Node ≥ 18**
- Integration tests cited: **10** green (server Phase A, v0.2), **20** green (server API completion, v0.3)
- `Checkpointer` trait: **5 methods**; reducers: **4**; routing outcomes: **3** (`Route::Node`, `Route::Send`, `Route::End`); super-step beats: **6**

---

## 6. Source traceability notes

- All code snippets are copied verbatim from `docs/architecture.md` (which itself cites file:line in `rusty-core/src/…`); the full HITL example assembles from `rusty-core/examples/human_in_loop.rs` and `rusty-core/README.md` per the docs.
- Roadmap statuses and dates are verbatim from `docs/roadmap.md` and the architecture doc's release table. R1.0 — Unleashed is explicitly "directional, not scheduled" — do not present it with a date.
- Crate versions as of the docs: `rusty-agent-runtime` 0.4.0, `rusty-server` 0.4.0 (roadmap table; architecture doc's crate table lists server at 0.4.0 as well), `rusty-worker` 0.1.0, `rusty-otel` 0.1.0.
