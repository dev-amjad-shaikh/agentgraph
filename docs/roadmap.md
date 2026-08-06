# agentgraph platform roadmap

Where the platform has been, what's landing this cycle, and what's next. Crates are versioned independently (`agentgraph` core, `agentgraph-server`, `agentgraph-worker`); phases below group work across the monorepo. History lives in [../CHANGELOG.md](../CHANGELOG.md); per-crate detail lives in each crate's README.

## Status at a glance

| Phase | Contents | Version target | Status | Date |
|---|---|---|---|---|
| Core kernel | State channels + reducers, Pregel/BSP executor, checkpoints, HITL interrupts, `Send` fan-out, `ChatModel`/`ToolExecutor`, prebuilt ReAct agent | `agentgraph` v0.1.0 | ✅ Shipped | 2026-07-31 |
| Durability & streaming + server Phase A | Postgres checkpointer, token streaming (`messages` mode), live example; axum server: threads, runs, SSE, auth | `agentgraph` v0.2.0, `agentgraph-server` v0.1.0 | ✅ Shipped | 2026-08-05 |
| **v0.3 — interop & distribution** | MCP client, remote nodes + worker SDK, server API completion, executor tracing | `agentgraph` v0.3.0, `agentgraph-server` v0.2.0, `agentgraph-worker` v0.1.0 | ✅ Shipped | 2026-08-05 |
| **v0.4 — production hardening** | WASM nodes, time-travel core + server API, Postgres server store, OpenTelemetry export, Studio UI, permissive CORS | `agentgraph` v0.4.0, `agentgraph-server` v0.3.0, `agentgraph-otel` v0.1.0 | ✅ Shipped | 2026-08-05 |
| Phase D — platform ambitions | Hosted multi-tenant service, WASM target, edge runtimes | TBD | 🌌 Ambitions | — |

## Shipped

### Phase: core kernel — `agentgraph` v0.1.0 (2026-07-31)

The LangGraph execution model rebuilt on tokio: typed state channels with per-key `Reducer`s, compile-time graph validation, the Pregel/BSP super-step executor (*plan → parallel → barrier → merge → route → checkpoint*), versioned thread-scoped checkpoints (in-memory + JSON-file), interrupt/resume HITL, `Route::Send` dynamic fan-out, typed `GraphEvent` streaming, the minimal `ChatModel` trait with an OpenAI-compatible client, parallel `ToolExecutor`, and `react::create_react_agent`. Details: [CHANGELOG 2026-07-31](../CHANGELOG.md).

### Phase: durability & streaming + server Phase A — `agentgraph` v0.2.0, `agentgraph-server` v0.1.0 (2026-08-05)

Core gained the `sqlx`-backed `PostgresCheckpointer` (`postgres` feature), real token streaming (`ChatModel::chat_stream` → `GraphEvent::Token`, the LangGraph `messages` stream mode), and a live-agent example against any OpenAI-compatible endpoint. The new `agentgraph-server` crate shipped Phase A of the Agent-Protocol surface: threads, background/blocking/SSE runs, checkpoint history, per-thread run queue, API-key auth — 10 integration tests green. Details: [CHANGELOG 2026-08-05](../CHANGELOG.md), [server design doc](agentgraph-server-design.md), [server quickstart](server-quickstart.md).

### Phase: v0.3 — interop & distribution — `agentgraph` v0.3.0, `agentgraph-server` v0.2.0, `agentgraph-worker` v0.1.0 (2026-08-05)

Four workstreams landed concurrently this cycle:

- **MCP client** (`agentgraph/src/mcp.rs`) — call any MCP server's tools from `agentgraph` `Tool` impls over stdio transport. MCP tool servers plug into `ToolRegistry` / `ToolExecutor` exactly like native tools, so the prebuilt ReAct agent can drive them with no graph changes.
- **Remote nodes + `agentgraph-worker`** (`agentgraph/src/remote.rs`, new crate) — `RemoteNode` POSTs node execution to worker services over HTTP; the `agentgraph-worker` SDK serves user handlers; HITL interrupts cross the wire, so a remote node can suspend the run and resume it with a human payload just like a local node.
- **Server API completion** (`agentgraph-server` v0.2) — fills out the Agent-Protocol surface from the [design doc](agentgraph-server-design.md): `GET /runs/{id}`, assistants, crons, and the KV store — 20 integration tests green.
- **Executor tracing** — `tracing` instrumentation through the super-step loop (spans per super-step, node, checkpoint), the foundation for the OpenTelemetry export candidate below.

### Phase: v0.4 — production hardening — `agentgraph` v0.4.0, `agentgraph-server` v0.3.0, `agentgraph-otel` v0.1.0 (2026-08-05)

Five workstreams landed concurrently this cycle:

- **WASM nodes** (`agentgraph/src/wasm_node.rs`, feature `wasm`) — `WasmNode` runs sandboxed WebAssembly modules as graph nodes via Wasmtime: untrusted-code isolation behind the same `Node` trait, without a separate worker fleet.
- **Time travel** — core gained `Checkpointer::get_by_id` / `Checkpointer::fork_thread` and `RunConfig::with_checkpoint_id`; the server exposes them as `POST /threads/{id}/fork` (full- or mid-history forks) and `"checkpoint": {"checkpoint_id": …}` replay on all three run endpoints. Fork first, replay on the fork.
- **Postgres server store** (`agentgraph-server`, feature `postgres`) — `ServerConfig::with_postgres(url)` moves run checkpoints *and* the assistants/crons/KV surface into Postgres (`server_*` tables, auto-migrated on first use; migrations serialize on a transaction-scoped advisory lock, so concurrent cold boots are safe).
- **OpenTelemetry export** (new `agentgraph-otel` crate) — one-call tracing subscriber setup with optional OTLP span export, completing the v0.3 executor instrumentation story.
- **Studio** (`studio/`, zero-build single-file UI) — connect bar, graph/thread panels, state + checkpoint-history viewers, all three run modes, interrupt/resume, and fork/replay against the real time-travel endpoints. The server now layers permissive CORS in `router()`, so the Studio can call it cross-origin (restrict it in production). See [docs/studio.md](studio.md).

## Explicitly rejected

- **napi-rs / PyO3 bindings** — REJECTED: they'd freeze a trait surface that's still moving and split maintenance across three ecosystems; the HTTP/SSE server is the polyglot interop layer instead.
- **`cdylib` / C ABI** — REJECTED: a C ABI over async tokio graphs leaks runtime-ownership and panic-safety problems across the boundary for near-zero demand; embed the Rust crate directly or talk HTTP.

## Phase D — platform ambitions

Directional, not scheduled:

- **Hosted multi-tenant service** — the server crate operated as a managed platform: tenant isolation, durable queues, autoscaling workers.
- **WASM target** — run graphs themselves in the browser or edge runtimes (sans native checkpointers).
- **Edge deployment** — single-digit-MB agent services on edge runtimes, leaning on Rust's footprint and the static-binary story.

## Design docs & references

- [agentgraph-server design](agentgraph-server-design.md) — endpoint mapping, SSE semantics, phased server roadmap (Phases A/B/C).
- [server quickstart](server-quickstart.md) — zero to a served graph with interrupt/resume over HTTP + SSE.
- [agentgraph README](../agentgraph/README.md#roadmap) — core crate roadmap checklist.
- [agentgraph-server README](../agentgraph-server/README.md) — server endpoint inventory and status.
- [CHANGELOG](../CHANGELOG.md) — release history.
