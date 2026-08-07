# Rusty platform roadmap

Where the platform has been, what's landing this cycle, and what's next. Crates are versioned independently (`rusty-agent-runtime` core, `rusty-server`, `rusty-worker`); phases below group work across the monorepo. Named releases: **R0.1 — Ignition**, **R0.2 — Persistence**, **R0.3 — Interop**, **R0.4 — Time Travel** (all implemented), and **R1.0 — Unleashed** (upcoming). History lives in [../CHANGELOG.md](../CHANGELOG.md); per-crate detail lives in each crate's README.

## Status at a glance

| Release | Phase | Contents | Version target | Status | Date |
|---|---|---|---|---|---|
| **R0.1 — Ignition** | Core kernel | State channels + reducers, Pregel/BSP executor, checkpoints, HITL interrupts, `Send` fan-out, `ChatModel`/`ToolExecutor`, prebuilt ReAct agent | `rusty-agent-runtime` v0.1.0 | ✅ Implemented | 2026-07-31 |
| **R0.2 — Persistence** | Durability & streaming + server Phase A | Postgres checkpointer, token streaming (`messages` mode), live example; axum server: threads, runs, SSE, auth | `rusty-agent-runtime` v0.2.0, `rusty-server` v0.1.0 | ✅ Implemented | 2026-08-05 |
| **R0.3 — Interop** | interop & distribution | MCP client, remote nodes + worker SDK, server API completion, executor tracing | `rusty-agent-runtime` v0.3.0, `rusty-server` v0.2.0, `rusty-worker` v0.1.0 | ✅ Implemented | 2026-08-05 |
| **R0.4 — Time Travel** | production hardening | WASM nodes, time-travel core + server API, Postgres server store, OpenTelemetry export, Studio UI, permissive CORS | `rusty-agent-runtime` v0.4.0, `rusty-server` v0.3.0, `rusty-otel` v0.1.0 | ✅ Implemented | 2026-08-05 |
| v0.5 (pre-1.0) | SDKs & tenancy | Python SDK (stdlib-only), TypeScript SDK (zero-dep ESM), multi-tenant auth with full isolation, live-LLM validation + calculator fix | `rusty-server` v0.4.0, `sdks/*` v0.1.0 | ✅ Implemented | 2026-08-05 |
| **R1.0 — Unleashed** | platform ambitions | Hosted multi-tenant service (tenant isolation implemented in v0.5 — the first brick), WASM target, edge runtimes | TBD | 🚧 Upcoming | — |

## Implemented

### R0.1 — Ignition · core kernel — `rusty-agent-runtime` v0.1.0 (2026-07-31)

The LangGraph execution model rebuilt on tokio: state channels with per-key `Reducer`s over schema-declared, runtime-validated JSON state, graph validation when you call `GraphBuilder::compile()`, the Pregel/BSP super-step executor (*plan → parallel → barrier → merge → route → checkpoint*), versioned thread-scoped checkpoints (in-memory + JSON-file), interrupt/resume HITL, `Route::Send` dynamic fan-out, typed `GraphEvent` streaming, the minimal `ChatModel` trait with an OpenAI-compatible client, parallel `ToolExecutor`, and `react::create_react_agent`. Details: [CHANGELOG 2026-07-31](../CHANGELOG.md).

### R0.2 — Persistence · durability & streaming + server Phase A — `rusty-agent-runtime` v0.2.0, `rusty-server` v0.1.0 (2026-08-05)

Core gained the `sqlx`-backed `PostgresCheckpointer` (`postgres` feature), real token streaming (`ChatModel::chat_stream` → `GraphEvent::Token`, the LangGraph `messages` stream mode), and a live-agent example against any OpenAI-compatible endpoint. The new `rusty-server` crate implemented Phase A of the Agent-Protocol surface: threads, background/blocking/SSE runs, checkpoint history, per-thread run queue, API-key auth — 10 integration tests green. Details: [CHANGELOG 2026-08-05](../CHANGELOG.md), [server design doc](rusty-server-design.md), [server quickstart](server-quickstart.md).

### R0.3 — Interop — `rusty-agent-runtime` v0.3.0, `rusty-server` v0.2.0, `rusty-worker` v0.1.0 (2026-08-05)

Four workstreams landed concurrently this cycle:

- **MCP client** (`rusty-core/src/mcp.rs`) — call any MCP server's tools from `rusty-agent-runtime` `Tool` impls over stdio transport. MCP tool servers plug into `ToolRegistry` / `ToolExecutor` exactly like native tools, so the prebuilt ReAct agent can drive them with no graph changes.
- **Remote nodes + `rusty-worker`** (`rusty-core/src/remote.rs`, new crate) — `RemoteNode` POSTs node execution to worker services over HTTP; the `rusty-worker` SDK serves user handlers; HITL interrupts cross the wire, so a remote node can suspend the run and resume it with a human payload just like a local node.
- **Server API completion** (`rusty-server` v0.2) — fills out the Agent-Protocol surface from the [design doc](rusty-server-design.md): `GET /runs/{id}`, assistants, crons, and the KV store — 20 integration tests green.
- **Executor tracing** — `tracing` instrumentation through the super-step loop (spans per super-step, node, checkpoint), the foundation for the OpenTelemetry export candidate below.

### R0.4 — Time Travel · production hardening — `rusty-agent-runtime` v0.4.0, `rusty-server` v0.3.0, `rusty-otel` v0.1.0 (2026-08-05)

Five workstreams landed concurrently this cycle:

- **WASM nodes** (`rusty-core/src/wasm_node.rs`, feature `wasm`) — `WasmNode` runs sandboxed WebAssembly modules as graph nodes via Wasmtime: untrusted-code isolation behind the same `Node` trait, without a separate worker fleet.
- **Time travel** — core gained `Checkpointer::get_by_id` / `Checkpointer::fork_thread` and `RunConfig::with_checkpoint_id`; the server exposes them as `POST /threads/{id}/fork` (full- or mid-history forks) and `"checkpoint": {"checkpoint_id": …}` replay on all three run endpoints. Fork first, replay on the fork.
- **Postgres server store** (`rusty-server`, feature `postgres`) — `ServerConfig::with_postgres(url)` moves run checkpoints *and* the assistants/crons/KV surface into Postgres (`server_*` tables, auto-migrated on first use; migrations serialize on a transaction-scoped advisory lock, so concurrent cold boots are safe).
- **OpenTelemetry export** (new `rusty-otel` crate) — one-call tracing subscriber setup with optional OTLP span export, completing the v0.3 executor instrumentation story.
- **Studio** (`studio/`, zero-build single-file UI) — connect bar, graph/thread panels, state + checkpoint-history viewers, all three run modes, interrupt/resume, and fork/replay against the real time-travel endpoints. The server now layers permissive CORS in `router()`, so the Studio can call it cross-origin (restrict it in production). See [docs/studio.md](studio.md).

### v0.5 — SDKs & tenancy (pre-1.0) — `rusty-server` v0.4.0, `sdks/*` v0.1.0 (2026-08-05)

- **Python SDK** (`sdks/python/`) — zero-dependency, stdlib-only client (`urllib.request` + `json`): the full thread/run/SSE/time-travel/assistant/cron/KV surface, verified by an e2e suite that boots the real `server_demo` binary. This is the "interop over HTTP" story made concrete — the polyglot path the rejected PyO3/napi-rs bindings were traded for.
- **TypeScript SDK** (`sdks/typescript/`) — zero-dependency ESM client for Node ≥ 18 and browsers (global `fetch`, async-generator `runStream`), with hand-written type declarations and its own live-server e2e suite.
- **Multi-tenant auth** (`rusty-server` v0.4.0) — `ServerConfig::with_tenant_key(tenant, key)` maps API keys to tenants; threads, runs, assistants, crons, and KV namespaces are fully isolated via internal `{tenant}/` id prefixing, cross-tenant access answers 404 (never 403), and open/dev mode stays byte-identical to before. **This is the first brick of the hosted control plane** — see R1.0 — Unleashed.
- **Live-LLM validation + calculator fix** — `examples/live_agent.rs` verified end-to-end against real Ollama models ([transcript](live-demo-transcript.md)); the run exposed (and a follow-up run confirmed the fix for) a calculator arg-parsing defect: quoted numeric args (`"128"`) failed `as_f64()` and silently computed `0 op 0`. The example now coerces numeric strings and alias keys, logs raw args on failure, and carries 5 unit tests.

## Explicitly rejected

- **napi-rs / PyO3 bindings** — REJECTED: they'd freeze a trait surface that's still moving and split maintenance across three ecosystems; the HTTP/SSE server is the polyglot interop layer instead.
- **`cdylib` / C ABI** — REJECTED: a C ABI over async tokio graphs leaks runtime-ownership and panic-safety problems across the boundary for near-zero demand; embed the Rust crate directly or talk HTTP.

## R1.0 — Unleashed — platform ambitions

Directional, not scheduled. The Phase D ambitions below are what R1.0 — Unleashed is made of:

- **Hosted multi-tenant service** — the server crate operated as a managed platform: tenant isolation, durable queues, autoscaling workers. **Partially started:** v0.5 implemented the tenant-isolation brick (per-tenant API keys, namespaced storage, 404-on-cross-tenant semantics) in `rusty-server` v0.4.0; durable queues and autoscaling remain open.
- **WASM target** — run graphs themselves in the browser or edge runtimes (sans native checkpointers).
- **Edge deployment** — single-digit-MB agent services on edge runtimes, leaning on Rust's footprint and the static-binary story.

## Design docs & references

- [rusty-server design](rusty-server-design.md) — endpoint mapping, SSE semantics, phased server roadmap (Phases A/B/C).
- [server quickstart](server-quickstart.md) — zero to a served graph with interrupt/resume over HTTP + SSE.
- [rusty-agent-runtime README](../rusty-core/README.md#roadmap) — core crate roadmap checklist.
- [rusty-server README](../rusty-server/README.md) — server endpoint inventory and status.
- [CHANGELOG](../CHANGELOG.md) — version history.
