# Changelog

All notable changes to the agentgraph platform. Format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); crates are versioned independently (`agentgraph`, `agentgraph-server`, `agentgraph-otel`, `agentgraph-worker`).

## [0.4.0] — 2026-08-05

### Added

- **WASM nodes** *(agentgraph, feature `wasm`)* — `WasmNode` (`wasm_node` module) runs sandboxed WebAssembly modules as graph nodes via Wasmtime: untrusted-code isolation behind the same `Node` trait, no separate worker fleet. 6 WAT-driven tests.
- **Time travel** *(agentgraph + agentgraph-server)* — core: `Checkpointer::get_by_id` / `Checkpointer::fork_thread` and `RunConfig::with_checkpoint_id` (replay a run from any checkpoint instead of the latest). Server: `POST /threads/{id}/fork` (`{new_thread_id?, checkpoint_id?}` → `201 {thread_id, checkpoints_copied}`; `404`/`400`/`409` error cases) and `"checkpoint": {"checkpoint_id": …}` on all three run endpoints (`404` for unknown checkpoint ids).
- **Postgres server store** *(agentgraph-server, feature `postgres`)* — `ServerConfig::with_postgres(url)` switches both persistence layers in one call: run checkpoints to core's `PostgresCheckpointer` and the assistants/crons/KV surface to the auto-migrated `server_assistants` / `server_crons` / `server_kv` tables behind a `ServerStore` trait. Covered by 4 live-database integration tests (gated, `--ignored`).
- **`agentgraph-otel`** *(new crate, v0.1.0)* — the OpenTelemetry export layer: one-call tracing subscriber setup with optional OTLP span export (HTTP/protobuf, `opentelemetry` 0.32), building on v0.3's executor `tracing` instrumentation.
- **Studio** *(studio/)* — a zero-build, single-file debug UI for `agentgraph-server`: connect bar, graph/thread panels, state + checkpoint-history viewers, background/wait/SSE runs, interrupt-resume helper, and fork / checkpoint-replay driven by the real time-travel endpoints (with client-side fallback notes for older servers). See [docs/studio.md](docs/studio.md).
- **Permissive CORS** *(agentgraph-server)* — `router()` now layers `tower_http::cors::CorsLayer::permissive()`, so browser clients like the Studio can call the API cross-origin; OPTIONS preflights are answered before the API-key middleware. Production deployments should replace it with a restrictive layer (see the server README).

### Fixed

- **Concurrent Postgres migration race** *(agentgraph + agentgraph-server)* — first-use auto-migrations (`CREATE TABLE IF NOT EXISTS …`) now run inside a transaction holding a transaction-scoped advisory lock, so several processes/tests booting against one fresh database serialize instead of failing with `duplicate key value violates unique constraint "pg_type_typname_nsp_index"`.

## [0.3.0] — 2026-08-05

### Added

- **MCP client** *(agentgraph)* — the `mcp` module calls any MCP server's tools from `agentgraph` `Tool` impls over stdio transport; MCP tool servers register into `ToolRegistry` / `ToolExecutor` exactly like native tools.
- **Remote nodes + `agentgraph-worker`** *(agentgraph / new crate)* — the `remote` module's `RemoteNode` POSTs node execution to worker services over HTTP; the new `agentgraph-worker` crate is the SDK that serves user handlers. HITL interrupts cross the wire, so remote nodes can suspend and resume runs like local nodes.
- **Server API completion** *(agentgraph-server v0.2)* — fills out the Agent-Protocol surface from the [design doc](docs/agentgraph-server-design.md): `GET /runs/{id}`, assistants, crons, and the KV store.
- **Executor tracing instrumentation** *(agentgraph)* — `tracing` spans through the super-step loop (per super-step, node, and checkpoint), laying the foundation for OpenTelemetry export.

## [2026-08-05] — agentgraph 0.2.0, agentgraph-server 0.1.0

### agentgraph 0.2.0

**Added**

- **Postgres checkpointer** — `PostgresCheckpointer` (`checkpoint_postgres` module, exported from the prelude) behind the `postgres` cargo feature, backed by `sqlx` (tokio + rustls). Same `Checkpointer` trait as the in-memory and JSON-file savers: thread-scoped, versioned snapshots with time-travel listing.
- **Token streaming** — `ChatModel::chat_stream` delivers incremental `TokenChunk`s through a callback; `OpenAiCompatibleClient` decodes real SSE deltas from the wire (`SseDecoder`, byte-chunk agnostic, multi-line `data:` per the SSE spec). The default trait impl falls back to a single chunk, so existing `ChatModel` implementors remain source-compatible.
- **`GraphEvent::Token` + executor plumbing** — forward `chat_stream` deltas into the executor's event channel via `Executor::with_token_tx` / `RunConfig::token_tx` to stream LLM tokens as run events (the LangGraph `messages` stream mode).
- **`examples/live_agent.rs`** — a live ReAct agent against any OpenAI-compatible endpoint (Ollama / OpenAI / vLLM / LM Studio, configured via `AGENTGRAPH_BASE_URL` / `AGENTGRAPH_API_KEY` / `AGENTGRAPH_MODEL`), with token streaming; exits 0 with setup instructions when no endpoint is reachable. Plus `examples/README.md`, a guided tour of all four examples.

**Changed**

- Streaming wire handling: stream termination is driven by the `[DONE]` sentinel with end-of-body fallback; `finish_reason` is deliberately not used for termination because the terminal usage chunk follows it with `stream_options.include_usage`.

### agentgraph-server 0.1.0 (initial release)

**Added**

- New crate: the axum-based HTTP/SSE network face of `agentgraph`, shipping as a **library** — `GraphRegistry` (name → `Graph` + `StateSpec`), `ServerConfig`, `serve()` / `router()`.
- **Endpoint inventory (Phase A):** `GET /ok`, `GET /info`, `POST /threads`, `GET`/`POST /threads/{id}/state`, `POST /threads/{id}/history`, `POST /threads/{id}/runs` (202 background), `POST /threads/{id}/runs/wait` (blocking), `POST /threads/{id}/runs/stream` (SSE), `DELETE /threads/{id}/runs/{run_id}` (checkpoint rollback for finished runs).
- **Runs** — `command.resume` (HITL), `config.recursion_limit`, `reject`/`enqueue` multitask strategies (one active run per thread; in-memory per-thread FIFO queue), terminal JSON for success/interrupted/error.
- **SSE streaming** — `metadata`/`updates`/`values`/`messages`/`error`/`end` frames filtered by `stream_mode`, frame ids `{checkpoint_id}:{step}:{seq}`, per-run in-memory event log (capacity-configurable) with `Last-Event-ID` dedup, in-process `tokio::sync::broadcast` fan-out.
- **Auth** — single static API key via `ServerConfig::with_api_key`, checked against the `X-Api-Key` header; dev mode (no auth) when unset.
- `examples/server_demo.rs` — a two-graph demo server (scripted model, no network) on `127.0.0.1:8100`.
- 10 integration tests covering liveness/info, thread creation, state read/write, history, blocking runs, SSE frame order, interrupt/resume round trip, auth, and both multitask strategies.

## [2026-07-31] — agentgraph 0.1.0 (initial release)

**Added**

- **Execution core** — typed state channels with per-key `Reducer`s (`Overwrite`, `Append`, `DeepMerge`, `AddMessages`); `GraphBuilder` with compile-time topology validation; Pregel/BSP super-step executor (*plan → parallel over immutable snapshot → barrier → merge via reducers → route → checkpoint*) with `max_steps` guard.
- **Checkpointing** — `Checkpointer` trait with `InMemoryCheckpointer` and durable `JsonFileCheckpointer` (pure `serde_json`); versioned, thread-scoped snapshots with time-travel listing.
- **Human-in-the-loop** — `ctx.interrupt(payload)` suspends a run into `ExecutionOutcome::Interrupted`; resume with `RunConfig::with_resume(value)` and `ctx.resume_value()`.
- **Routing** — static edges, conditional routers, `Route::Send` dynamic fan-out, and `Command::goto` node-driven control flow.
- **Streaming events** — typed `GraphEvent` stream (`SuperStep`, `NodeStart`, `NodeEnd`, `StateUpdate`, `CheckpointSaved`) over `tokio::mpsc`.
- **LLM & tool layer** — minimal `ChatModel` trait, `OpenAiCompatibleClient` (OpenAI / vLLM / Ollama / LM Studio / Azure-compatible), `ToolRegistry` + parallel, order-stable, error-isolating `ToolExecutor`, and the prebuilt ReAct agent `react::create_react_agent`.
- **Examples** — `react_agent`, `parallel_fanout`, `human_in_loop`.
