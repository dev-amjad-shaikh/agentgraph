# R3_RepoFacts — Facts Brief for aboutrusty.com

**Source of truth:** `CHANGELOG.md`, `docs/versioning.md`, `docs/stability.md`, `rusty-core/README.md`, `sdks/python/README.md`, `sdks/typescript/README.md` in the Rusty monorepo. Nothing below is invented; all version numbers, dates, and phrasing are taken verbatim or condensed faithfully from those files.

---

## 1. Release History — the story of the project

**Release branding (verbatim from CHANGELOG):** "v0.1 = R0.1 — Ignition, v0.2 = R0.2 — Persistence, v0.3 = R0.3 — Interop, v0.4 = R0.4 — Time Travel; R1.0 — Unleashed is the upcoming v1.0 track."

> **Pull-quote:** *"Release branding: v0.1 = R0.1 — Ignition … R1.0 — Unleashed is the upcoming v1.0 track."*

Crates are versioned independently: `rusty-agent-runtime`, `rusty-server`, `rusty-otel`, `rusty-worker`. The named releases are a branding/history layer only — a named release does not imply a shared version number.

### Timeline table

| Release | Date | Codename | Headline |
|---|---|---|---|
| rusty-agent-runtime 0.1.0 | 2026-07-31 | R0.1 — Ignition | Execution core, checkpointing, HITL interrupts, LLM & tool layer |
| rusty-agent-runtime 0.2.0 + rusty-server 0.1.0 | 2026-08-05 | R0.2 — Persistence | Postgres checkpointer, token streaming, HTTP/SSE server crate |
| v0.3.0 | 2026-08-05 | R0.3 — Interop | MCP client, remote nodes + `rusty-worker`, server API completion, tracing |
| v0.4.0 | 2026-08-05 | R0.4 — Time Travel | WASM nodes, time travel (fork/replay), Postgres server store, `rusty-otel`, Rusty Studio, CORS |
| v0.5.0 | 2026-08-05 | (pre-1.0 cycle) | Python SDK + TypeScript SDK (both v0.1.0), multi-tenant auth, live-LLM validation |
| Unreleased | — | Quality & docs pass | Correctness fixes (deterministic fan-in merge, transactional interrupt resume, UTF-8 SSE decoding, tenant isolation hardening), API-key masking in `Debug`, compile-time edge validation, full rustdoc review |

### Milestone detail (for "story" sections)

- **R0.1 — Ignition (0.1.0, 2026-07-31):** State channels with per-key `Reducer`s (`Overwrite`, `Append`, `DeepMerge`, `AddMessages`); `GraphBuilder` with topology validation at `compile()`; Pregel/BSP super-step executor (*plan → parallel over immutable snapshot → barrier → merge via reducers → route → checkpoint*) with `max_steps` guard; `Checkpointer` trait with `InMemoryCheckpointer` + `JsonFileCheckpointer`; HITL `ctx.interrupt(payload)` / `RunConfig::with_resume(value)`; static edges, conditional routers, `Route::Send` dynamic fan-out, `Command::goto`; typed `GraphEvent` stream over `tokio::mpsc`; minimal `ChatModel` trait, `OpenAiCompatibleClient` (OpenAI / vLLM / Ollama / LM Studio / Azure-compatible), parallel `ToolExecutor`, prebuilt `react::create_react_agent`. First examples: `react_agent`, `parallel_fanout`, `human_in_loop`.
- **R0.2 — Persistence (2026-08-05):** `PostgresCheckpointer` (`sqlx`, tokio + rustls, behind the `postgres` feature); token streaming (`ChatModel::chat_stream` → `TokenChunk`s; `SseDecoder` byte-chunk agnostic); `GraphEvent::Token` executor plumbing (the LangGraph `messages` stream mode); `examples/live_agent.rs` against any OpenAI-compatible endpoint via `RUSTY_BASE_URL` / `RUSTY_API_KEY` / `RUSTY_MODEL`. **rusty-server 0.1.0 (initial release):** axum-based HTTP/SSE library crate — `GraphRegistry`, `ServerConfig`, `serve()` / `router()`; full Phase-A endpoint inventory (threads, state, history, runs background/blocking/SSE, run rollback); `command.resume` HITL; `reject`/`enqueue` multitask strategies; SSE frame ids `{checkpoint_id}:{step}:{seq}` with `Last-Event-ID` dedup; single static API-key auth (`X-Api-Key` header); `examples/server_demo.rs` on `127.0.0.1:8100`.
- **R0.3 — Interop (2026-08-05):** MCP client (call any MCP server's tools from Rusty `Tool` impls over stdio; MCP servers register into `ToolRegistry` like native tools); `RemoteNode` POSTs node execution to worker services over HTTP + new `rusty-worker` crate — HITL interrupts cross the wire; server API completion (`GET /runs/{id}`, assistants, crons, KV store); executor `tracing` instrumentation (foundation for OpenTelemetry).
- **R0.4 — Time Travel (2026-08-05):** `WasmNode` (sandboxed WebAssembly via Wasmtime, `wasm` feature, 6 WAT-driven tests); time travel — `Checkpointer::get_by_id` / `fork_thread`, `RunConfig::with_checkpoint_id`, `POST /threads/{id}/fork`, `"checkpoint": {"checkpoint_id": …}` on all three run endpoints; Postgres server store (`ServerConfig::with_postgres(url)`, auto-migrated `server_assistants` / `server_crons` / `server_kv` tables); **`rusty-otel`** (new crate, v0.1.0 — one-call tracing subscriber + optional OTLP span export, HTTP/protobuf, `opentelemetry` 0.32); **Rusty Studio** (zero-build single-file debug UI); permissive CORS for browser clients. Fixed: concurrent Postgres migration race via transaction-scoped advisory lock.
- **v0.5.0 (2026-08-05):** Python SDK + TypeScript SDK (see §4); multi-tenant auth in rusty-server v0.4.0 — `ServerConfig::with_tenant_key(tenant, key)`, internal `{tenant}/` id prefixing, **cross-tenant access answers `404`, never `403`**, tenancy-aware cron scheduler, open (no-key) mode byte-identical to pre-multi-tenancy, 9 dedicated integration tests; live-LLM validation transcript against Ollama (`qwen2.5:0.5b`, `llama3.2`); fixed live-example calculator arg coercion (numeric strings like `{"a": "128"}` silently computed `0 op 0 = 0` before the fix; post-fix live run: `128 multiply 46 = 5888` ✅).

---

## 2. Versioning scheme & stability guarantees

### Versioning policy (docs/versioning.md)

> **Pull-quote:** *"There is no single 'Rusty version'."* — packages version independently.

- **Pre-1.0 SemVer.** All packages are `0.x`. A **minor** bump (`0.x.0 → 0.x+1.0`) may contain breaking changes (each recorded in the CHANGELOG); a **patch** bump is fixes only — no API or wire-format changes.
- **The remote-execution wire protocol versions separately:** `PROTOCOL_VERSION` (in `rusty-core/src/remote.rs`) is a single `u32`, currently **`1`**. It governs `RemoteNode` ↔ `rusty-worker` (`POST /execute`, `NodeTask` / `TaskResult`). Evolution within v1 is additive-only; workers must reject tasks with an unsupported `protocol_version`; responses are accepted regardless of their version field (newer workers serve older clients). A non-additive change bumps the protocol to 2.
- **Server↔SDK compatibility is not yet versioned by a constant** — no numeric protocol version on the HTTP/SSE API today. Rule: an SDK `0.x.y` is tested against the same-cycle server release; cross-cycle pairing may work where overlap is additive but is unvalidated.
- **MSRV = Rust 1.86** for all four crates, declared once in `[workspace.package]` (`rust-version = "1.86"`) and inherited workspace-wide; enforced in CI per-crate. Pre-1.0, an MSRV bump may land in any minor release.

### Current versions (as of 2026-08-06)

| Package | Registry | Source | Version |
|---|---|---|---|
| `rusty-agent-runtime` | crates.io | `rusty-core/` | 0.4.0 |
| `rusty-server` | crates.io | `rusty-server/` | 0.4.0 |
| `rusty-worker` | crates.io | `rusty-worker/` | 0.1.0 |
| `rusty-otel` | crates.io | `rusty-otel/` | 0.1.0 |
| `@rusty-runtime/client` | npm | `sdks/typescript/` | 0.1.0 |
| `rusty-agent-runtime` (import: `rusty_client`) | PyPI | `sdks/python/` | 0.1.0 |

> **Name-collision note (by design):** the Rust core crate and the Python SDK are both published as `rusty-agent-runtime` (crates.io and PyPI respectively). Different packages, independent version numbers; the Python SDK is imported as `rusty_client`. Registry publishing for both SDKs is still pending.

### Stability contract (docs/stability.md)

> **Pull-quote:** *"This document is a contract, not an aspiration: if something is not listed under 'stable', assume it can change in the next minor release."*

**Stable today — only two surfaces, treated as protocol-level:**

1. **The remote-execution wire protocol (v1)** — additive-only within v1 (rules above).
2. **The checkpoint format, within a minor version line** — a checkpoint written by any `rusty-agent-runtime` `0.x.*` release is readable by every other `0.x.*` in that same minor line, including restore, `get_by_id` replay, and `fork_thread` time-travel forks. Across a minor bump the struct may change (CHANGELOG will say so and ship a migration path where one exists); no cross-minor guarantee in either direction.

**Not stable (may change in any 0.x minor release):** the Rust API surface of all four crates (pin `=0.x.y` if rebuilds must not break); HTTP request/response JSON fields; SSE event families and payload fields (clients must ignore unknown events/fields; `metadata`, `error`, `end` always emitted; default `stream_mode` is `["values", "updates"]`); SDK class/function shapes (`RustyClient` / `RustyError` / `SSEEvent`, `@rusty-runtime/client` exports); Rusty Studio internals; tenant-isolation internals (the `{tenant}/` prefix layout is an implementation detail — but 404-never-403 is intended behavior).

**Deprecation at 0.x:** a CHANGELOG commitment, not a code mechanism; removal lands no sooner than the following minor release where feasible (security/correctness fixes excepted). No `#[deprecated]` lint guarantee — *"The CHANGELOG is the channel."*

**What changes at R1.0 — Unleashed:** full SemVer across crates, HTTP/SSE API, and both SDKs; the HTTP/SSE API becomes a versioned, stable surface (same-cycle pairing rule goes away); checkpoint migrations guaranteed (a 1.x runtime reads any earlier 1.x checkpoint; migration path across the 0.x → 1.0 boundary); MSRV bumps become minor-release-only events; deprecation gains teeth (`#[deprecated]` warnings for ≥ 1 minor release before removal).

> **Pull-quote:** *"R1.0 — Unleashed flips the default from 'may break' to 'must not break' for the public surface."*

---

## 3. rusty-core crate specifics

**Identity:** published on crates.io as `rusty-agent-runtime`, currently **v0.4.0**. Dual-licensed MIT OR Apache-2.0. MSRV 1.86 (workspace-wide).

> **Tagline (verbatim):** *"The durable agent runtime built in Rust — core graph engine. LangGraph's execution model, rebuilt on tokio, with Rust's safety and single-binary deployment."*

> **Model summary (verbatim):** *"Rusty Core models agent workflows as cyclic graphs over shared state. Every state key is a versioned channel with per-key reducer semantics; nodes are async functions returning partial updates; execution follows a Pregel/BSP super-step model with first-class checkpoints, interrupts, streaming events, and dynamic fan-out."*

### Why Rust (pull-quote fodder, from README)

- **No GC pauses, no GIL** — deterministic streaming latency and true parallelism for concurrent tool calls on a single tokio runtime.
- **Validation before execution** — graph topology validated at `compile()`, before any node (or paid LLM call) runs.
- **Single-binary deployment** — one static artifact; small, auditable dependency tree (tokio, serde, reqwest+rustls, thiserror).
- **Memory footprint** — small resident set when colocating thousands of agent threads.
- *"The trade-off is deliberate: you give up Python's runtime monkey-patching and get durable, auditable execution semantics in return."*

### Feature list (each with version introduced)

- **Typed state channels with reducers** — `Overwrite` (LangGraph `LastValue`), `Append`, `DeepMerge`, `AddMessages` (ID-aware message upsert). Writes to undeclared channels rejected.
- **Pregel/BSP super-step executor** — *plan → parallel over immutable snapshot → barrier → merge via reducers → route → checkpoint*; each step transactional.
- **Checkpointing** — `InMemoryCheckpointer`, `JsonFileCheckpointer` (pure `serde_json`), `PostgresCheckpointer` (`sqlx`, `postgres` feature, v0.2). *"One primitive, four use cases: durable execution, human-in-the-loop, time travel, partial-failure recovery."*
- **Human-in-the-loop interrupts** — `ctx.interrupt(payload)` → `ExecutionOutcome::Interrupted`; resume via `RunConfig::with_resume(value)` / `ctx.resume_value()`.
- **Dynamic fan-out (`Send`)** — `Route::Send(vec![Send::new(node, state), ...])` for runtime map-reduce.
- **Streaming events** — typed `GraphEvent`s (`SuperStep`, `NodeStart`, `NodeEnd`, `StateUpdate`, `CheckpointSaved`, `Token`) over `tokio::mpsc`; LangGraph `values`/`updates`/`messages` modes are filters over one stream.
- **Token streaming** (v0.2) — `ChatModel::chat_stream` → `GraphEvent::Token` via `Executor::with_token_tx` / `RunConfig::token_tx`.
- **LLM & tool layer** — minimal `ChatModel` trait; `OpenAiCompatibleClient` (OpenAI, vLLM, Ollama, LM Studio, Azure-compatible); `ToolRegistry` / `ToolExecutor` — parallel, order-stable, error-isolating.
- **`Command` routing** — `NodeOutput::route(Command::goto(...))`.
- **MCP client** (v0.3) — call any MCP server's tools over stdio; register like native tools.
- **Remote nodes** (v0.3) — `RemoteNode` + `rusty-worker`; HITL interrupts cross the wire.
- **Time travel** (v0.4) — `Checkpointer::get_by_id` / `fork_thread`, `RunConfig::with_checkpoint_id`. *"Fork first, replay on the fork."*
- **WASM nodes** (v0.4, feature `wasm`) — `WasmNode` via Wasmtime: capability isolation, same `Node` trait, no worker fleet.

### Design rules worth knowing (verbatim)

- **Nodes never call nodes.** They publish partial updates to channels; routing is decided at the barrier.
- **Cycles are not recursion.** The ReAct loop is nodes re-scheduled across super-steps, guarded by `RunConfig::max_steps` (default 1000, matching LangGraph's `recursion_limit`).
- **Node logic must be idempotent.** Interrupt/resume and failure recovery re-execute a node from its start; checkpointing happens at super-step boundaries, never mid-node.

### Examples (4 runnable, under `rusty-core/examples/`)

| Example | What it shows | Run |
|---|---|---|
| `react_agent.rs` | Prebuilt ReAct loop via `react::create_react_agent` — `agent` node + `tools` node, conditional routing on pending tool calls | `cargo run --example react_agent` |
| `parallel_fanout.rs` | Dynamic map-reduce: `Route::Send` fan-out, parallel workers, fan-in via `Reducer::Append` | `cargo run --example parallel_fanout` |
| `human_in_loop.rs` | Interrupt → durable `JsonFileCheckpointer` checkpoint → resume with human approval payload | `cargo run --example human_in_loop` |
| `live_agent.rs` | Live ReAct agent against a real OpenAI-compatible endpoint (Ollama, OpenAI, vLLM, LM Studio) with token streaming; exits gracefully with setup instructions if no endpoint reachable | `cargo run --example live_agent` |

### Positioning pull-quote

> *"Rusty Core is the orchestration core, not another provider client… The wedge no Rust crate ships today is the LangGraph quartet (state graph + durable checkpointing + HITL interrupts + resumable execution) as first-class, production-grade primitives."*

**vs. LangGraph highlight:** Rusty matches LangGraph on state graphs/reducers, checkpointing, HITL, `Send` fan-out, streaming, prebuilt ReAct, MCP interop, remote nodes, time travel — and uniquely adds **sandboxed WASM nodes** (LangGraph: ❌). Runtime cost: "interpreter + GC" vs "single static binary".

### Dependency snippet (for copy-paste)

```toml
[dependencies]
rusty-agent-runtime = "0.4"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

### Roadmap status (from README)

- Implemented ✅: super-step executor, JSON-file + Postgres checkpointers, prebuilt ReAct agent, token streaming, live agent example, MCP client, remote nodes + `rusty-worker`, executor tracing, time travel, WASM nodes, OpenTelemetry (`rusty-otel` v0.1.0).
- Not yet: **WASM target** (run graphs in browser/edge), **provider adapters** (thin `ChatModel` impls over Rig, `async-openai`, `genai`).
- Rejected ❌: **PyO3 / napi-rs bindings** — "the HTTP/SSE server is the polyglot interop layer."

---

## 4. SDK facts

Both SDKs are zero-dependency, both at **v0.1.0**, both shipped 2026-08-05 (v0.5.0 release), both with registry publishing still pending, both tested by true end-to-end suites that boot the real `server_demo` binary as a subprocess. Both cover the full server API surface: **threads, runs (background / blocking / SSE-streaming), checkpoint history, time travel (fork + replay), assistants, crons, and the cross-thread KV store.**

### Python SDK — `rusty_client`

- **PyPI name:** `rusty-agent-runtime`; **import name:** `rusty_client`. Requires **Python 3.8+**.
- **Zero-dependency, stdlib-only** — `urllib.request` + `json`, nothing to `pip install`, ever. No `requests`, no `httpx`, no `sseclient`; hand-rolled SSE parser, blocking I/O.
- Can be installed (`pip install rusty-agent-runtime`), path-installed, or simply copied (`cp -r sdks/python/rusty_client /your/project/`) — no build step.
- > **Philosophy pull-quote:** *"This SDK is the 'interop over HTTP' story: the Rust server owns orchestration, checkpoints, and streaming; any language that can speak HTTP and parse SSE can drive it."*
- Positioned for "scripts, CI, notebooks, and LangChain-adjacent glue code."
- **Tested:** 18-test e2e suite (17 pass + 1 documented skip — interrupt/resume, because `server_demo` registers no interrupting graph); suite boots the real `server_demo` subprocess.
- **Key classes/errors:** `RustyClient`, `RustyError` (`.status`, `.body`), `SSEEvent` dataclass (`event`, `data`, `id`).
- Auth: `RustyClient(url, api_key="...")` → sent as `X-Api-Key` header on every request.

**Python quickstart snippet (verbatim):**

```python
from rusty_client import RustyClient

client = RustyClient("http://127.0.0.1:8100")   # api_key="..." when auth is on

client.ok()      # True
client.info()    # {"service": "rusty-server", "graphs": [...], ...}

thread = client.create_thread("pipeline")
tid = thread["thread_id"]

# Blocking run
result = client.run_wait(tid)

# Streaming run (SSE) — frames arrive as the graph executes
for frame in client.run_stream(tid, stream_mode=["updates", "values"]):
    print(frame.event, frame.id, frame.data)

# Time travel: fork at an earlier checkpoint, replay on the fork
mid = next(h for h in client.history(tid) if h["next"] == ["second"])
cp_id = mid["checkpoint"]["checkpoint_id"]
fork = client.fork(tid, checkpoint_id=cp_id)
client.run_wait(fork["thread_id"], checkpoint_id=cp_id)

# Human-in-the-loop: resume an interrupted run
client.run_wait(tid, command={"resume": {"approved": True}})
```

**Python API surface (18 method families):** `ok`, `info`, `create_thread`, `get_state`, `update_state`, `history`, `fork`, `run` (202 background), `run_wait` (blocking), `run_stream` (generator of `SSEEvent`), `run_status`, `delete_run` (checkpoint rollback), `create_assistant` / `list_assistants` / `get_assistant`, `create_cron` / `list_crons` / `delete_cron`, `kv_put` / `kv_get` / `kv_delete` / `kv_list`. Streaming: `stream_mode` filters frame families (`updates`, `values`, `messages`); `metadata`/`error`/`end` always emitted; `last_event_id=frame.id` resumes a dropped connection (`Last-Event-ID` header); frame ids are `{checkpoint_id}:{step}:{seq}`.

### TypeScript SDK — `@rusty-runtime/client`

- **npm name:** `@rusty-runtime/client`. **ESM-only** (`"type": "module"`; from CommonJS use `await import(...)`). **Node.js ≥ 18** (`engines` pinned) and **modern browsers** (global `fetch`, `ReadableStream`, `TextDecoder`, `AbortController`).
- **Zero-dependency** with hand-written `.d.ts` declarations; full JSDoc in source.
- SSE consumed via `fetch` + `ReadableStream` (**not** `EventSource`) — that's what makes POST streaming and the `Last-Event-ID` resume header possible.
- Browser-friendly: server ships permissive CORS (`access-control-allow-origin: *`, preflights answered before auth) — works cross-origin out of the box, even from `file://` pages.
- Constructor options: `{ apiKey, timeout (default 30_000 ms per request, 0 disables), fetch (custom fetch for tests/proxies/polyfills), signal }` on every method.
- **Errors:** non-2xx throws `RustyError` (`.status`, `.body`); timeouts throw `RustyTimeoutError` (subclass, `.status === 0`, `.timeoutMs` set). For `runStream` the timeout covers *establishing* the stream, not its lifetime.
- **Tested:** 17-test e2e suite (16 pass + 1 self-skip — the 401 test, because the demo binary runs with auth disabled); spawns the real `server_demo` binary on `127.0.0.1:8100`.

**TypeScript quickstart snippet (verbatim):**

```js
import { RustyClient } from '@rusty-runtime/client';

const client = new RustyClient('http://localhost:8100', {
  // apiKey: '…',   // sent as X-Api-Key when the server configures one
  // timeout: 30_000, // ms per request (default); 0 disables
});

const info = await client.info();
// { service: 'rusty-server', version: '0.4.0', graphs: [{ name: 'pipeline', channels: ['log'] }, …] }

const { thread_id } = await client.createThread('react_agent');
const terminal = await client.runWait(thread_id, {
  input: { messages: [{ role: 'user', content: 'What is 17 + 25?' }] },
});
console.log(terminal.status, terminal.output);

// Stream a run over SSE
for await (const frame of client.runStream(thread_id, { input: { /* … */ } })) {
  // frame: { event: 'metadata'|'updates'|'values'|'messages'|'error'|'end', data, id? }
  if (frame.event === 'end') console.log('done:', frame.data.status);
}
```

**TypeScript API surface (18 method families, camelCase):** `ok`, `info`, `createThread`, `getState`, `updateState`, `history`, `fork`, `run`, `runWait`, `runStream` (async generator), `runStatus`, `deleteRun`, `createAssistant` / `listAssistants` / `getAssistant`, `createCron` / `listCrons` / `deleteCron`, `kvGet` / `kvPut` / `kvDelete` / `kvList`.

**Run payload shape (both SDKs):** `{ input, command: { resume }, config: { recursion_limit }, checkpoint: { checkpoint_id }, metadata, stream_mode, multitask_strategy, assistant_id }`.

**Time-travel snippet (TS, verbatim):**

```js
const history = await client.history(threadId);
const earliest = history.at(-1).checkpoint.checkpoint_id;
const { thread_id: forkId } = await client.fork(threadId, { checkpointId: earliest });
await client.runWait(forkId, { checkpoint: { checkpoint_id: earliest } }); // replay on the fork
```

### SDK ↔ server compatibility

SDK 0.1.x ↔ rusty-server 0.4.x is the tested, supported pairing (same-cycle rule — no numeric HTTP protocol version exists yet). Cross-cycle use may work where the API overlap is additive but is unvalidated.

---

## 5. Memorable numbers & phrases (quick reference for landing copy)

- **4 crates**: `rusty-agent-runtime` (0.4.0), `rusty-server` (0.4.0), `rusty-worker` (0.1.0), `rusty-otel` (0.1.0) — plus **2 SDKs** (Python + TypeScript, both 0.1.0, both zero-dependency).
- **MSRV: Rust 1.86**, workspace-wide, CI-enforced.
- **`PROTOCOL_VERSION = 1`** — additive-only remote-execution wire protocol.
- **Default `max_steps` = 1000** (matches LangGraph's `recursion_limit`).
- **SSE frame ids:** `{checkpoint_id}:{step}:{seq}`; default `stream_mode` = `["values", "updates"]`; `metadata`/`error`/`end` always emitted.
- **Multi-tenancy:** *"Cross-tenant access answers `404`, never `403`."*
- **Demo server:** `127.0.0.1:8100`, graphs `pipeline` + `react_agent`, no network/keys needed.
- **Test counts:** Python SDK 18 e2e tests (17 pass + 1 skip); TypeScript SDK 17 e2e tests (16 pass + 1 skip); multi-tenant auth 9 integration tests; WASM nodes 6 WAT-driven tests; rusty-server 10 integration tests at 0.1.0.
- **Live-model validation:** ReAct runs verified against Ollama `qwen2.5:0.5b` and `llama3.2`; calculator fix verified live: `128 multiply 46 = 5888` ✅.
- **Release codenames:** R0.1 Ignition → R0.2 Persistence → R0.3 Interop → R0.4 Time Travel → R1.0 Unleashed (upcoming).
- **License:** dual MIT OR Apache-2.0 everywhere.
