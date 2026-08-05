# agentgraph-server

**The network face of [`agentgraph`](../agentgraph)** — serve your agent graphs over HTTP + SSE from a single ~20 MB static binary. No interpreter, no Postgres, no Redis. Dual-licensed under MIT OR Apache-2.0.

> **Status: v0.2, under active development.** The crate ships as a *library*: you call `agentgraph_server::serve()` from your own `main.rs`. The endpoint set, streaming semantics, and config surface follow the architecture document in [`docs/agentgraph-server-design.md`](../docs/agentgraph-server-design.md). The core `agentgraph` crate is untouched — it has no HTTP, no axum, no server dependencies, and never learns that a server exists.

## Why one binary instead of three containers

A self-hosted LangGraph Platform standalone deployment needs **three moving parts**: the API container, Postgres (threads / runs / checkpoints / task queue), and Redis (pub/sub fan-out for background-run streaming) — plus a queue-worker topology for exactly-once background runs. `agentgraph-server` collapses that into a single static binary, because the primitives LangGraph rents from infrastructure fall out of `agentgraph`'s execution model for free:

| Concern | LangGraph Platform | agentgraph-server |
|---|---|---|
| User-code loading | `langgraph.json` + pip install at image build | `Cargo.toml` + `main.rs`, static link |
| Deployment unit | API image + Postgres + Redis (compose) | one static binary (~20 MB) |
| Checkpoint store | Postgres | embedded `JsonFileCheckpointer` (wired from `ServerConfig::store_path`; core's `postgres` feature is server-roadmap) |
| Stream fan-out | Redis pub/sub | in-process `tokio::sync::broadcast` per run |
| Background-run queue | Postgres task queue + workers | in-process per-thread run queue |
| Stream resume | `stream_resumable` contract | replay from the per-run in-memory event log, deduped by `Last-Event-ID` |
| Multi-process scale-out | supported | Phase B gRPC worker protocol (see [roadmap](#roadmap)) |

The trade is explicit: this is a **single-process** server. That covers the overwhelming majority of self-hosted agent deployments, and the `Node` trait keeps the multi-process door open — remote gRPC workers and WASM nodes are planned implementations of the same trait, not architectural changes.

## Setup: Cargo.toml is the new langgraph.json

LangGraph's `langgraph.json` exists because Python can import user modules at runtime. Rust cannot — so in Rust, the declaration of "which graphs this server hosts" *is* your `main.rs`, and the dependency list *is* `Cargo.toml`. The server is a crate you call, not a binary you load graphs into.

```toml
[dependencies]
agentgraph = "0.3"
agentgraph-server = "0.2"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
tracing-subscriber = "0.3"
```

A realistic `main.rs` — register a graph under a name, hand the registry to `serve`:

```rust
use std::sync::Arc;
use agentgraph::prelude::*;
use agentgraph_server::{serve, GraphRegistry, ServerConfig};

mod graphs; // your code: build_support_graph(), etc.

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Graph 1: the prebuilt ReAct agent.
    let mut tools = ToolRegistry::new();
    tools.register(Calculator);
    tools.register(Echo);
    let model: Arc<dyn ChatModel> = Arc::new(OpenAiCompatibleClient::from_env(
        "https://api.openai.com/v1",
        "OPENAI_API_KEY",
        "gpt-4o-mini",
    ));
    let react = create_react_agent(model, tools)?;
    let react_spec = StateSpec::new().channel("messages", Reducer::AddMessages);

    // Graph 2: a custom compiled graph.
    let (support, support_spec) = graphs::build_support_graph()?;

    // The registry: the Rust analog of langgraph.json's `graphs` map.
    let mut registry = GraphRegistry::new();
    registry.register("react_agent", react, react_spec);
    registry.register("support_agent", support, support_spec);

    // One call: serve. Blocks on the axum/tokio runtime.
    let config = ServerConfig::new(
        "0.0.0.0:8080".parse()?,       // bind address
        "./data/checkpoints",          // JsonFileCheckpointer root
    );
    serve(registry, config).await?;
    Ok(())
}
```

A `GraphRegistry` entry is a name plus the two things the executor needs — a `Graph` and its `StateSpec` — so `Executor::run(&graph, &spec, state, config)` can be driven for any registered name over HTTP. Registration is **compile-checked**: a graph whose nodes write channels absent from its spec fails in your CI, not in production.

**Dev loop.** No `langgraph dev` equivalent is needed. `cargo watch -x run` (or `bacon run`) recompiles and restarts on save; incremental rebuilds of a single-graph binary take seconds. During development, point `ServerConfig::new`'s `store_path` at a scratch directory you can delete between runs.

**Embedding.** `serve(registry, config)` binds and blocks; if you want the routes inside a larger axum application (or want to drive the API in tests via `tower::ServiceExt::oneshot`), call `agentgraph_server::router(registry, config)` instead and merge the returned `Router` yourself.

## HTTP API

An Agent-Protocol-compatible subset — wire-compatible with the core run/thread shapes LangGraph Platform uses, without the commercial surface. This table is the v0.2 endpoint inventory; everything listed here is implemented and covered by integration tests.

| Endpoint | Description |
|---|---|
| `GET /ok` | Liveness probe → `{"ok": true}` |
| `GET /info` | Service version, checkpointer kind, store path, registered graphs + their channels |
| `POST /threads` | Create a thread bound to a registered graph: `{graph, metadata?, thread_id?}` → `201` |
| `GET /threads/{id}/state` | Latest checkpoint: `{values, next, checkpoint}` |
| `POST /threads/{id}/state` | Write a new checkpoint (the `update_state` analog; optional `as_node`, `next_nodes`) → `201` |
| `POST /threads/{id}/history` | List checkpoints, newest first, with `limit` / `before` |
| `POST /threads/{id}/runs` | Start a **background** run → `202` + `{run_id, thread_id, status}` |
| `POST /threads/{id}/runs/wait` | Run to completion; returns the terminal JSON (`{status, output \|\| interrupt, …}`) |
| `POST /threads/{id}/runs/stream` | Run with [SSE streaming](#streaming-sse) |
| `DELETE /threads/{id}/runs/{run_id}` | Rollback: delete a **finished** run's checkpoints, re-anchoring the thread to the pre-run checkpoint (`409` while the run is active) |
| `GET /runs/{run_id}` | Poll a run: `{run_id, thread_id, graph, attempt, status}`; once terminal the body also carries the run's `output` / `error` / `interrupt` fields |
| `POST /assistants` | Create a named graph alias: `{name, graph, config?, metadata?, assistant_id?}` → `201` (persisted under `{store_path}/assistants/`) |
| `GET /assistants` / `GET /assistants/{id}` | List / fetch assistants |
| `POST /crons` | Schedule recurring runs: `{graph, interval_secs ‖ cron_expr, input?, metadata?, on_run_completed?}` → `201` (persisted under `{store_path}/crons/`) |
| `GET /crons` / `DELETE /crons/{id}` | List crons (with `runs_fired`, `last_run_at`) / delete a cron (`404` when unknown) |
| `PUT /store/{ns}/{key}` | Upsert a JSON value in a namespace → `201` on create, `200` on replace (`created_at` preserved) |
| `GET /store/{ns}/{key}` / `DELETE /store/{ns}/{key}` | Fetch / delete one item (`404` when absent) |
| `GET /store/{ns}` | List a namespace's items, sorted by key (empty array for an unwritten namespace) |

Not in v0.2 (roadmap, see below): thread listing/deletion endpoints, `/metrics`, `/graphs`, the gRPC worker protocol, `WasmNode`, and the Postgres checkpointer wiring. Thread records live in memory — checkpoints are durable on disk, but the thread registry itself is rebuilt empty on restart (re-create a thread with the same `thread_id` to re-attach to its on-disk checkpoints). Assistants, crons, and store items **are** durable: they persist as JSON files under `store_path` and reload on startup.

**Run-create payload** (subset of LangGraph's shape):

```json
{
  "input": { "messages": [ { "role": "user", "content": "What is 17 + 25?" } ] },
  "command": { "resume": { "approved": true } },
  "config": { "recursion_limit": 25 },
  "metadata": {},
  "stream_mode": ["values", "updates"],
  "multitask_strategy": "reject",
  "assistant_id": "optional-assistant-uuid"
}
```

- `assistant_id` runs through a [named assistant](#assistants-crons-and-the-kv-store): the assistant must be bound to the same graph as the thread (`400` on mismatch, `404` when unknown), and its `config.recursion_limit` applies as a default when the payload doesn't set one.
- `command.resume` is the human-in-the-loop channel: it maps directly to `RunConfig::with_resume(value)`. The executor restores the thread's latest checkpoint, re-runs the interrupted node with `NodeContext::resume_value()` returning the payload, and the run continues. An interrupted run is reported as `{"status": "interrupted", "interrupt": <value>, "checkpoint_id": …, "state": …}`.
- `config.recursion_limit` maps to `RunConfig::with_max_steps(n)`.
- `stream_mode` selects which frame families the SSE endpoint emits; default `["values", "updates"]`. `metadata`, `error`, and `end` frames are always emitted. Add `"messages"` for LLM token deltas.
- `multitask_strategy` — one active run per thread: `enqueue` (default) queues onto the per-thread run queue (depth-capped by `ServerConfig::max_concurrent_runs_per_thread`), `reject` returns `409 Conflict`. LangGraph's `rollback` strategy is instead an explicit operation: `DELETE /threads/{id}/runs/{run_id}` on a finished run.

**Auth.** A single static API key checked against the `X-Api-Key` header (the LangSmith managed-deployment convention), set via `ServerConfig::with_api_key("…")`. With no key configured (the default), the server runs in dev mode with auth disabled.

## Assistants, crons, and the KV store

The v0.2 platform surface, all durable as JSON files under `store_path`:

**Assistants** bind a name plus free-form `config` / `metadata` to a registered graph, so clients can create runs by `assistant_id` instead of repeating a graph name and config. Files live at `{store_path}/assistants/{assistant_id}.json` and reload on startup.

**Crons** fire runs on a schedule. `POST /crons` takes exactly one schedule kind: `interval_secs` (fixed interval, ≥ 1 s) or `cron_expr` (5-field `min hour day-of-month month day-of-week`, UTC, minute resolution — parsed with the `cron` crate). A background scheduler (200 ms tick) fires each due cron by creating a **fresh thread** bound to the cron's graph and scheduling a background run with the cron's `input`. Records carry `runs_fired` / `last_run_at` bookkeeping and persist at `{store_path}/crons/{cron_id}.json`. `on_run_completed: "delete"` turns a cron into a one-shot: it removes itself once its first fired run reaches a terminal state.

**Store** is a cross-thread key-value memory: `PUT /store/{namespace}/{key}` writes any JSON value, namespaced items persist at `{store_path}/store/{namespace}/{key}.json`, and listing a namespace returns its items sorted by key. Namespace and key segments are restricted to `[A-Za-z0-9._-]` (1–128 chars) to keep the path mapping unambiguous.

## Streaming (SSE)

The executor emits one typed event stream — `GraphEvent::{SuperStep, NodeStart, NodeEnd, StateUpdate, CheckpointSaved, Token}` — and LangGraph's stream modes are **filters over that single stream**, implemented as such:

| `stream_mode` | SSE frame | Source |
|---|---|---|
| `updates` | `event: updates` — `{"step": n, "updates": {node→update}}` per step | `GraphEvent::StateUpdate` |
| `values` | `event: values` — full state per step | the `Checkpoint.state` persisted at that step's boundary, read back from the checkpoint log |
| `messages` | `event: messages` — `{"node": …, "delta": …}` per LLM token | `GraphEvent::Token` (requires the node to stream via `ChatModel::chat_stream`) |
| `metadata` | first frame: `{run_id, thread_id, graph, attempt, metadata}` | synthesized by the server |
| `error` | `{error, message}` | `Err(AgentGraphError)` from the executor |
| `end` | `{status: success\|\|interrupted\|\|error}` (plus `interrupt` when interrupted) | the run's `ExecutionOutcome` |

Fan-out is in-process: each run owns a `tokio::sync::broadcast` channel fed from the executor's event sink, and every attached SSE client subscribes. No Redis.

### Last-Event-ID resume

Every SSE frame carries `id: {checkpoint_id}:{step}:{seq}`, where `seq` is a per-run monotonically increasing sequence number (1-based; frames emitted before the first checkpoint use `-` as the checkpoint component). A client that reconnects with the `Last-Event-ID` header skips every frame whose sequence number it has already seen — the server replays the run's event-log tail after that point before streaming live frames. The event log is a per-run, in-memory ring buffer (`ServerConfig::event_log_capacity`, default 1000 frames), so replay covers client reconnects within the server's lifetime; durable cross-restart stream resume is roadmap (checkpoints *are* the stream history, so the data to rebuild it is already on disk).

**Proxy guidance.** Behind nginx or another reverse proxy, disable response buffering for SSE routes (`X-Accel-Buffering: no`) and flush per event, or your clients will see nothing until the buffer fills.

## Configuration

Configuration is code, via `ServerConfig` (constructed with `ServerConfig::new(bind_addr, store_path)` plus builder methods, or `ServerConfig::default()`). If you want twelve-factor env-based config in your binary, read the environment in your own `main.rs` and build the `ServerConfig` from it — the crate deliberately does not read process env itself.

| Field / builder | Default | Purpose |
|---|---|---|
| `bind_addr` | `0.0.0.0:8080` | Listen address (used by `serve`) |
| `store_path` | `./data/checkpoints` | `JsonFileCheckpointer` root (`{store_path}/{thread_id}/{checkpoint_id}.json`) |
| `with_api_key(…)` | `None` = dev mode, no auth | Static key required via the `X-Api-Key` header |
| `with_max_concurrent_runs_per_thread(…)` | `1` | Per-thread enqueue queue depth cap (there is always at most one *active* run per thread) |
| `with_event_log_capacity(…)` | `1000` | Per-run SSE replay buffer (frames) |

## Deployment

Build one static binary:

```bash
cargo build --release
# -> target/release/my-agent   (~20 MB, statically linked)
```

Ship it in a scratch image — no interpreter, no pip layer, no system Python:

```dockerfile
FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release

FROM scratch                       # or gcr.io/distroless/static
COPY --from=build /app/target/release/my-agent /my-agent
ENTRYPOINT ["/my-agent"]
```

## curl quickstart

With the server running locally in dev mode (no API key configured):

```bash
# Liveness + what's registered
curl localhost:8080/ok
# {"ok":true}
curl localhost:8080/info
# {"service":"agentgraph-server","version":"0.2.0","checkpointer":"json_file",
#  "store_path":"./data/checkpoints",
#  "graphs":[{"name":"react_agent","channels":["messages"]}]}

# Create a thread bound to a registered graph
curl -X POST localhost:8080/threads \
  -H 'Content-Type: application/json' \
  -d '{"graph": "react_agent"}'
# -> 201 {"thread_id": "3f2b9c…", "graph": "react_agent",
#         "metadata": null, "created_at": "2026-08-05T…Z"}

# Blocking run
curl -X POST localhost:8080/threads/$TID/runs/wait \
  -H 'Content-Type: application/json' \
  -d '{"input": {"messages": [{"role": "user", "content": "What is 17 + 25?"}]}}'

# Streaming run (SSE) — note -N to disable curl buffering
curl -N -X POST localhost:8080/threads/$TID/runs/stream \
  -H 'Content-Type: application/json' \
  -d '{"input": {"messages": [{"role": "user", "content": "Echo hi"}]},
       "stream_mode": ["updates", "values"]}'

# Resume an interrupted run (human-in-the-loop)
curl -X POST localhost:8080/threads/$TID/runs/wait \
  -H 'Content-Type: application/json' \
  -d '{"command": {"resume": {"approved": true}}}'

# Thread state + checkpoint history
curl localhost:8080/threads/$TID/state
curl -X POST localhost:8080/threads/$TID/history \
  -H 'Content-Type: application/json' -d '{"limit": 10}'

# Roll back a finished run's checkpoints
curl -X DELETE localhost:8080/threads/$TID/runs/$RUN_ID

# Poll a background run's status (terminal runs carry output/error)
curl localhost:8080/runs/$RUN_ID

# Create an assistant and run by assistant_id
curl -X POST localhost:8080/assistants \
  -H 'Content-Type: application/json' \
  -d '{"name": "support-bot", "graph": "react_agent",
       "config": {"recursion_limit": 25}}'
curl -X POST localhost:8080/threads/$TID/runs/wait \
  -H 'Content-Type: application/json' \
  -d '{"assistant_id": "'$AID'"}'

# A cron that fires a run every 60 seconds on a fresh thread
curl -X POST localhost:8080/crons \
  -H 'Content-Type: application/json' \
  -d '{"graph": "react_agent", "interval_secs": 60,
       "input": {"messages": [{"role": "user", "content": "hourly summary"}]}}'
curl localhost:8080/crons
curl -X DELETE localhost:8080/crons/$CRON_ID

# Cross-thread KV store
curl -X PUT localhost:8080/store/memories/user-1 \
  -H 'Content-Type: application/json' -d '{"preference": "dark-mode"}'
curl localhost:8080/store/memories
curl -X DELETE localhost:8080/store/memories/user-1
```

With auth configured, add `-H "X-Api-Key: $KEY"` to every call. For a full walkthrough — project scaffolding, a two-node graph, streaming, and a complete interrupt/resume round trip — see **[docs/server-quickstart.md](../docs/server-quickstart.md)**.

## Roadmap

- [x] **Phase A — the server crate (v0.1).** `GraphRegistry`, the thread/run/SSE endpoint set, per-thread run queue with `multitask_strategy` (`enqueue` / `reject`) plus explicit rollback via `DELETE /threads/{id}/runs/{run_id}`, SSE with mode filters (`updates` / `values` / `messages`) + per-run event log + `Last-Event-ID` dedup, static API-key middleware, `JsonFileCheckpointer` wiring from `ServerConfig::store_path`. *Shipped.*
- [x] **Phase C (partial) — platform surface (v0.2).** `GET /runs/{run_id}` status polling, **assistants** (named graph + config aliases, JSON-persisted, `assistant_id` on run-create), **crons** (interval or 5-field cron schedules, durable records, background tokio scheduler firing runs on fresh threads, `on_run_completed: keep|delete`), and the cross-thread **KV store** (`/store/{namespace}/{key}`, JSON-file-backed). *Shipped.*
- [ ] **Phase B — gRPC worker protocol (`agentgraph-proto`).** `RemoteNode`: a gRPC client behind the same `Node` trait, delegating node execution to stateless out-of-process workers that long-poll named node-queues. The server keeps checkpoints, super-step scheduling, interrupts, and stream fan-out. Agent nodes are dominated by LLM latency (hundreds of ms to minutes), so a 1–5 ms gRPC hop is <1% overhead — and since `State` is already a JSON map, the wire boundary is lossless. Crash isolation, polyglot workers (a Python worker can host the LangChain ecosystem while Rust owns orchestration), and independent scaling of tool-heavy nodes follow.
- [ ] **Phase C (remainder).** Durable thread registry, thread listing/deletion endpoints, `/metrics`, `/graphs`, `WasmNode` (sandboxed wasmtime components behind the same trait — the only locality safe for untrusted/community nodes), and wiring the core crate's `postgres` checkpointer feature into `ServerConfig`.

Deliberately skipped: A2A/MCP server endpoints and WebSocket "protocol v2" (SSE + HTTP sidecar is sufficient), and `feedback_keys` (LangSmith-tracing coupling we don't have).

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](../LICENSE-APACHE))
- MIT License ([LICENSE-MIT](../LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual-licensed as above, without any additional terms or conditions.
