# agentgraph platform

**A LangGraph-style agentic platform in Rust** — a checkpointable, human-in-the-loop agent graph runtime, plus an axum HTTP/SSE server that serves registered graphs from a single static binary. Dual-licensed under MIT OR Apache-2.0.

## Crates

| Crate | Version | What it is |
|---|---|---|
| [`agentgraph`](agentgraph/) | 0.3.0 | The execution core: typed state channels with reducers, Pregel/BSP super-step executor, checkpoints (in-memory / JSON-file / Postgres), interrupts & resume, dynamic fan-out (`Send`), token streaming, `ChatModel` + parallel tool execution, prebuilt ReAct agent, MCP client, remote nodes, executor tracing. No HTTP, no server dependencies. |
| [`agentgraph-server`](agentgraph-server/) | 0.2.0 | The network face: an axum library crate implementing an Agent-Protocol subset — threads, background/blocking/SSE runs, checkpoint history, per-thread run queue, run-status polling, assistants, crons, KV store, API-key auth. You call `agentgraph_server::serve(registry, config)` from your own `main.rs`. |
| [`agentgraph-worker`](agentgraph-worker/) | 0.1.0 | The worker SDK: serves your node handlers over HTTP so `agentgraph`'s `RemoteNode` can execute graph nodes on remote services — HITL interrupts cross the wire. |

**Architecture one-liner:** nodes publish partial updates to versioned state channels; a Pregel/BSP super-step loop (*plan → parallel → barrier → merge → route → checkpoint*) makes shared-state parallelism safe and every step durable — and the server crate is a thin axum shell over that same `Executor`, so HTTP runs get checkpoints, interrupts, and stream replay for free.

## Quickstarts

- **Library (core):** [agentgraph/README.md](agentgraph/README.md#quickstart) — build a graph in ~30 lines, run it under tokio. Runnable demos: `cargo run --example react_agent|parallel_fanout|human_in_loop|live_agent` (see [agentgraph/examples/README.md](agentgraph/examples/README.md)).
- **Server:** [docs/server-quickstart.md](docs/server-quickstart.md) — 10 minutes from `cargo new` to a served graph with an interrupt/resume round trip over HTTP + SSE. Or run the bundled demo: `cd agentgraph-server && cargo run --example server_demo`.
- **Design:** [docs/agentgraph-server-design.md](docs/agentgraph-server-design.md) — endpoint mapping, SSE semantics, phased roadmap.

## Status

- **v0.3.0 (2026-08-05).** Interop & distribution shipped: an MCP client module (`agentgraph/src/mcp.rs`), remote nodes + the new `agentgraph-worker` crate (HITL interrupts cross the wire), `agentgraph-server` v0.2 API completion (`GET /runs/{id}`, assistants, crons, KV store), and `tracing` instrumentation in the executor. Full picture: [docs/roadmap.md](docs/roadmap.md).
- **v0.2.0 / v0.1.0 (2026-08-05).** Core: Postgres checkpointer (`postgres` cargo feature), token streaming (`ChatModel::chat_stream`, `GraphEvent::Token`, the `messages` stream mode), and a live-agent example join the v0.1 execution core. Server: Phase A shipped — full endpoint inventory in [agentgraph-server/README.md](agentgraph-server/README.md#http-api), 10 integration tests green.
- All three crates are under active development; see each crate's roadmap section. History: [CHANGELOG.md](CHANGELOG.md).

## Repository layout

```text
agentgraph/          core crate (library)
agentgraph-server/   axum HTTP/SSE server crate (library + server_demo example)
agentgraph-worker/   worker SDK crate (v0.1.0)
docs/                server design doc + quickstart + platform roadmap
research/            design research notes
whitepaper/, blog/   accompanying writing
```

## License

Dual-licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT), at your option.
