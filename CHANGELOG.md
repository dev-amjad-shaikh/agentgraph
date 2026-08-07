# Changelog

All notable changes to the Rusty platform. Format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); crates are versioned independently (`rusty-agent-runtime`, `rusty-server`, `rusty-otel`, `rusty-worker`). Release branding: v0.1 = R0.1 — Ignition, v0.2 = R0.2 — Persistence, v0.3 = R0.3 — Interop, v0.4 = R0.4 — Time Travel, v0.6 = R0.5 — Flight Recorder (v0.5 was the SDK/tenancy pre-1.0 cycle); R1.0 — Unleashed is the upcoming v1.0 track.

## [Unreleased] — Quality & documentation review pass

### Correctness

- Deterministic fan-in merge when several nodes write the same channel in one super-step.
- Interrupt suspension re-schedules the **entire** active set on resume (the super-step is transactional; completed siblings' discarded writes are no longer lost).
- `GraphEvent::StateUpdate` now reports post-reducer merged values instead of raw partial writes.
- Per-thread JSON checkpoint serialization.
- SSE decoding handles multi-byte UTF-8 sequences split across byte-chunk boundaries.
- MCP client enforces frame-size caps.
- WASM node output bounds are validated.
- Tenant isolation hardening across threads/checkpoints, runs, assistants, crons, and KV.
- Cron interval clamping.
- Thread durability across server restarts.
- OpenTelemetry initialization and filter fixes (`rusty-otel`).
- Python/TypeScript SDK stream parsing and error-mapping fixes.

### Security

- API keys are masked in `Debug` output.

### Validation

- Duplicate, reserved, and mixed graph edges are now rejected at `compile()` time.

### Documentation

- Full rustdoc review pass: module inventory in `rusty-core/src/lib.rs` corrected (file saver no longer marked WIP; `PostgresCheckpointer` / MCP / `RemoteNode` / WASM documented), `create_react_agent_streaming` added to the prelude, `RemoteNode` wire-error semantics and `NodeContext::interrupt` run-wide suspension semantics documented.

## [0.6.0] — 2026-08-07 — R0.5 — Flight Recorder

### Added

- **Flight Recorder contracts** *(rusty-agent-runtime v0.5.0, `record` module)* — the canonical, serde-versioned evidence schema every later wave builds on: `RunEvent` (one recorded fact about a run — super-step boundaries, node input/output, model/tool/remote/WASM calls, interrupts, resumes, routing decisions, checkpoint writes — with causal `parent`, monotonic `seq`, latency, token usage, cost, and status), the `Effect` taxonomy (`Pure` / `ReadOnly` / `Idempotent` / `Compensatable` / `NonIdempotent` — a severity ladder declared by the producer, defaulting to `NonIdempotent` for model and tool calls), `DecisionEvent` (family, features, the closed legal-action set, selected action, propensity assigned at decision time, policy version, outcome — the offline-learning contract, frozen now so R0.5 journals are already learnable evidence; the executor does not yet emit decision events), and `CheckpointHeader` (`format_version`, graph version + `Graph::topology_hash`, policy version, logical clock) stamped into every checkpoint. Payloads travel inline up to `INLINE_PAYLOAD_MAX_BYTES` (4 KiB) and content-addressed (`ArtifactRef`, SHA-256) above it, with bytes held in the journal's own artifact map. Golden-file tests under `rusty-core/tests/golden/` pin the serialized shapes — accidental contract drift fails CI.
- **Determinism seams** *(rusty-agent-runtime, `journal` module)* — the executor sources all time and randomness through injectable `Clock` (`System` / `Logical`) and `RngSource` (`System` / `Seeded`, ChaCha8), configured per run via `RunConfig::with_clock` / `with_rng` / `with_journal` (plus `with_policy_version` / `with_graph_version` for checkpoint headers). Defaults are byte-identical to pre-R0.5 behavior; a logical clock + seeded RNG pair makes event timestamps, event ids, run ids, and checkpoint ids reproducible — the precondition for exact replay.
- **Effect journal** *(rusty-agent-runtime, `journal` module)* — every `Executor::run` records into an append-only in-memory `Journal` (auto-created per run, or attach your own via `RunConfig::with_journal`; retrieve via `Executor::journal`). Recording goes through the `EventDraft` builder; events chain a SHA-256 head hash (tamper-evident), and checkpoints stamp it as a `JournalRef`, binding state and evidence together. `JournalSnapshot` is the serde-complete export form; `Journal::from_snapshot` re-verifies the head hash on load. Node code parents its own model/tool calls via the reserved `PARENT_EVENT_KEY` in `NodeConfig::extra`, which crosses the worker wire protocol so remote nodes parent evidence the same way.
- **Exact-replay engine** *(rusty-agent-runtime, `replay` module)* — re-drive a recorded run exactly. `RecordingChatModel` / `RecordingTool` journal calls in the canonical shapes (`model_call_request` / `model_call_response` / `tool_call_request`); `ReplayingChatModel` / `ReplayingTool` answer the same calls from the journal with **zero outbound calls by construction** — the wrapped implementation is never invoked (proven with panic-on-call sentinels). `ReplaySource::serve` matches by sequence + canonical request hash and fails loudly with `RustyError::Replay` on divergence, order violation, or exhaustion. `ExactReplay::{run, verify, run_and_verify}` drive a replay and check the replayed journal reproduces the recorded one event-for-event, artifacts and head hash included. Byte-identical replay requires the recorded run's determinism seams and runs whose super-steps execute one node at a time (parallel steps interleave logical-clock reads by schedule). Exact replay of *resumed* runs is refused: their evidence begins mid-run against checkpointed state the journal does not carry.
- **Branch diff** *(rusty-agent-runtime)* — `BranchDiff::between(base, branch)` compares two journal snapshots logically (identity and timing fields excluded): the first divergent sequence, added/removed events, per-super-step state-channel diffs, and per-branch event/token/cost totals — fork comparison with no explicit fork marker; the evidence carries the cut.
- **Portable replay fixtures** *(rusty-agent-runtime)* — `ReplayFixture::{capture, export, import, replay_in_ci}` bundle a run (graph topology hash, journal snapshot, final checkpoint, determinism metadata) into one self-contained JSON document (`FIXTURE_FORMAT_VERSION` = 1; `import` rejects unsupported versions and tampered or truncated journals at the boundary). `replay_in_ci` goes from checked-in artifact to verified replay in one call.
- **Server journal persistence + evidence endpoints** *(rusty-server v0.5.0)* — run journals flush at every checkpoint boundary and at run completion: JSON-file store at `{store_path}/journals/{run_id}.json` (atomic temp-file + rename, `journals` is a reserved layout name), Postgres store in the auto-migrated `server_journals` table behind `ServerStore::{put_journal, get_journal}`. New endpoints: `GET /runs/{id}/events` (the run's journaled `RunEvent`s as `{run_id, events, complete}`), `GET /runs/{id}/fixture` (the run as a portable `ReplayFixture`; `409` before the first checkpoint boundary — server runs record under the system clock and OS entropy, so downloaded fixtures support `exact_replay` sessions while byte-identical CI replay requires runs recorded with determinism seams), `POST /runs/replay` (exact replay of a recorded run's journal), and `GET /runs/diff` (a `BranchDiff` between two runs' journals).
- **SDK parity** — the Python (`rusty_client`) and TypeScript (`@rusty-runtime/client`) clients gain Flight Recorder methods: `run_events`, `replay_run`, `diff_runs`, `get_fixture`. Client versions are unchanged (0.1.0); the new methods are covered by the e2e suites against the real server.
- **Studio Recorder** *(studio/)* — a Recorder timeline over `GET /runs/{id}/events`: event scrubbing with effect-class badges and payload inspection, causal-path highlighting from any selected event, replay, and branch compare, with a client-side fallback note for older servers that lack the route.
- **Checkpoint-placement headroom experiment** *([docs/benchmarks.md](docs/benchmarks.md))* — the measurement gating R0.10's checkpoint-placement learning: after mandatory checkpoints (the boundary following every super-step containing a `NonIdempotent` effect), does placement freedom remain? **Headroom exists** — `mandatory_only` wrote 10–50× fewer checkpoints than `uniform` at 2–10 % non-idempotent density (1.05 GB → 21.0 MB of checkpoint bytes at 1 MB state, 1000 steps, 2 % density). The payoff matters where durable checkpointing is a material run cost; methodology and full numbers in the benchmarks doc.

### Changed

- **Checkpoint envelope** *(rusty-agent-runtime)* — `Checkpoint` gains the `CheckpointHeader` provenance header and a `JournalRef` to the journal head at the boundary. Both additions are serde-defaulted: checkpoints written before R0.5 deserialize into `CheckpointHeader::default()` (format version 1, `"unversioned"` graph identity, `static-v0` policy) and keep loading unchanged.
- **Executor time/id sourcing** *(rusty-agent-runtime)* — timestamps, measured latencies, run ids, and checkpoint ids now flow through the run's `Clock` / `RngSource`. With no seams configured, behavior is byte-identical to v0.4.
- **Server run loop** *(rusty-server)* — runs now journal by default and flush the snapshot to the configured store at every checkpoint boundary (previously no journal was persisted). No API or wire change; the delta is the journal write itself.
- **`RustyError`** *(rusty-agent-runtime)* — new `Replay(String)` variant for replay divergence, journal-integrity, and fixture-version failures.

### Compatibility

- **Old checkpoints load.** Every checkpoint written by a `0.4.x` runtime deserializes under 0.5.0 with the default header (serde defaults); the within-minor-line checkpoint guarantee in [docs/stability.md](docs/stability.md) is unaffected.
- **Additive error variant.** `RustyError::Replay` is a new enum variant: downstream `match`es with a wildcard arm compile unchanged; exhaustive matchers add one arm — permitted at a 0.x minor bump.
- **Format versions frozen at 1.** `CURRENT_FORMAT_VERSION` (checkpoint header) and `FIXTURE_FORMAT_VERSION` (replay fixture) are both 1; evolution within v1 is additive via serde defaults, so previously written checkpoints and fixtures keep deserializing. `ReplayFixture::import` rejects unsupported versions at the boundary.
- **Additive-only API evolution.** All R0.5 additions are new modules, methods, endpoints, and SDK methods; no v0.4 signature, route, or wire field was removed or changed.

## [0.5.0] — 2026-08-05

### Added

- **Python SDK** *(sdks/python, v0.1.0)* — a zero-dependency, stdlib-only client for `rusty-server` (`urllib.request` + `json`, nothing to `pip install`): threads, runs (background / blocking / SSE-streaming with a hand-rolled SSE parser), checkpoint history, time travel (fork + replay), assistants, crons, and the KV store. Python 3.8+. Covered by an 18-test e2e suite (17 pass + 1 skip) that boots the real `server_demo` binary as a subprocess. The package is named `rusty-agent-runtime` for PyPI and imported as `rusty_client` (registry publishing is still pending — see the roadmap).
- **TypeScript SDK** *(sdks/typescript, v0.1.0)* — a zero-dependency ESM client for Node.js ≥ 18 and modern browsers (global `fetch` / `ReadableStream` / `AbortController`), with hand-written `.d.ts` declarations: the full HTTP + SSE surface including an async-generator `runStream`. The package is named `@rusty-runtime/client` for npm (registry publishing is still pending — see the roadmap). Covered by a 17-test e2e suite (16 pass + 1 skip) against the real server.
- **Multi-tenant auth** *(rusty-server v0.4.0)* — API keys map to tenants via `ServerConfig::with_tenant_key(tenant, key)` (legacy `with_api_key` = the `default` tenant). Threads + checkpoints, runs, assistants, crons, and KV namespaces are fully isolated through internal `{tenant}/` id prefixing (no schema changes; the unprefixed default tenant keeps existing deployments' flat layout). Cross-tenant access answers `404`, never `403`; the cron scheduler is tenancy-aware. Open (no-key) mode is byte-identical to pre-multi-tenancy behavior — both SDK suites pass against it unchanged. 9 dedicated integration tests (`tests/multi_tenant.rs`).
- **Live-LLM validation** — `docs/live-demo-transcript.md` captures real end-to-end ReAct runs of `examples/live_agent.rs` against Ollama (`qwen2.5:0.5b`, `llama3.2`): graph loop, tool dispatch, and event stream all verified against a live model.
- **Blog follow-up** — `we-shipped-the-whole-engine` (published outside this repo): the v0.1→v0.4 platform story.

### Fixed

- **Calculator tool args in the live example** *(rusty-agent-runtime, examples/live_agent.rs)* — Ollama's tool-call emulation delivers numeric arguments quoted (`{"a": "128", "b": "46"}`); `Value::as_f64()` returned `None` and `unwrap_or(0.0)` silently computed `0 op 0 = 0` in every live run. The calculator now coerces numbers **and** numeric strings (`coerce_f64`), tolerates common alias keys (`operation`/`operator`, `lhs`/`rhs`, `x`/`y`, …), and logs the raw args payload when coercion still fails. Audited `llm.rs` streamed tool-call accumulation (per-index `push_str` concat) — correct as designed; the defect was example-side argument parsing. 5 new unit tests (`cargo test --example live_agent`), plus a post-fix live run appended to the transcript (`128 multiply 46 = 5888` ✅).

## [0.4.0] — 2026-08-05 — R0.4 — Time Travel

### Added

- **WASM nodes** *(rusty-agent-runtime, feature `wasm`)* — `WasmNode` (`wasm_node` module) runs sandboxed WebAssembly modules as graph nodes via Wasmtime: untrusted-code isolation behind the same `Node` trait, no separate worker fleet. 6 WAT-driven tests.
- **Time travel** *(rusty-agent-runtime + rusty-server)* — core: `Checkpointer::get_by_id` / `Checkpointer::fork_thread` and `RunConfig::with_checkpoint_id` (replay a run from any checkpoint instead of the latest). Server: `POST /threads/{id}/fork` (`{new_thread_id?, checkpoint_id?}` → `201 {thread_id, checkpoints_copied}`; `404`/`400`/`409` error cases) and `"checkpoint": {"checkpoint_id": …}` on all three run endpoints (`404` for unknown checkpoint ids).
- **Postgres server store** *(rusty-server, feature `postgres`)* — `ServerConfig::with_postgres(url)` switches both persistence layers in one call: run checkpoints to core's `PostgresCheckpointer` and the assistants/crons/KV surface to the auto-migrated `server_assistants` / `server_crons` / `server_kv` tables behind a `ServerStore` trait. Covered by 4 live-database integration tests (gated, `--ignored`).
- **`rusty-otel`** *(new crate, v0.1.0)* — the OpenTelemetry export layer: one-call tracing subscriber setup with optional OTLP span export (HTTP/protobuf, `opentelemetry` 0.32), building on v0.3's executor `tracing` instrumentation.
- **Rusty Studio** *(studio/)* — a zero-build, single-file debug UI for `rusty-server`: connect bar, graph/thread panels, state + checkpoint-history viewers, background/wait/SSE runs, interrupt-resume helper, and fork / checkpoint-replay driven by the real time-travel endpoints (with client-side fallback notes for older servers). See [docs/studio.md](docs/studio.md).
- **Permissive CORS** *(rusty-server)* — `router()` now layers `tower_http::cors::CorsLayer::permissive()`, so browser clients like Rusty Studio can call the API cross-origin; OPTIONS preflights are answered before the API-key middleware. Production deployments should replace it with a restrictive layer (see the server README).

### Fixed

- **Concurrent Postgres migration race** *(rusty-agent-runtime + rusty-server)* — first-use auto-migrations (`CREATE TABLE IF NOT EXISTS …`) now run inside a transaction holding a transaction-scoped advisory lock, so several processes/tests booting against one fresh database serialize instead of failing with `duplicate key value violates unique constraint "pg_type_typname_nsp_index"`.

## [0.3.0] — 2026-08-05 — R0.3 — Interop

### Added

- **MCP client** *(rusty-agent-runtime)* — the `mcp` module calls any MCP server's tools from Rusty Core `Tool` impls over stdio transport; MCP tool servers register into `ToolRegistry` / `ToolExecutor` exactly like native tools.
- **Remote nodes + `rusty-worker`** *(rusty-agent-runtime / new crate)* — the `remote` module's `RemoteNode` POSTs node execution to worker services over HTTP; the new `rusty-worker` crate is the SDK that serves user handlers. HITL interrupts cross the wire, so remote nodes can suspend and resume runs like local nodes.
- **Server API completion** *(rusty-server v0.2)* — fills out the Agent-Protocol surface from the [design doc](docs/rusty-server-design.md): `GET /runs/{id}`, assistants, crons, and the KV store.
- **Executor tracing instrumentation** *(rusty-agent-runtime)* — `tracing` spans through the super-step loop (per super-step, node, and checkpoint), laying the foundation for OpenTelemetry export.

## [2026-08-05] — rusty-agent-runtime 0.2.0, rusty-server 0.1.0 — R0.2 — Persistence

### rusty-agent-runtime 0.2.0

**Added**

- **Postgres checkpointer** — `PostgresCheckpointer` (`checkpoint_postgres` module, exported from the prelude) behind the `postgres` cargo feature, backed by `sqlx` (tokio + rustls). Same `Checkpointer` trait as the in-memory and JSON-file checkpointers: thread-scoped, versioned snapshots with time-travel listing.
- **Token streaming** — `ChatModel::chat_stream` delivers incremental `TokenChunk`s through a callback; `OpenAiCompatibleClient` decodes real SSE deltas from the wire (`SseDecoder`, byte-chunk agnostic, multi-line `data:` per the SSE spec). The default trait impl falls back to a single chunk, so existing `ChatModel` implementors remain source-compatible.
- **`GraphEvent::Token` + executor plumbing** — forward `chat_stream` deltas into the executor's event channel via `Executor::with_token_tx` / `RunConfig::token_tx` to stream LLM tokens as run events (the LangGraph `messages` stream mode).
- **`examples/live_agent.rs`** — a live ReAct agent against any OpenAI-compatible endpoint (Ollama / OpenAI / vLLM / LM Studio, configured via `RUSTY_BASE_URL` / `RUSTY_API_KEY` / `RUSTY_MODEL`), with token streaming; exits 0 with setup instructions when no endpoint is reachable. Plus `examples/README.md`, a guided tour of all four examples.

**Changed**

- Streaming wire handling: stream termination is driven by the `[DONE]` sentinel with end-of-body fallback; `finish_reason` is deliberately not used for termination because the terminal usage chunk follows it with `stream_options.include_usage`.

### rusty-server 0.1.0 (initial release)

**Added**

- New crate: the axum-based HTTP/SSE network face of Rusty Core, shipping as a **library** — `GraphRegistry` (name → `Graph` + `StateSpec`), `ServerConfig`, `serve()` / `router()`.
- **Endpoint inventory (Phase A):** `GET /ok`, `GET /info`, `POST /threads`, `GET`/`POST /threads/{id}/state`, `POST /threads/{id}/history`, `POST /threads/{id}/runs` (202 background), `POST /threads/{id}/runs/wait` (blocking), `POST /threads/{id}/runs/stream` (SSE), `DELETE /threads/{id}/runs/{run_id}` (checkpoint rollback for finished runs).
- **Runs** — `command.resume` (HITL), `config.recursion_limit`, `reject`/`enqueue` multitask strategies (one active run per thread; in-memory per-thread FIFO queue), terminal JSON for success/interrupted/error.
- **SSE streaming** — `metadata`/`updates`/`values`/`messages`/`error`/`end` frames filtered by `stream_mode`, frame ids `{checkpoint_id}:{step}:{seq}`, per-run in-memory event log (capacity-configurable) with `Last-Event-ID` dedup, in-process `tokio::sync::broadcast` fan-out.
- **Auth** — single static API key via `ServerConfig::with_api_key`, checked against the `X-Api-Key` header; dev mode (no auth) when unset.
- `examples/server_demo.rs` — a two-graph demo server (scripted model, no network) on `127.0.0.1:8100`.
- 10 integration tests covering liveness/info, thread creation, state read/write, history, blocking runs, SSE frame order, interrupt/resume round trip, auth, and both multitask strategies.

## [2026-07-31] — rusty-agent-runtime 0.1.0 (initial release) — R0.1 — Ignition

**Added**

- **Execution core** — state channels with per-key `Reducer`s (`Overwrite`, `Append`, `DeepMerge`, `AddMessages`) over schema-declared, runtime-validated JSON state; `GraphBuilder` with topology validation when you call `compile()`; Pregel/BSP super-step executor (*plan → parallel over immutable snapshot → barrier → merge via reducers → route → checkpoint*) with `max_steps` guard.
- **Checkpointing** — `Checkpointer` trait with `InMemoryCheckpointer` and durable `JsonFileCheckpointer` (pure `serde_json`); versioned, thread-scoped snapshots with time-travel listing.
- **Human-in-the-loop** — `ctx.interrupt(payload)` suspends a run into `ExecutionOutcome::Interrupted`; resume with `RunConfig::with_resume(value)` and `ctx.resume_value()`.
- **Routing** — static edges, conditional routers, `Route::Send` dynamic fan-out, and `Command::goto` node-driven control flow.
- **Streaming events** — typed `GraphEvent` stream (`SuperStep`, `NodeStart`, `NodeEnd`, `StateUpdate`, `CheckpointSaved`) over `tokio::mpsc`.
- **LLM & tool layer** — minimal `ChatModel` trait, `OpenAiCompatibleClient` (OpenAI / vLLM / Ollama / LM Studio / Azure-compatible), `ToolRegistry` + parallel, order-stable, error-isolating `ToolExecutor`, and the prebuilt ReAct agent `react::create_react_agent`.
- **Examples** — `react_agent`, `parallel_fanout`, `human_in_loop`.
