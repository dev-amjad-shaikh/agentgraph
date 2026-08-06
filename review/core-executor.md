# Review: Core Executor & Checkpointing

**Reviewer:** Reviewer_Core_Executor (staff-level, READ-ONLY review)
**Scope:** `agentgraph/src/executor.rs`, `agentgraph/src/checkpoint.rs`, `agentgraph/src/checkpoint_postgres.rs`
**Verdict (overall):** Solid, unusually well-documented core with real transactional super-step semantics — but one genuine correctness hole around interrupt/resume of parallel nodes, one API footgun in `RunConfig::default()`, and several cross-backend consistency gaps in the checkpoint layer.

## Findings

Severity: **H**igh / **M**edium / **L**ow. Categories: AI-tell, Idiom, Correctness, Docs.

| # | Location | Severity | Category | Gist | Fix |
|---|----------|----------|----------|------|-----|
| 1 | `agentgraph/src/executor.rs:632-685` | H | Correctness | On `Interrupt` the barrier loop `break`s, the `JoinSet` is dropped (aborting in-flight siblings), and the suspension checkpoint schedules **only** the interrupting node (`vec![name]`). Parallel sibling nodes that were mid-flight are silently dropped from history: after resume they never re-run, and any work they did before abort is lost. LangGraph persists all pending tasks at an interrupt. | Persist the full pending set: include every active node that had not yet completed (or recompute from the last boundary checkpoint). If the simplification is deliberate, document it loudly on `ExecutionOutcome::Interrupted` and `NodeContext::interrupt` — current docs imply only the interrupting node is affected. |
| 2 | `agentgraph/src/executor.rs:98` (`#[derive(Default)]` on `RunConfig`) | H | Correctness / API | `RunConfig` derives `Default`, so `RunConfig::default().max_steps == 0` — the documented default (1000, field doc line 105-107 and `DEFAULT_MAX_STEPS` line 190) only applies via `RunConfig::new`. Any run built from `RunConfig::default()` immediately fails with "max_steps (0) exceeded". | Implement `Default` manually delegating to `RunConfig::new(String::new())`, or remove the derive. |
| 3 | `agentgraph/src/executor.rs:690-711` | M | Correctness / Docs | `GraphEvent::StateUpdate.updates` is built by last-write-wins insertion into one map: when several nodes write the same channel in one super-step (the normal `Append` fan-in case), earlier writes vanish from the event. Values are also the raw pre-reducer partials, while the rustdoc (line 231) promises "merged partial updates (channel -> new value)". | Emit per-node writes (`Vec<(node, channel, value)>`), or read post-reducer values out of `state` after `apply_super_step` so the event matches its doc. |
| 4 | `agentgraph/src/executor.rs:319-363` | M | AI-tell / Docs | Rustdoc carries a full pseudocode "implementation plan" that has already drifted: it says `join_set.join_all().await`, the code uses a `join_next()` loop; it omits `Send`/scoped-state handling and the sibling-drop behavior of finding #1. Pseudocode embedded in docs rots. | Collapse to a prose summary of phase semantics, or regenerate the block from the actual code and add a comment in `execute_super_step` telling future editors to keep it in sync. |
| 5 | `agentgraph/src/checkpoint.rs:364-406` | M | Correctness (race) | `JsonFileCheckpointer::put` is not serialized per thread: two concurrent same-thread puts can interleave (file A, file B, pointer B, pointer A), leaving `latest` pointing at the older checkpoint; the `get_latest` fast path then trusts the pointer and returns a stale checkpoint. The trait doc (line 80-81) defines recency as insertion order but nothing enforces an order. | Serialize puts per thread (e.g. a `Mutex` keyed by thread_id, or a lock file), or CAS the pointer by comparing `step`/`created_at` before overwriting. At minimum document a single-writer-per-thread precondition. |
| 6 | `agentgraph/src/checkpoint_postgres.rs:70-74` (`LIST_SQL`) | M | Correctness | `ORDER BY step ASC` has no tie-break, but replay-on-same-thread legitimately appends checkpoints with the **same** `step` value (executor restores `step` from the checkpoint and re-checkpoints at that boundary). `list()` order for same-step rows is then DB-dependent, and `fork_thread` truncates by list position — so fork cuts become nondeterministic. `GET_LATEST_SQL` already has the `created_at` tie-break; `LIST_SQL` doesn't. | `ORDER BY step ASC, created_at ASC, checkpoint_id ASC`. |
| 7 | `agentgraph/src/checkpoint_postgres.rs` (no `get_by_id` override; default at `agentgraph/src/checkpoint.rs:92-95`) | M | Idiom / Perf | `PostgresCheckpointer` inherits the default `get_by_id`, which runs `list()` (fetches and decodes **every** checkpoint of the thread) then linear-searches. The PK `(thread_id, checkpoint_id)` would make this an index point-lookup; `checkpoint_id` replay hits this on every time-travel run. The trait doc explicitly invites overrides. | Override with `SELECT ... WHERE thread_id = $1 AND checkpoint_id = $2`. |
| 8 | `agentgraph/src/checkpoint.rs:80-81` vs `checkpoint.rs:408-431` vs `checkpoint_postgres.rs:62-67` | M | Correctness / Docs | The three `get_latest` implementations disagree about "recency": trait says insertion order; in-memory = last push; JSON file = last pointer write (with a highest-step scan fallback); Postgres = highest step. Under out-of-step-order puts (replay, forks) the backends return different checkpoints for the same logical history. The test at `checkpoint.rs:567-570` codifies the discrepancy instead of resolving it. | Pick one definition (highest `(step, created_at)` is the most defensible) and implement it uniformly; add a cross-impl conformance test. |
| 9 | `agentgraph/src/executor.rs:575-583` vs docs at `executor.rs:111-112` | L | Docs / Correctness | `pending_resume.clone()` is injected into **every** node's `NodeConfig` in the first super-step; the rustdoc says "the interrupted node re-executes with `resume_value` returning this value". On replay+resume against a multi-node `next_nodes` checkpoint, all of them observe the resume value. | Document the broadcast semantics, or only attach the resume value when the active set came from an interrupt checkpoint. |
| 10 | `agentgraph/src/executor.rs:667-684` | L | Docs | With no checkpointer configured, a fresh UUID is still minted and returned as `Interrupted.checkpoint_id` — an id that names nothing and can never be replayed. The field rustdoc (line 76-78) says "The checkpoint persisted at the suspension point". | Return `Option<String>` or state in the docs that the id is meaningful only when a checkpointer is attached. |
| 11 | `agentgraph/src/checkpoint.rs:376-389` | L | Correctness (TOCTOU) | Duplicate-id check is `try_exists` then `atomic_write` (rename) — a concurrent duplicate `put` can pass the check and overwrite. Only reachable via fork re-put or manual id reuse (ids are unique by construction), hence Low. | Use create-new semantics (`OpenOptions::create_new(true)`) for the checkpoint file. |
| 12 | `agentgraph/src/executor.rs:106, 189-190` | L | Docs | Claims `DEFAULT_MAX_STEPS = 1000` "matches LangGraph's default `recursion_limit`" — LangGraph's default recursion_limit is widely documented as 25, not 1000. Verify against current LangGraph docs; either correct the number or drop the "matches" claim. | Fix the comparison or remove it. |
| 13 | `agentgraph/src/checkpoint_postgres.rs:127-149` | L | Idiom | Hand-rolled per-column `try_get(...).map_err(...)` boilerplate for six columns; `#[derive(sqlx::FromRow)]` on `CheckpointRow` eliminates the whole function. | Derive `FromRow`. |
| 14 | `agentgraph/src/checkpoint.rs:408-428` | L | Idiom | Three-deep nested `if let` fast path in `get_latest`; readable but a combinator chain (`tokio::fs::read(...).await.ok()` etc.) would flatten it. | Optional refactor. |
| 15 | `agentgraph/src/executor.rs:535, 478-480, 544-548` | L | AI-tell | A few narrating comments restate what the code (and the module doc's 6-phase list) already says, e.g. "the active set is fully determined at this point." Most other comments in this file genuinely earn their keep (JoinSet/`.instrument()` rationale, transactional-barrier notes). | Trim the pure narration; keep the rationale comments. |
| 16 | `agentgraph/src/executor.rs:91-92, 294-296, 298-301`; `checkpoint_postgres.rs:230-233`; `checkpoint.rs:172-175, 241-244` | L | AI-tell | Signature-restating rustdoc on trivial accessors: "`true` if the run was interrupted.", "The configured checkpointer, if any.", "The underlying connection pool.", "An empty in-memory store." | Either delete (the signature is the doc) or add one fact the signature can't carry. |
| 17 | `agentgraph/src/checkpoint.rs:163` | L | AI-tell | Marketing adjective: "In-memory checkpointer: **fast**, thread-safe, lost on restart." | "O(1) lookups behind a single mutex" or just drop "fast". |

**Severity counts:** High 2 · Medium 6 · Low 9 (17 findings)

## Explicitly verified clean

- **SQL injection:** all Postgres statements use bound parameters (`$1..$6`); no string interpolation of any user-controlled value anywhere in `checkpoint_postgres.rs`. Table/column names are compile-time constants. Clean.
- **TODO/FIXME/placeholder leftovers:** none in any of the three files.
- **Silent failure swallowing:** every `let _ = ...` site carries an explicit rationale — `Executor::emit` ("Best-effort event emission: a full or closed channel never aborts a run", executor.rs:820-825), temp-file cleanup (`checkpoint.rs:275, 282`, documented "Best-effort temp cleanup on failure"), test-only `remove_dir_all`/`write!`. No unjustified swallows.
- **Step-counter truncation/overflow:** Postgres path converts `usize -> i64` with `try_from` on write and `i64 -> usize` (rejecting negatives) on read (`checkpoint_postgres.rs:109-114, 155-160`). `duration_ms` uses `as u64` from `u128` millis — practically unreachable overflow. Executor `step`/`steps_run` are bounded by `max_steps` per run. Clean.
- **Transactional super-step:** on node failure the barrier returns before `apply_super_step` is ever called and `state` is untouched; `apply_super_step` validates all channels before merging, so a validation error also leaves state unmodified. JoinSet drop aborts stragglers in both failure and interrupt paths. Semantics hold (modulo finding #1).
- **Dead code / everything-pub:** all pub items in scope are used internally or form the deliberate public API; the all-pub fields on `Checkpoint`/`RunConfig`/`Command` are consistent DTO style. No dead code found.

## Per-file verdicts

### `agentgraph/src/executor.rs` — **Approve with required changes**
The super-step engine is well-architected: snapshot isolation, JoinSet barrier, validation-before-merge, and interrupt-as-control-flow are all correctly implemented, and the tracing story is thoughtful. Must-fix: the interrupt path's silent loss of parallel sibling nodes (#1) and the `RunConfig::default()` step-limit footgun (#2). Should-fix: `StateUpdate` event fidelity (#3) and the drifted doc pseudocode (#4). Comment density is high but mostly justified; a light trim of narration (#15) and restating rustdoc (#16) would de-AI the surface.

### `agentgraph/src/checkpoint.rs` — **Approve with required changes**
Trait design is good (default `get_by_id`/`fork_thread` with honest override guidance), atomic-write discipline is right, and the graceful-degradation read paths are well tested. Must-fix: the per-thread `latest`-pointer race (#5) and the cross-backend recency inconsistency (#8) — the latter needs a single definition before a third backend makes it worse. Nice-to-have: TOCTOU hardening (#11), combinator flattening (#14), "fast" (#17).

### `agentgraph/src/checkpoint_postgres.rs` — **Approve with minor changes**
The cleanest of the three files. Parameterization, error mapping (23505 special-case), advisory-lock-serialized migration, and checked numeric conversions are all correct; the no-DB unit tests for SQL shape and row mapping are a good pattern. Should-fix: add the `LIST_SQL` tie-break (#6) and override `get_by_id` to use the PK index (#7). Nice-to-have: `FromRow` derive (#13); consider exposing `max_connections` instead of the hardcoded 5 (`checkpoint_postgres.rs:190`).


---

## Final summary (from agent reply)

## Severity counts

**High 2 · Medium 6 · Low 9 — 17 findings total**

## Top 5 must-fixes

1. **Interrupt silently loses parallel sibling nodes** — `executor.rs:632-685`. On `Interrupt` the barrier breaks, `JoinSet` drop aborts in-flight siblings, and the checkpoint schedules only the interrupting node (`vec![name]`). Sibling work vanishes from history and never re-runs after resume. LangGraph persists all pending tasks.
2. **`RunConfig::default()` footgun** — `executor.rs:98`. The derived `Default` yields `max_steps == 0`; the documented default of 1000 applies only via `RunConfig::new`. Any `default()`-built run instantly fails with "max_steps (0) exceeded".
3. **`JsonFileCheckpointer` latest-pointer race** — `checkpoint.rs:364-406`. Concurrent same-thread puts interleave checkpoint-file vs pointer writes; `latest` can end up pointing at the older checkpoint, and the `get_latest` fast path trusts it.
4. **Postgres `LIST_SQL` missing tie-break** — `checkpoint_postgres.rs:70-74`. Replay legitimately creates same-step duplicates; `ORDER BY step ASC` alone makes `list()` order DB-dependent, and `fork_thread` truncates by list position → nondeterministic fork cuts.
5. **`PostgresCheckpointer` doesn't override `get_by_id`** — inherits the trait default that fetches and decodes the thread's entire history, then linear-searches, instead of using the `(thread_id, checkpoint_id)` PK index. Hit on every time-travel replay.

## Notes on the hunt categories

- **AI-tells:** found but mild — drifted pseudocode "implementation plan" in rustdoc (`join_all` vs actual `join_next`), a few narrating comments, signature-restating accessor rustdoc, one "fast" adjective. No TODO/FIXME/placeholder leftovers, no dead code, no everything-pub smell (pub fields are deliberate DTO style), and every `let _ =` swallow carries a rationale comment.
- **Verified clean:** SQL parameterization (all `$n` binds, zero interpolation), step-counter conversions (`try_from` both directions), transactional super-step semantics (validation-before-merge, JoinSet abort on failure), and the interrupt/JoinSet instrumentation rationale comments are genuine.
- Also flagged: `GraphEvent::StateUpdate` is lossy for multi-writer channels and shows pre-reducer values despite its doc; cross-backend `get_latest` "recency" definitions disagree (test at `checkpoint.rs:567-570` codifies the discrepancy); `DEFAULT_MAX_STEPS = 1000` doc claims to match LangGraph's default `recursion_limit`, which is widely documented as 25 — worth verifying.
