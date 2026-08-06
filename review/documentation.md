# Review: Documentation & Prose

**Reviewer:** Reviewer_Documentation (technical editor + Rust dev, READ-ONLY review)
**Scope:** `README.md`, `CHANGELOG.md`, `agentgraph/README.md`, `agentgraph/CONTRIBUTING.md`, `agentgraph/examples/README.md`, `agentgraph-server/README.md`, `agentgraph-otel/README.md`, `agentgraph-worker/` (no README — see finding #2), `sdks/python/README.md`, `sdks/typescript/README.md`, `docs/roadmap.md`, `docs/agentgraph-server-design.md`, `docs/server-quickstart.md`, `docs/studio.md`, `docs/live-demo-transcript.md`
**Verdict (overall):** The prose is unusually strong for an AI-assisted repo — zero hype adjectives, one consistent engineering voice, and 27 of 30 spot-checked code samples/commands/versions/response shapes verified exactly against source. The real problems are **staleness at the edges** (a CONTRIBUTING frozen at scaffold time, version strings in sample outputs), **one undocumented crate** (`agentgraph-worker`), and **a missing time-travel walkthrough in the flagship quickstart**.

## Method

- Read all 15 prose documents in scope, end to end.
- Spot-checked 30 verifiable claims against source: all 4 `Cargo.toml` versions + key dependency pins (`opentelemetry 0.32`, `wasmtime 47`, `cron 0.12`); core quickstart APIs (`StateSpec`/`Reducer` variants, `GraphBuilder`, `NodeOutput::update`, `Route`, `Executor::new`/`with_checkpointer`/`with_token_tx`, `RunConfig::new`/`with_resume`/`with_max_steps`/`with_checkpoint_id`/`with_event_tx`, `ExecutionOutcome::{Done, Interrupted}`, `InMemoryCheckpointer`/`JsonFileCheckpointer`, `Checkpointer::get_by_id`/`fork_thread`, `react::create_react_agent` signature, `OpenAiCompatibleClient::from_env(base_url, key_env, model)`, `ChatModel::chat_stream`); server APIs (`ServerConfig::new(bind, store)` + all 5 builders + `Default`, `/info` response shape, fork `201 {thread_id, checkpoints_copied}`, run-payload fields incl. `checkpoint`/`assistant_id`/`multitask_strategy`, cron `interval_secs ‖ cron_expr`/`on_run_completed`/200 ms tick, KV 1..=128-char segments, tenant ids 1..=64, `server_*` table names, `X-Api-Key` header, `event_log_capacity` default 1000); otel API (`init`/`init_local`/`OTelConfig`/`OTelGuard::shutdown(&mut self)`/`DEFAULT_FILTER`/`OTelError` variants, executor span names `agentgraph.run`/`super_step`/`node`); SDK APIs (all 22 Python methods, all 20 TS methods, `last_event_id`/`lastEventId` resume, `requires-python >=3.8`, `engines >=18`); test-count claims (Python 18 = 17+1 skip ✓, TS 17 = 16+1 skip ✓, multi_tenant 9 ✓, live_agent 5 unit tests ✓); example/demo facts (server_demo port 8100 + graphs `pipeline`/`react_agent` ✓, live_agent env-var defaults ✓, otel_demo + docker-compose ✓, studio files ✓, LICENSE files ✓).
- Result: 27 pass, 3 fail/stale (findings #4, #5, #7).

## Findings

Severity: **H**igh / **M**edium / **L**ow. Categories: AI-tell, Accuracy, Consistency, Gap.

| # | Location | Severity | Category | Gist | Fix |
|---|----------|----------|----------|------|-----|
| 1 | `agentgraph/CONTRIBUTING.md:54-55, 61-62` | H | Accuracy | Module map is frozen at scaffold time: `executor.rs` is labeled "**Current status: `todo!()` — the top good-first-issue**" and `JsonFileCheckpointer` "declared but `todo!()`" — both shipped (executor since v0.1, JSON checkpointer since v0.1, Postgres since v0.2). "Good first issues" #1 and #2 invite contributors to implement already-shipped, tested code. Anyone acting on this doc wastes a PR. | Rewrite the module map from current `src/`; replace the stale good-first-issues with real ones (the README already suggests provider adapters + GenAI span attributes). |
| 2 | `agentgraph-worker/` (no README) | H | Gap | The only crate without a README. Its public API — `WorkerRegistry` (`new`/`register`/`with`), `router(registry)`, `POST /execute`, `NodeTask`, `probe_body()` — is documented nowhere in prose; the root README gives it one table row. Users must read `src/lib.rs` doc comments to use the crate at all. | Add `agentgraph-worker/README.md`: purpose, a minimal worker `main.rs` sample, the wire contract (`POST /execute` request/response shapes), how `RemoteNode` targets it, interrupt-across-the-wire semantics, and a pointer to the e2e test. |
| 3 | `docs/server-quickstart.md:236-261` | M | Gap | The flagship 10-minute tutorial ends at §7 "Time travel: checkpoint history" — which covers only `history` and rollback (`DELETE /runs/{id}`). The v0.4 headline features `POST /threads/{id}/fork` and `"checkpoint": {"checkpoint_id": …}` replay never appear, and "Where to go next" doesn't mention them either. A reader finishing the quickstart doesn't know fork/replay exists. | Add §8 "Time travel: fork & replay" mirroring the server README's fork-first-replay-on-the-fork pattern with curl. |
| 4 | `sdks/typescript/README.md:22` | M | Accuracy (stale sample) | `info()` sample output shows `version: '0.3.0'`; the server is 0.4.0 (`/info` returns `CARGO_PKG_VERSION`). Python SDK README and server README show no version or the correct one. | Update to `'0.4.0'` or elide the field. |
| 5 | `docs/server-quickstart.md:131-133, 180` | M | Accuracy (stale sample) | Sample `/info` output shows `"version":"0.1.0"`; §5 text says "v0.1 keeps thread records in memory". The behavior is still true at 0.4.0, but the version stamps read as unmaintained. | `"version":"0.4.0"`; rephrase to "thread records are in memory (checkpoints durable on disk)" without a version label. |
| 6 | `agentgraph-server/README.md:3,14,288`; `docs/server-quickstart.md:260`; `docs/agentgraph-server-design.md:134,146` | M | AI-tell (evidence-free claim) | "~20 MB static binary" is asserted at least five times across three documents; no measurement, build log, or `ls -la` anywhere in the repo substantiates it. Similarly `agentgraph/README.md:16`: "Rust services routinely run an order of magnitude leaner than their Python counterparts" — unsourced, and "routinely" makes it a quantitative claim. | Either measure (`cargo build --release` on server_demo and print the size in the README) or soften: "a single static binary (no interpreter/runtime)". Drop or source the order-of-magnitude claim. |
| 7 | `docs/agentgraph-server-design.md:87-90` | M | Accuracy | Sample `main.rs` calls `OpenAiCompatibleClient::from_env("OPENAI_API_KEY")?` — wrong signature. The real API is `from_env(base_url, api_key_env, model) -> Self` (3 args, not `Result`; verified `agentgraph/src/llm.rs:334-338`). The doc's own "Deviations" status section covers `ServerConfig::from_env` but not this. Server README (line 53-57) has it right. | Fix the snippet to the 3-arg form and drop the `?`, or mark the whole §2 snippet as aspirational design-draft code. |
| 8 | repo root (no `CONTRIBUTING.md`); `agentgraph/README.md:265` | M | Gap | Only `agentgraph/CONTRIBUTING.md` exists, and it covers the core crate exclusively (module map is core-only). Nothing guides contributors to `agentgraph-server`, `agentgraph-otel`, `agentgraph-worker`, or the SDKs; the root README never links any CONTRIBUTING at all. Compounds finding #1. | Add a root `CONTRIBUTING.md` (workspace-wide checks, per-crate pointers) and link it from the root README. |
| 9 | `README.md:63-68`; `CHANGELOG.md` (whole); `docs/roadmap.md:7-14` | M | Consistency | Two version systems run in parallel: platform releases (v0.5.0) vs independent crate versions (`agentgraph` 0.4.0, server 0.4.0 = platform v0.5). The root README "Status" section uses platform numbers without ever saying so — a reader can hunt for `agentgraph = "0.5"`, which does not exist. CHANGELOG declares "crates are versioned independently" yet has no entries labeled `agentgraph-server` 0.2.0/0.3.0 (folded into platform [0.3.0]/[0.4.0]). Only `docs/roadmap.md`'s table makes the mapping explicit. | One line under the root README Status heading: "Versions below are platform releases; per-crate versions are in the crates table and CHANGELOG." Optionally split CHANGELOG headers per crate. |
| 10 | `agentgraph/src/lib.rs:23-24` | L | Accuracy (stale rustdoc) | Crate-level doc says persistence "ships with an in-memory saver and a **(WIP)** pure-`serde_json` file saver" — the file saver shipped in v0.1 and Postgres in v0.2. (Rustdoc rather than prose, but it is the first thing `cargo doc` readers see; CONTRIBUTING says `lib.rs` is scaffold-owned, so nobody fixed it.) | Drop "(WIP)" and mention `PostgresCheckpointer`. |
| 11 | `agentgraph-otel/README.md:88` | L | Accuracy (formatting) | API-table row for `init` has broken inline code: the description cell opens a code span at `` `Registry + EnvFilter + fmt (+ OTLP layer when `` and closes it at `` `otlp_endpoint` ``, leaving `is set)` dangling unformatted with a stray backtick. Renders incorrectly on GitHub/crates.io. | Restructure as two code spans: `` `Registry + EnvFilter + fmt` `` plus prose "(OTLP layer when `otlp_endpoint` is set)". |
| 12 | `agentgraph/README.md:203`; `CHANGELOG.md:49` | L | Consistency | Terminology wobble: "checkpointer" everywhere vs "savers" in two spots ("the in-memory and JSON-file savers", comparison table "Postgres (`postgres` feature) savers"). Deliberate LangGraph analogy, but the project's own noun is `Checkpointer`. Elsewhere vocabulary is impressively uniform ("super-step" always hyphenated in project docs; only `research/` uses "superstep"). | Prefer "checkpoints"/"implementations" or gloss once: "savers (LangGraph's term for checkpointers)". |
| 13 | `sdks/python/pyproject.toml:27-28` | L | Accuracy | `Homepage`/`Documentation` URLs point to `https://github.com/agentgraph/agentgraph`, an unverified placeholder — `agentgraph/Cargo.toml` has `repository = ""` (empty). If the org/repo doesn't exist, both links 404 from PyPI. | Point at the real repo URL or remove the `[project.urls]` table. |
| 14 | `agentgraph/README.md:36-143` | L | Gap | Core quickstart shows a two-node graph + HITL resume but no time-travel snippet (`fork_thread` / `with_checkpoint_id`) — a headline v0.4 feature reachable only via the Features bullet and the comparison table. The sibling server README demonstrates the HTTP path well. | Add a 10-line "Time travel" snippet after the HITL example (fork, then `RunConfig::with_checkpoint_id` on the fork). |
| 15 | `docs/studio.md:20-21, 74, 121, 128` | L | Consistency (stale labels) | Studio doc repeatedly says "the v0.3 API has **no list-threads endpoint**" / "Since `agentgraph-server` v0.3 sends permissive CORS" — historically accurate (CORS shipped in server v0.3) but the server is now v0.4.0 and the statements are about the *current* API. | "The server API (as of v0.4) has no list-threads endpoint"; keep the v0.3 attribution only where it is a changelog note. |
| 16 | all reviewed docs | L | AI-tell (style) | The single visible AI-generation tell is **em-dash density**: roughly 90 em-dashes in `agentgraph-server/README.md` alone, ~50 in the root README, heavy use everywhere, plus recurring aphoristic constructions ("The trade-off is deliberate", "One primitive, four use cases", "Cargo.toml is the new langgraph.json", "byte-identical", "fork first, replay on the fork"). No hype adjectives anywhere ("robust/seamless/blazing/enterprise-grade" count: **0** in scope), no "not just X but Y" constructions, and bullet lists vary their sentence shapes — so it still reads as one disciplined author rather than generated slurry. | Optional: one editorial pass converting a third of em-dashes to colons/parentheses/periods. Keep the voice; it is currently the repo's strongest asset. |

**Severity counts:** High 2 · Medium 7 · Low 7 (16 findings)

## Per-document verdicts

### `README.md` (root) — **Approve with minor changes**
Accurate and well-organized. All four crate versions match their `Cargo.toml`s; both SDK snippets match the real client APIs (verified method-by-method); all internal links resolve; the `opentelemetry 0.32` parenthetical matches `agentgraph-otel/Cargo.toml`. Fix: the platform-vs-crate version ambiguity (#9) and one claim audit (#6 is mostly in the server README but the "thin axum shell … for free" architecture one-liner is assertive in the same register).

### `CHANGELOG.md` — **Approve with minor changes**
Every factual claim spot-checked held up: Python 18 tests (17+1 skip) ✓, TS 17 (16+1) ✓, 9 multi-tenant tests ✓, fork response shape ✓, `server_*` tables ✓, 5 live_agent unit tests + `coerce_f64` ✓, advisory-lock migration ✓. The 0.5.0 calculator-fix entry is a model of honest defect reporting. Fix: "savers" terminology (#12) and the missing per-crate server 0.2.0/0.3.0 headers (#9).

### `agentgraph/README.md` — **Approve with minor changes**
Both quickstart samples use the real API verbatim (verified against `state.rs`, `node.rs`, `executor.rs`, `lib.rs` prelude); the comparison tables are careful; roadmap checkmarks match shipped features. Fix: drop or source the "order of magnitude leaner" claim (#6), add a time-travel snippet (#14), "savers" (#12).

### `agentgraph/CONTRIBUTING.md` — **Revise (stale)**
Structurally excellent — module ownership table, concrete CI commands, contract-oriented doc guidance — but its two most actionable statements (executor is `todo!()`, JsonFileCheckpointer is `todo!()`, both listed as good first issues) describe a repository that no longer exists (#1). This is the single most misleading document in the repo.

### `agentgraph/examples/README.md` — **Approve**
All four examples exist; run commands correct; env-var table matches `live_agent.rs` defaults exactly (`llama3.1`, `http://localhost:11434/v1`, `ollama`); CI-safe-exit claim matches the example's design. No changes required.

### `agentgraph-server/README.md` — **Approve with minor changes**
The largest doc and the most thoroughly verified: endpoint table, run-payload JSON, fork responses, SSE frame-id format, config defaults, tenant-id and KV-segment limits, Postgres table names, cron semantics — all match source. Its own roadmap section is correctly versioned (v0.3 = time travel/postgres/CORS, v0.4 = multi-tenancy). Fix: the repeated unmeasured "~20 MB" claim (#6); consider noting thread-registry volatility more prominently than line 111's parenthetical.

### `agentgraph-otel/README.md` — **Approve with minor changes**
Every API-table row matches `src/lib.rs` (`init`, `init_local(&str)`, `OTelConfig` fields, `shutdown(&mut self)`, `DEFAULT_FILTER`, both error variants); span taxonomy matches the executor's instrumentation; docker-compose stack and `otel_demo` exist with the documented env var and service name. Fix: the broken inline-code span in the API table (#11).

### `agentgraph-worker/` — **Revise (missing)**
No README at all (#2). The crate is small enough that one page would close the gap.

### `sdks/python/README.md` — **Approve**
All 22 API-reference rows match `client.py` signatures (including `run_stream`'s `stream_mode`/`last_event_id`/`timeout` trailing params); the test-suite description matches reality (18 tests, the interrupt/resume skip is documented identically in both); `requires-python >=3.8` confirms the "Python 3.8+" claim. Tone is opinionated but honest about trade-offs. No required changes (pyproject URLs are finding #13, adjacent to this doc).

### `sdks/typescript/README.md` — **Approve with minor changes**
All 20 methods and the `lastEventId`/`streamMode` options verified against `src/index.js`; `engines >=18` and `"type": "module"` confirm the Node/ESM claims; the e2e-suite description matches the test file (17 tests, conditional 401 skip). Fix: stale `version: '0.3.0'` in the `info()` sample (#4).

### `docs/roadmap.md` — **Approve**
The only document that makes the platform-vs-crate version mapping explicit (its status table is the Rosetta stone for #9); every phase row matches the CHANGELOG; the "Explicitly rejected" section is excellent practice. No changes required.

### `docs/agentgraph-server-design.md` — **Approve with minor changes**
Honestly labeled a design draft with a 2026-08-05 status appendix listing implementation deviations — exactly how design docs should age. But the deviations section missed one: the §2 sample's `OpenAiCompatibleClient::from_env("OPENAI_API_KEY")?` never matched the shipped 3-arg non-`Result` API (#7). Historical table claim "v0.2 is single-process" etc. is fine in context.

### `docs/server-quickstart.md` — **Approve with required changes**
The tutorial code is the real API (`GraphRegistry`/`ServerConfig::new(bind, store)`/`serve` verified; the interrupt/resume pattern is idiomatic and correctly emphasizes resume-first node logic). Two required fixes: add the missing fork/replay section (#3) and refresh the stale `0.1.0` version stamps (#5).

### `docs/studio.md` — **Approve with minor changes**
Unusually candid ("not exercised in a real browser … visual/behavioral bugs are possible", a "Verification performed" section listing exactly what was and wasn't checked) — this is the anti-AI-tell document and other docs should copy its honesty. Claims about server_demo, CORS, and fork fallback match source and tests (`tests/time_travel.rs`, `tests/cors.rs` exist). Fix: the "v0.3 API" labels (#15).

### `docs/live-demo-transcript.md` — **Approve**
The most credible document in the repo: verbatim transcripts, wrong answers printed unflinchingly (5952 ≠ 5888, "37 words"), a defect found, root-caused, fixed, and re-run with the fix confirmed (`128 multiply 46 = 5888` ✓ against the actual `coerce_f64` code and its 5 tests). No hype, no cherry-picking. No changes required.

## Explicitly verified clean

- **Hype adjectives:** `robust / seamless / blazing / enterprise-grade / cutting-edge / state-of-the-art / best-in-class / revolutionary / powerful / leverage` — **zero occurrences** in all 15 in-scope documents (matches exist only in `research/`, where they quote other projects' marketing).
- **"Not just X but Y" constructions:** none found in scope.
- **Identical bullet shapes:** feature bullets across READMEs vary in structure and length; the roadmap checklists are repetitive by design (changelog style), not by laziness.
- **Broken internal links:** none found. Every relative link in every reviewed doc resolves to an existing file/section, including LICENSE files (root, `agentgraph/`, `agentgraph-server/` all carry both).
- **Version numbers in dependency snippets:** `agentgraph = "0.4"`, `agentgraph-server = "0.4"`, `opentelemetry 0.32` claims — all match `Cargo.toml`s.
- **Command samples:** `cargo run --example react_agent|parallel_fanout|human_in_loop|live_agent|server_demo|otel_demo`, `node --test test/`, `python3 -m unittest discover -s sdks/python/tests`, the gated Postgres test invocation, and the docker-compose flow — all reference files/examples that exist with the documented behavior.
- **Terminology:** "super-step" is hyphenated uniformly across all project docs; "checkpointer" is the dominant noun (two "saver" exceptions, finding #12); crate names and module paths are spelled consistently everywhere.
- **Response shapes:** every documented JSON response (`/ok`, `/info`, fork `201`, run terminal JSON, state `{values, next, checkpoint}`, interrupted-run body) matches the serializers in `agentgraph-server/src/routes.rs` and `src/runs.rs`.


---

## Reviewer_Documentation — Summary

**Scope covered:** all 15 prose documents (root README, CHANGELOG, 3 crate READMEs + CONTRIBUTING + examples README, both SDK READMEs, all 5 docs/*.md, live-demo transcript). `agentgraph-worker/` has no README — itself a finding. **30 claims spot-checked against source: 27 pass, 3 fail/stale.**

**Severity counts:** High 2 · Medium 7 · Low 7 (16 findings)

**Top 5 must-fixes:**

1. **(H) `agentgraph/CONTRIBUTING.md:54-55,61-62` is frozen at scaffold time** — labels `executor.rs` and `JsonFileCheckpointer` as "`todo!()`" and lists both as the top "good first issues," inviting contributors to re-implement shipped, tested code. Most misleading doc in the repo.
2. **(H) `agentgraph-worker/` has no README** — the only undocumented crate; `WorkerRegistry`/`router`/`POST /execute` wire contract exists only in rustdoc.
3. **(M) `docs/server-quickstart.md` never teaches fork/time-travel** — the flagship tutorial stops at history + rollback; `POST /threads/{id}/fork` and `"checkpoint": {"checkpoint_id": …}` replay (v0.4 headline features) are absent.
4. **(M) Stale version stamps in sample outputs** — `sdks/typescript/README.md:22` shows `version: '0.3.0'`, `docs/server-quickstart.md:131` shows `"0.1.0"`; server is 0.4.0.
5. **(M) Evidence-free claims repeated across docs** — "~20 MB static binary" asserted 5× with no measurement anywhere; "order of magnitude leaner than Python" unsourced. Also: design doc `OpenAiCompatibleClient::from_env("OPENAI_API_KEY")?` snippet uses a wrong signature (real: 3-arg, non-`Result`).

**AI-tell audit:** cleanest dimension — zero hype adjectives (robust/seamless/blazing/enterprise-grade = 0 hits in scope), no "not just X but Y", varied bullet shapes. Only real tell is em-dash density (~90 in the server README alone) plus recurring aphorisms; reads as one disciplined author. No broken internal links found. Version/terminology consistency is good ("super-step" uniform; two "saver" exceptions) with one structural wrinkle: platform-release numbers (v0.5.0) vs per-crate versions (server 0.4.0) are never reconciled in the root README.
