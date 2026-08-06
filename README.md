# agentgraph platform

**A LangGraph-style agentic platform in Rust** — a checkpointable, human-in-the-loop agent graph runtime, plus an axum HTTP/SSE server that serves registered graphs from a single static binary. Dual-licensed under MIT OR Apache-2.0.

## Crates

| Crate | Version | What it is |
|---|---|---|
| [`agentgraph`](agentgraph/) | 0.4.0 | The execution core: typed state channels with reducers, Pregel/BSP super-step executor, checkpoints (in-memory / JSON-file / Postgres) with time travel (`get_by_id` / `fork_thread` / replay-from-checkpoint), interrupts & resume, dynamic fan-out (`Send`), token streaming, `ChatModel` + parallel tool execution, prebuilt ReAct agent, MCP client, remote nodes, sandboxed `WasmNode` execution (`wasm` feature), executor tracing. No HTTP, no server dependencies. |
| [`agentgraph-server`](agentgraph-server/) | 0.4.0 | The network face: an axum library crate implementing an Agent-Protocol subset — threads, background/blocking/SSE runs, checkpoint history, fork + checkpoint-replay time travel, per-thread run queue, run-status polling, assistants, crons, KV store, API-key auth with **multi-tenancy** (per-tenant keys, namespaced isolation), permissive CORS for browser clients, optional Postgres persistence (feature `postgres`). You call `agentgraph_server::serve(registry, config)` from your own `main.rs`. |
| [`agentgraph-otel`](agentgraph-otel/) | 0.1.0 | The observability layer: one-call `tracing` subscriber setup for `agentgraph` executors with optional OTLP span export (OpenTelemetry 0.32, HTTP/protobuf). |
| [`agentgraph-worker`](agentgraph-worker/) | 0.1.0 | The worker SDK: serves your node handlers over HTTP so `agentgraph`'s `RemoteNode` can execute graph nodes on remote services — HITL interrupts cross the wire. |

Plus [`studio/`](studio/): a zero-build, single-file debug UI for `agentgraph-server` (vanilla JS, no npm) — connect, create threads, run/wait/stream, inspect state and checkpoint history, fork and replay from any checkpoint. See [docs/studio.md](docs/studio.md).

## Client SDKs

The server is the polyglot interop layer: if a language speaks HTTP + SSE, it can drive `agentgraph` graphs. Two zero-dependency SDKs ship in [`sdks/`](sdks/), each verified by an e2e suite that boots the real server binary:

**Python** ([`sdks/python/`](sdks/python/), stdlib-only, Python 3.8+ — nothing to `pip install`):

```python
from agentgraph_client import AgentGraphClient

client = AgentGraphClient("http://127.0.0.1:8100")   # api_key="..." when auth is on
thread = client.create_thread("react_agent")
result = client.run_wait(thread["thread_id"], input={
    "messages": [{"role": "user", "content": "What is 17 + 25?"}]
})
print(result["status"], result["output"])

for frame in client.run_stream(thread["thread_id"]):   # SSE, as the graph executes
    print(frame.event, frame.data)
```

**TypeScript / JavaScript** ([`sdks/typescript/`](sdks/typescript/), zero-dep ESM, Node ≥ 18 and browsers, `.d.ts` included):

```js
import { AgentGraphClient } from 'agentgraph-client';

const client = new AgentGraphClient('http://localhost:8100');  // { apiKey } when auth is on
const { thread_id } = await client.createThread('react_agent');
const terminal = await client.runWait(thread_id, {
  input: { messages: [{ role: 'user', content: 'What is 17 + 25?' }] },
});

for await (const frame of client.runStream(thread_id, { input: { /* … */ } })) {
  if (frame.event === 'end') console.log('done:', frame.data.status);
}
```

Both cover the full surface: threads, background/blocking/streaming runs, checkpoint history, fork + replay time travel, assistants, crons, and the KV store — and both send `X-Api-Key` transparently, so they work unchanged against multi-tenant servers.

**Architecture one-liner:** nodes publish partial updates to versioned state channels; a Pregel/BSP super-step loop (*plan → parallel → barrier → merge → route → checkpoint*) makes shared-state parallelism safe and every step durable — and the server crate is a thin axum shell over that same `Executor`, so HTTP runs get checkpoints, interrupts, and stream replay for free.

## Quickstarts

- **Library (core):** [agentgraph/README.md](agentgraph/README.md#quickstart) — build a graph in ~30 lines, run it under tokio. Runnable demos: `cargo run --example react_agent|parallel_fanout|human_in_loop|live_agent` (see [agentgraph/examples/README.md](agentgraph/examples/README.md)).
- **Server:** [docs/server-quickstart.md](docs/server-quickstart.md) — 10 minutes from `cargo new` to a served graph with an interrupt/resume round trip over HTTP + SSE. Or run the bundled demo: `cd agentgraph-server && cargo run --example server_demo`.
- **Studio:** [docs/studio.md](docs/studio.md) — open the zero-build debug UI in [`studio/`](studio/) and point it at a running server (fork & checkpoint replay included).
- **Design:** [docs/agentgraph-server-design.md](docs/agentgraph-server-design.md) — endpoint mapping, SSE semantics, phased roadmap.

## Status

Versions below are **platform releases**; crates are versioned independently. The crates table above and [docs/roadmap.md](docs/roadmap.md) carry the per-crate versions; [CHANGELOG.md](CHANGELOG.md) carries the history.

- **v0.5.0 (2026-08-05).** SDKs & tenancy shipped: the zero-dependency Python (stdlib-only) and TypeScript (ESM) client SDKs in [`sdks/`](sdks/) — each with a live-server e2e suite — and multi-tenant auth in `agentgraph-server` v0.4.0 (per-tenant API keys, namespaced isolation of threads/runs/assistants/crons/KV, 404-not-403 cross-tenant semantics, open mode unchanged). Also: live-LLM validation of the ReAct example against real Ollama models ([docs/live-demo-transcript.md](docs/live-demo-transcript.md)) with the calculator arg-coercion fix it exposed.
- **v0.4.0 (2026-08-05).** Production hardening shipped: sandboxed WASM nodes (`agentgraph` feature `wasm`), checkpoint time travel end-to-end (core `get_by_id` / `fork_thread` / `with_checkpoint_id`; server `POST /threads/{id}/fork` + checkpoint replay on all run endpoints), the Postgres-backed server store (`agentgraph-server` feature `postgres`), the new `agentgraph-otel` crate (OTLP export), the zero-build Studio debug UI ([`studio/`](studio/)), and permissive CORS in `router()` for browser clients. Full picture: [docs/roadmap.md](docs/roadmap.md).
- **v0.3.0 (2026-08-05).** Interop & distribution shipped: an MCP client module (`agentgraph/src/mcp.rs`), remote nodes + the new `agentgraph-worker` crate (HITL interrupts cross the wire), `agentgraph-server` v0.2 API completion (`GET /runs/{id}`, assistants, crons, KV store), and `tracing` instrumentation in the executor.
- **v0.2.0 / v0.1.0 (2026-08-05).** Core: Postgres checkpointer (`postgres` cargo feature), token streaming (`ChatModel::chat_stream`, `GraphEvent::Token`, the `messages` stream mode), and a live-agent example join the v0.1 execution core. Server: Phase A shipped — full endpoint inventory in [agentgraph-server/README.md](agentgraph-server/README.md#http-api).
- All four crates are under active development; see each crate's roadmap section. Contributing: [CONTRIBUTING.md](CONTRIBUTING.md) (workspace-wide) and [agentgraph/CONTRIBUTING.md](agentgraph/CONTRIBUTING.md) (core crate).

## Repository layout

```text
agentgraph/          core crate (library)
agentgraph-server/   axum HTTP/SSE server crate (library + server_demo example, v0.4.0)
agentgraph-otel/     OpenTelemetry export crate (library, v0.1.0)
agentgraph-worker/   worker SDK crate (v0.1.0)
sdks/                zero-dep client SDKs: Python (stdlib-only) + TypeScript (ESM)
studio/              zero-build single-file debug UI for agentgraph-server
docs/                server design doc + quickstart + platform roadmap + studio guide + live-LLM transcript
research/            design research notes
whitepaper/, blog/   accompanying writing
```

## License

Dual-licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT), at your option.
