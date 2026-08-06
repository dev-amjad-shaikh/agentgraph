# agentgraph platform

**A LangGraph-style agentic platform in Rust** — a checkpointable, human-in-the-loop agent graph runtime, plus an axum HTTP/SSE server that serves registered graphs from a single static binary. Dual-licensed under MIT OR Apache-2.0.

## Crates

| Crate | Version | What it is |
|---|---|---|
| [`agentgraph`](agentgraph/) | 0.4.0 | The execution core: typed state channels with reducers, Pregel/BSP super-step executor, checkpoints (in-memory / JSON-file / Postgres) with time travel (`get_by_id` / `fork_thread` / replay-from-checkpoint), interrupts & resume, dynamic fan-out (`Send`), token streaming, `ChatModel` + parallel tool execution, prebuilt ReAct agent, MCP client, remote nodes, sandboxed `WasmNode` execution (`wasm` feature), executor tracing. No HTTP, no server dependencies. |
| [`agentgraph-server`](agentgraph-server/) | 0.3.0 | The network face: an axum library crate implementing an Agent-Protocol subset — threads, background/blocking/SSE runs, checkpoint history, fork + checkpoint-replay time travel, per-thread run queue, run-status polling, assistants, crons, KV store, API-key auth, permissive CORS for browser clients, optional Postgres persistence (feature `postgres`). You call `agentgraph_server::serve(registry, config)` from your own `main.rs`. |
| [`agentgraph-otel`](agentgraph-otel/) | 0.1.0 | The observability layer: one-call `tracing` subscriber setup for `agentgraph` executors with optional OTLP span export (OpenTelemetry 0.32, HTTP/protobuf). |
| [`agentgraph-worker`](agentgraph-worker/) | 0.1.0 | The worker SDK: serves your node handlers over HTTP so `agentgraph`'s `RemoteNode` can execute graph nodes on remote services — HITL interrupts cross the wire. |

Plus [`studio/`](studio/): a zero-build, single-file debug UI for `agentgraph-server` (vanilla JS, no npm) — connect, create threads, run/wait/stream, inspect state and checkpoint history, fork and replay from any checkpoint. See [docs/studio.md](docs/studio.md).

**Architecture one-liner:** nodes publish partial updates to versioned state channels; a Pregel/BSP super-step loop (*plan → parallel → barrier → merge → route → checkpoint*) makes shared-state parallelism safe and every step durable — and the server crate is a thin axum shell over that same `Executor`, so HTTP runs get checkpoints, interrupts, and stream replay for free.

## Quickstarts

- **Library (core):** [agentgraph/README.md](agentgraph/README.md#quickstart) — build a graph in ~30 lines, run it under tokio. Runnable demos: `cargo run --example react_agent|parallel_fanout|human_in_loop|live_agent` (see [agentgraph/examples/README.md](agentgraph/examples/README.md)).
- **Server:** [docs/server-quickstart.md](docs/server-quickstart.md) — 10 minutes from `cargo new` to a served graph with an interrupt/resume round trip over HTTP + SSE. Or run the bundled demo: `cd agentgraph-server && cargo run --example server_demo`.
- **Studio:** [docs/studio.md](docs/studio.md) — open the zero-build debug UI in [`studio/`](studio/) and point it at a running server (fork & checkpoint replay included).
- **Design:** [docs/agentgraph-server-design.md](docs/agentgraph-server-design.md) — endpoint mapping, SSE semantics, phased roadmap.

## Status

- **v0.4.0 (2026-08-05).** Production hardening shipped: sandboxed WASM nodes (`agentgraph` feature `wasm`), checkpoint time travel end-to-end (core `get_by_id` / `fork_thread` / `with_checkpoint_id`; server `POST /threads/{id}/fork` + checkpoint replay on all run endpoints), the Postgres-backed server store (`agentgraph-server` feature `postgres`), the new `agentgraph-otel` crate (OTLP export), the zero-build Studio debug UI ([`studio/`](studio/)), and permissive CORS in `router()` for browser clients. Full picture: [docs/roadmap.md](docs/roadmap.md).
- **v0.3.0 (2026-08-05).** Interop & distribution shipped: an MCP client module (`agentgraph/src/mcp.rs`), remote nodes + the new `agentgraph-worker` crate (HITL interrupts cross the wire), `agentgraph-server` v0.2 API completion (`GET /runs/{id}`, assistants, crons, KV store), and `tracing` instrumentation in the executor.
- **v0.2.0 / v0.1.0 (2026-08-05).** Core: Postgres checkpointer (`postgres` cargo feature), token streaming (`ChatModel::chat_stream`, `GraphEvent::Token`, the `messages` stream mode), and a live-agent example join the v0.1 execution core. Server: Phase A shipped — full endpoint inventory in [agentgraph-server/README.md](agentgraph-server/README.md#http-api).
- All four crates are under active development; see each crate's roadmap section. History: [CHANGELOG.md](CHANGELOG.md).

## Repository layout

```text
agentgraph/          core crate (library)
agentgraph-server/   axum HTTP/SSE server crate (library + server_demo example)
agentgraph-otel/     OpenTelemetry export crate (library, v0.1.0)
agentgraph-worker/   worker SDK crate (v0.1.0)
studio/              zero-build single-file debug UI for agentgraph-server
docs/                server design doc + quickstart + platform roadmap + studio guide
research/            design research notes
whitepaper/, blog/   accompanying writing
```

## License

Dual-licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT), at your option.
