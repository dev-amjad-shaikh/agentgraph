# Review: agentgraph-worker + agentgraph-otel

**Reviewer:** Reviewer_WorkerOtel (staff-level, read-only)
**Scope:** `agentgraph-worker/` (src, examples, tests), `agentgraph-otel/` (src, examples, tests, README, docker stack)
**Cross-referenced:** `agentgraph/src/remote.rs`, `agentgraph/src/node.rs`, `agentgraph/src/graph.rs`, `agentgraph/src/executor.rs`, `agentgraph/Cargo.toml`

## Severity summary

| Critical | Major | Minor | Nit |
|----------|-------|-------|-----|
| 0 | 3 | 8 | 6 |

## Findings

| # | Sev | Category | Location | Finding |
|---|-----|----------|----------|---------|
| 1 | Major | Correctness | `agentgraph-otel/src/lib.rs:140-149` | **Failed second `init` mutates global state and leaks an exporter.** The OTLP path calls `global::set_tracer_provider(provider.clone())` *before* `try_init()`. When `try_init()` fails with `SubscriberAlreadyInstalled`, the doc contract "leaves the existing subscriber untouched" holds for tracing, but the **global OTel tracer provider has already been replaced**, and the freshly built exporter + batch-processor threads are dropped without shutdown (buffered spans lost, threads detached). The error path also pays the cost of building an exporter for a call destined to fail. Fix: probe/check the subscriber first, or set the global provider only after `try_init()` succeeds (the layer doesn't need the global provider — it takes the tracer directly). |
| 2 | Major | Idiom | `agentgraph-otel/src/lib.rs:144-148` | **The registry-level `EnvFilter` also gates OTLP export.** `filter` is applied to the whole `Registry`, so spans below the filter never reach the collector: `RUST_LOG=warn` produces near-empty traces in Jaeger even though the collector is configured. Ecosystem norm is per-layer filtering (filtered `fmt` layer + unfiltered/separately-filtered OTel layer via `Filtered`/`with_filter`). At minimum this coupling must be documented in the README and `OTelConfig::log_filter` rustdoc, which currently describe the filter as a "log filter" only. |
| 3 | Major | Idiom/Correctness | `agentgraph-worker/src/lib.rs:177` + `204` | **`span.enter()` guard held across `.await` — the exact pattern core forbids.** `execute_handler` enters the `execute` span and then awaits `handler.run(ctx)` while the guard is live. Core `executor.rs:372-374` documents the opposite ("Attached via `.instrument()` (never `.enter()`) so no span guard is held across `.await` points"): on a multi-threaded runtime, other tasks polled on the same thread while this handler is suspended inherit the entered span as current → misattributed telemetry. The two crates no longer read as the same author here. Fix: instrument the dispatch future (`async move { ... }.instrument(span)`) or use a tower layer. |
| 4 | Minor | Robustness | `agentgraph-otel/src/lib.rs:86` | **`EnvFilter::new(directive)` panics on invalid input.** A user-supplied `OTelConfig::log_filter` like `"info,,,bogus==="` panics inside `init` instead of returning an `OTelError`. The crate already has a structured error enum; add a `FilterParse` variant and use `EnvFilter::try_new`. |
| 5 | Minor | Dep hygiene | `agentgraph-otel/Cargo.toml:13` | **`tokio = { features = ["full"] }` in `[dependencies]` but the library never uses tokio** (no `tokio::` reference in `src/lib.rs`). Every downstream user pulls `tokio/full` for nothing. Move to `[dev-dependencies]` (the example/tests need it). |
| 6 | Minor | Dep hygiene | `agentgraph-worker/Cargo.toml:16` | **`tracing-subscriber` is in `[dependencies]` but unused by the library** — only `examples/worker_demo.rs` uses it. Move to `[dev-dependencies]`. |
| 7 | Minor | Docs | `agentgraph-otel/README.md:11` | **Stale version claim:** "span taxonomy emitted by `agentgraph` v0.3.0" — core is at **v0.4.0** (`agentgraph/Cargo.toml:3`). The taxonomy itself is still accurate, but the pinned version is wrong. |
| 8 | Minor | Docs | `agentgraph-otel/README.md:20-21` | **Span-table field inaccuracies vs `executor.rs`:** interrupt event is listed with fields "—" but actually carries `node`, `step` (executor.rs:662-665); error events are listed with fields "—" but carry `node`, `step`, `error`, `retryable` (executor.rs:642-647). The table is otherwise correct. |
| 9 | Minor | Correctness (design) | `agentgraph-worker/src/lib.rs:216` + `agentgraph/src/executor.rs:641` | **Error taxonomy does not survive the wire.** The worker flattens every handler error to `error: String`; `RemoteNode` maps it to `AgentGraphError::Node`. The executor classifies `Llm`/`Tool` errors as retryable — a remote LLM node's transient failure arrives as `Node` and becomes a hard failure. This is a protocol-level tradeoff worth either a structured error `kind` (protocol v2) or explicit documentation in `remote.rs` + worker rustdoc. |
| 10 | Minor | Robustness | `agentgraph-worker/src/lib.rs:204` | **No panic isolation for handlers.** A panicking handler kills the connection (axum default), which `RemoteNode` classifies as a *transport* failure → retried with backoff. That silently replays potentially non-idempotent node logic, contradicting `remote.rs`'s own rationale ("no silent client replays"). Recommend `tower-http`'s `CatchPanicLayer` (or `catch_unwind` around `handler.run`) mapping panics to a 200 + `NodeTaskResponse::error`. |
| 11 | Minor | Testing | `agentgraph-worker/tests/remote_e2e.rs` | **Untested HTTP-layer contract paths:** protocol-version mismatch → 400 (`lib.rs:179-188`), unknown handler → 200 + error body (`lib.rs:190-200`), handler error → 200 + error body (`lib.rs:214-217`). Only the happy path and interrupt round-trip are covered e2e. These are the exact branches that keep `RemoteNode`'s retry semantics honest. |
| 12 | Minor | Docs | `agentgraph-worker/` | **No README.** Every sibling crate (`agentgraph`, `agentgraph-otel`, `agentgraph-server`) ships one; the worker crate — which owns a wire protocol — has only rustdoc. |
| 13 | Nit | Dead-ish API | `agentgraph-worker/src/lib.rs:253-260` | `probe_body()` is public but used only by its own unit test; the example and doc curl snippets hardcode the same JSON by hand (`examples/worker_demo.rs:94`, `lib.rs` header). Either use it in docs/examples or make it private/test-only. |
| 14 | Nit | Consistency | `agentgraph-worker/src/lib.rs:197` | Unknown-handler error embeds `registry.names()` in nondeterministic `HashMap` order, while `/ok` sorts (`lib.rs:145`). Sort for deterministic logs/errors. |
| 15 | Nit | Idiom | `agentgraph-worker/examples/worker_demo.rs:41` | Env-filter default `"agentgraph_worker=info,info"` — valid but unconventional; ecosystem norm is global level first: `"info,agentgraph_worker=info"`. |
| 16 | Nit | Docs | `agentgraph-worker/examples/worker_demo.rs:94` + `lib.rs` header | Hardcoded `"protocol_version":1` in curl snippets will drift silently if `PROTOCOL_VERSION` ever bumps; using `probe_body()` (or a comment) would keep one source of truth. |
| 17 | Nit | Docs | `agentgraph-otel/src/lib.rs:106-113` | `OTelGuard::shutdown` is synchronous and blocks the caller for the batch flush (up to the export timeout); the demo calls it from async `main`. Worth one rustdoc line ("blocking; call outside hot async paths"). The `eprintln!` on shutdown error is defensible and already justified by comment. |
| 18 | Nit | Style | `agentgraph-otel/examples/otel_demo.rs:201-209` | `match &endpoint { ... }` immediately followed by `if endpoint.is_some()` duplicates the branch; fold into the `Some` arm. |

## Things verified correct (no action)

- **Registry ergonomics vs core:** `WorkerRegistry::register` (&mut, replace-on-duplicate) + `with` (builder) genuinely mirror `GraphBuilder::add_node` semantics (`graph.rs:219-225` also silently replaces). The rustdoc claim "ergonomics match `GraphBuilder::add_node` exactly" is accurate, including reliance on the same blanket `Node` impl (`node.rs:257-265`).
- **Protocol-version handling is coherent end-to-end:** worker rejects mismatches with 400 + error body (`worker/lib.rs:179-188`); `RemoteNode` treats 4xx as `Fatal` (never retried) and surfaces the body text (`remote.rs:340-346`). Matches `remote.rs:54-56`'s contract.
- **Outcome shaping is correct:** success/interrupt/error → always 200 with exactly-one-payload body, matching `NodeTaskResponse::into_result`'s exactly-one validation (`remote.rs:189-207`). Interrupts and worker errors are never retried client-side; wire behavior confirmed by core tests.
- **OTel once-per-process semantics are tested correctly:** single-test isolation for process-global state (`tests/init.rs`), `try_init` + dedicated error variant, idempotent `shutdown` via `Option::take` + `Drop`. Sound design — the bug is only in the OTLP arm's ordering (finding #1).
- **README infra claims check out:** `docker-compose.yml` + `otel-collector-config.yaml` exist and match the README (ports 4317/4318/16686, debug exporter, batch processor).
- **No AI-slop tells of consequence:** no TODO/FIXME leftovers, no dead code, no marketing adjectives, rustdoc restates signatures only where it adds contract detail. Comments are purposeful throughout — quality is visibly above typical generated code.
- **Test hygiene:** ephemeral ports, hand-rolled HTTP helpers to avoid extra deps, builder/replace/probe-shape unit tests.

## Verdicts

**agentgraph-worker — PASS with required fixes.** Idiomatic, faithful to the core crate's design language, and the wire contract is implemented correctly. Fix before calling it done: the span-guard-across-await (#3), panic isolation (#10), and the dependency placement (#6); add the missing HTTP-contract tests (#11). The error-flattening tradeoff (#9) needs a documented decision.

**agentgraph-otel — REVISE.** API shape (guard, once-per-process, config resolution order) is exactly right and the local stack/docs are unusually complete, but finding #1 (global-state mutation + leak on failed re-init) and finding #2 (log filter silently throttling exported traces) are real behavioral bugs that contradict the crate's own documentation. Both are small, mechanical fixes. Also fix the `tokio` dependency (#5) and the stale v0.3.0 reference (#7).

---

## Final summary

**Severity counts:** Critical 0 · Major 3 · Minor 8 · Nit 6

**Top 5 must-fixes:**
1. **`agentgraph-otel/src/lib.rs:141` — `global::set_tracer_provider()` runs before `try_init()`**; a failed second `init` still replaces the global tracer provider and leaks an unshut-down exporter/batch threads, contradicting the "leaves existing subscriber untouched" contract.
2. **`agentgraph-otel/src/lib.rs:144-148` — the registry-wide `EnvFilter` also gates OTLP export**; `RUST_LOG=warn` sends almost nothing to the collector. Needs per-layer filtering or explicit documentation.
3. **`agentgraph-worker/src/lib.rs:177,204` — `span.enter()` held across `.await`** in `execute_handler`, the exact anti-pattern core `executor.rs` explicitly forbids; causes span misattribution on multi-threaded runtimes. Use `.instrument()`.
4. **`agentgraph-worker/src/lib.rs:204` — no panic isolation**: a panicking handler becomes a retryable transport error client-side, silently replaying non-idempotent node logic — against the protocol's own no-replay rationale. Add `CatchPanicLayer`.
5. **`agentgraph-otel/src/lib.rs:86` — `EnvFilter::new(user_directive)` panics on invalid input** instead of returning `OTelError`; add a `FilterParse` variant and use `try_new`. (Runner-up: `tokio/full` and `tracing-subscriber` are lib dependencies used only by examples — move to dev-dependencies.)
