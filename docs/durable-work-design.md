# Durable Work design (R0.6)

Rusty's Durable Work release turns workers from remote-execution helpers into
a **durable activity system**: tasks that survive server crashes, worker
deaths, and deployments — and that retry safely because the runtime knows
what each task does to the world.

The promise, stated precisely: **effectively-once execution when
applications use idempotency — not universal exactly-once side effects.**
Delivery through the queue is at-least-once; the idempotency key, carried on
every envelope and passed to the effect itself, is what collapses duplicate
deliveries into one visible effect. Where an effect cannot be made
idempotent, Rusty does not pretend: the retry machinery refuses to re-drive
it silently (see the effect gate below).

This document is the design for the whole release. The shared contracts —
the retry taxonomy and the task envelope — land first, in
`rusty-core/src/durable.rs`, because the queue (server), the workers, and
the SDKs must all agree on them byte-for-byte. Golden-file tests under
`rusty-core/tests/golden/` pin the wire shapes; drift fails CI.

## Lineage, named

Durable Work stands on established patterns, and says so:

- **Saga / process-manager patterns** (Garcia-Molina & Salem 1987; the
  process-manager routing of enterprise integration) — long-running work
  decomposed into steps whose state lives outside any single process,
  recovered by re-driving from durable state rather than by keeping a
  process alive.
- **Temporal-style activity retries** — activities with declared retry
  policy (maximum attempts, backoff coefficients, non-retryable error
  types), server-side scheduling, and heartbeating workers. Our
  `ErrorClass` closed enum is the statically-typed version of Temporal's
  non-retryable error list.
- **SQS visibility timeouts** — a delivered message is invisible, not
  deleted; expiry returns it to the queue. Our leases are the same idea with
  an explicit owner and heartbeat.
- **Transactional outbox** (the microservices pattern) — state change and
  message emission committed in one transaction, published by a relay, so a
  crash can never produce "state saved, task lost" or the reverse.

## What Rusty does differently

Two things, both consequences of the Flight Recorder (R0.5) landing first:

1. **Effect classification drives retry safety.** Every journaled event —
   and now every `TaskEnvelope` — carries a declared [`Effect`] from the
   frozen taxonomy (`Pure` / `ReadOnly` / `Idempotent` / `Compensatable` /
   `NonIdempotent`). The retry policy does not guess from error strings or
   ask the application for a retryable-error list: it gates on the effect
   class. Work that is not `is_freely_repeatable()` is never silently
   retried, in any failure mode — including `Timeout`, where the work may
   already have happened. The `Idempotent` declaration *plus a stable
   idempotency key* is what unlocks automatic retry. This check lives in one
   function, `classify_retry`, shared verbatim by server and workers.
2. **Evidence is first-class.** Task lifecycle transitions — submitted,
   leased, attempt failed with class, retried, dead-lettered, completed,
   cancelled — are journaled as `RunEvent`s with causal parentage into the
   run that spawned them, and the envelope's `parent` field links a task
   tree into the run's causal chain. A dead-lettered task is not a log line;
   it is inspectable, replayable evidence with its full attempt history.
   Retry decisions are `DecisionFamily::Retry` decisions in the learning
   contract, so the R0.10 policy plane can later learn backoff policy from
   recorded outcomes — replay before learning.

## The contracts (`rusty-core/src/durable.rs`)

All types are `Serialize`/`Deserialize`, additive-evolution only: optional
fields carry serde defaults, `format_version` pins the envelope, and the
conservative default effect (`NonIdempotent`) means an undeclared task is
never silently retried.

### `ErrorClass` — why the attempt failed

Closed enum, declared by whoever ran the work (worker, transport, or lease
reaper), never inferred from logs:

| Class | Retry semantics |
|---|---|
| `transient` | Retry with backoff; expected to succeed later. |
| `rate_limited` | Retry with backoff; callee `Retry-After` floors the delay (scheduler-side). |
| `timeout` | Retry with backoff — but the attempt may have partially executed, so the effect gate decides first. |
| `invalid_input` | Never retried; the same bytes fail the same way. Fails the task immediately. |
| `dependency_failure` | Retry with backoff; distinct from `transient` so telemetry separates "their outage" from "our wiring". |
| `resource_exhausted` | Retry with backoff, ideally placed elsewhere (scheduler's concern). |
| `cancelled` | Never retried, never dead-lettered — control flow, not failure. Keeps the retry machinery out of the cancellation path. |
| `unknown` | Retry to the attempt limit, then dead-letter. Unclassified handler errors and lease-expiry reaping land here; unknowns are the DLQ's primary input. |

### `RetryDecision` + `classify_retry` — one policy, shared verbatim

A failed attempt maps to exactly one decision — `retry { after_ms }`,
`dead` (dead-letter), or `fail` — through four gates, in order:

1. **Effect gate** — not `Effect::is_freely_repeatable()` → `fail`. Never
   silently re-drive a non-idempotent or compensatable effect.
2. **Class gate** — `invalid_input` / `cancelled` → `fail`.
3. **Attempt gate** — attempts exhausted (`attempt >= max_attempts`) →
   `dead`.
4. Otherwise → `retry` with `backoff_delay_ms(attempt, uniform)`.

### Backoff policy

Exponential with **full jitter**: retry `n` (1-based) draws uniformly from
`[0, 1s × 2^(n−1)]`, **capped at 5 minutes** (`BASE_RETRY_DELAY_MS = 1_000`,
`MAX_RETRY_DELAY_MS = 300_000`). Full jitter — uniform over the whole
exponential range, not a fixed delay plus noise — is what decorrelates a
fleet of tasks that failed together when a shared dependency recovers (the
thundering-herd problem; the AWS Architecture Blog's "Exponential Backoff
And Jitter" analysis is the reference). The jitter sample is a parameter,
not an internal draw: schedulers source it from the run's seeded
`RngSource`, so a recorded run reproduces its retry schedule exactly under
replay. **Attempt limits** come from the envelope's `TaskBudget`
(`max_attempts`, per-attempt `timeout_ms`); a task without a budget takes
the queue's defaults.

### `TaskEnvelope` — the unit of work

One serde-versioned struct carrying: `task_id`; `parent` (causal link into
the run's event tree); `sender` / `recipient` (a worker pool name, or a
pinned worker identity); `input` as a Flight Recorder `PayloadRef` (inline
≤ 4 KiB, content-addressed above — the queue row stays cheap to scan and
artifact addressing is shared with the journal); `output_contract` (an
`ArtifactContract`: kind + optional size bound; full payload schema
validation is R0.7's typed-contract work); `deadline` (whole-task, across
attempts); `budget`; `idempotency_key`; and the declared `effect`.

The idempotency key is load-bearing, not decorative: the queue refuses a
duplicate submission with an existing key, and the recipient passes the key
to the effect it performs. `None` is honest only for `Pure` / `ReadOnly`
work.

## The lease / visibility-timeout model (wave 1)

The queue is a Postgres table in `rusty-server` (same store family as
`server_journals`; advisory-locked auto-migrations as established). Rows
carry the envelope, status, attempt count, lease owner, and lease expiry.

- **Delivery is a lease, not a deletion.** A worker that pops a task takes
  a lease (default 30 s); the task is invisible to other workers until the
  lease expires. This is the SQS visibility-timeout idea with an explicit
  owner identity.
- **Heartbeats renew.** A healthy worker heartbeats to extend the lease
  while the attempt runs (per-attempt `timeout_ms` bounds how far). A
  worker that dies stops heartbeating; the lease expires and the task
  returns to visibility with its attempt counter incremented — safe
  reassignment with no double execution beyond the at-least-once the
  idempotency key already absorbs.
- **Lease-expiry reaping classifies as `unknown`.** A dead worker tells us
  nothing about whether the effect fired; the effect gate and attempt
  budget handle it like any other unclassified failure.
- **Crash recovery is the release proof:** kill the server and a worker
  mid-effect, restart, and the run completes without losing state or
  duplicating the external effect — the checkpointed run state resumes, the
  leased task returns to visibility, and the idempotency key makes the
  re-attempt a no-op at the effect.

## Dead-letter policy (wave 1)

A task dead-letters when a retryable failure class exhausts its attempt
budget (gate 3) or an `unknown` failure keeps recurring. Non-retryable
classes (`invalid_input`) and non-repeatable effects do **not** dead-letter
— they `fail` immediately, because re-driving the same input fixes nothing
and the DLQ is for actionable work, not a graveyard. DLQ entries keep the
full envelope plus the attempt history (classes, decisions, timings) as
evidence; operators inspect them, fix the cause, and re-drive by hand.
`cancelled` never enters the DLQ. Tenant quotas (wave 3) count DLQ depth
against the tenant — an unbounded DLQ is a quiet disk-full outage.

## Transactional outbox + effect receipts (wave 2)

The split-brain the outbox kills: a node completes, writes state (or a
checkpoint), and submits a task — and crashes between the two. Wave 2 makes
state change and task submission one Postgres transaction: the outbox row
is written with the checkpoint, and a relay publishes outbox rows into the
queue at-least-once. Publish is idempotent on the task's idempotency key,
so a relay retry cannot double-submit.

**Effect receipts** close the loop the other way: when a task performing an
`Idempotent` effect completes, the recipient journals the receipt (the
effect's own confirmation — the provider's id, the stored key) as a
`RunEvent` causally parented to the task. Exact replay can then serve the
receipt instead of re-sending the effect — the same rule the Flight
Recorder already applies to journaled model and tool calls, extended across
a crash boundary.

## Cancellation propagation + drain (wave 2)

- **Propagation.** Cancellation is a tree: cancelling a run cancels its
  outstanding tasks; cancelling a task signals the leased worker, which
  aborts the attempt and reports `cancelled` — never retried, never
  dead-lettered. Deadline expiry is cancellation by clock: the scheduler
  stops re-queuing, the worker treats an expired deadline as `cancelled`.
  A worker that misses the signal (partition, slow handler) is cleaned up
  by ordinary lease expiry; cancellation is a hint for promptness, not the
  correctness mechanism.
- **Drain.** A worker asked to drain (deployment, scale-down) stops taking
  new leases, finishes or fast-fails in-flight attempts within a grace
  period, and releases the rest — which return to visibility for other
  workers. The server drains its per-thread run queues the same way, so a
  rolling deploy never strands a leased task longer than one lease period.

## Pools, quotas, version pinning, autoscaling signals (wave 3)

- **Named pools** with per-pool concurrency limits: the envelope's
  `recipient` is a pool name; the scheduler hands out leases up to the
  pool's limit. A GPU-bound pool and an IO-bound pool coexist without
  starving each other.
- **Tenant quotas** — tasks queued, tasks in flight, DLQ depth — enforced
  at submission under the existing `{tenant}/` id-namespacing isolation;
  cross-tenant tasks do not exist, as with every other server resource.
- **Version pinning** for in-flight runs: a run started against worker
  version `w1` keeps dispatching its tasks to `w1`-capable workers until it
  finishes, so a deploy mid-run never changes semantics under an in-flight
  execution (the same fork-first conservatism as time travel).
- **Autoscaling signals** are metrics, not mechanisms: queue depth,
  oldest-visible-task age, lease saturation per pool. Rusty publishes the
  signals; the autoscaler is the operator's HPA/KEDA/etc. Evidence over
  claims: wave 3 ships with published numbers for these signals under load,
  against the [benchmarks](benchmarks.md) baseline.

## Composition with the Flight Recorder

The two systems are one system seen from two sides:

- **Task lifecycle is journaled.** Submission, lease, failure-with-class,
  retry decision, dead-letter, completion, cancellation — each a `RunEvent`
  in the run's journal, causally parented, so a run that fans out into
  durable tasks is one connected evidence tree from super-step to queue to
  effect receipt.
- **`Effect::Idempotent` is the safety contract.** The Flight Recorder
  froze the taxonomy; Durable Work is the first policy that *consumes* it.
  Retry safety is a classification check, not a hope.
- **Retry decisions are learning evidence.** Each `classify_retry` outcome
  is recordable as a `DecisionFamily::Retry` `DecisionEvent` (features:
  error class, attempt, dependency latency; legal actions: retry/abort;
  propensity from the active policy version), which is what makes the
  R0.10 retry-policy learning wedge well-posed from day one.
- **Determinism carries through.** Backoff jitter draws from the run's
  seeded `RngSource`; task event timestamps read from the run's clock. A
  recorded run's retry schedule is reproducible.

## Explicitly not promised

- **No universal exactly-once.** Delivery is at-least-once; exactly-once
  *effects* require idempotency, and Rusty enforces the honesty of that by
  refusing to silently retry effects that don't declare it.
- **No mid-attempt checkpointing.** The activity boundary is the
  granularity, same as the super-step boundary in the executor; partial
  progress inside an attempt is lost on lease expiry and re-executed.
- **No automatic compensation in v1.** `Compensatable` effects fail closed
  today; pairing effects with their compensations (the saga half of the
  lineage) is later work, gated on real demand.
- **No built-in autoscaler.** Signals in wave 3; scaling decisions stay
  with the operator's infrastructure.
