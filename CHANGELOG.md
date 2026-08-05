# Changelog

All notable changes to the agentgraph platform. Format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); crates are versioned independently (`agentgraph`, then `agentgraph-server`).

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
