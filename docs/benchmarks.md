# Benchmarks — Rusty Core (`rusty-agent-runtime`)

Initial Criterion benchmark suite for the core engine. These numbers exist so
that performance claims about the runtime are backed by published, reproducible
measurements rather than intuition.

> **Status: baseline.** This is the first published run (2026-08-06). It
> establishes the measurement harness and a single-machine baseline; it is not
> a regression history yet.

## How to reproduce

```bash
export PATH="$HOME/.cargo/bin:$PATH"

# Compile-only check:
cargo bench -p rusty-agent-runtime --no-run

# Full suite (takes ~7 minutes with Criterion defaults):
cargo bench -p rusty-agent-runtime

# Individual targets:
cargo bench -p rusty-agent-runtime --bench graph_compile
cargo bench -p rusty-agent-runtime --bench node_execution
cargo bench -p rusty-agent-runtime --bench parallel_fanout
cargo bench -p rusty-agent-runtime --bench reducers
cargo bench -p rusty-agent-runtime --bench checkpoint
cargo bench -p rusty-agent-runtime --bench interrupt_resume
cargo bench -p rusty-agent-runtime --bench state_clone
```

Results (JSON estimates) are written to `target/criterion/<group>/<id>/new/estimates.json`.
The suite uses Criterion's default configuration: 3 s warm-up, 100 samples per
benchmark, 95 % confidence intervals. Async benchmarks drive the executor via a
single multi-threaded tokio runtime created outside the measurement loops.

## Environment

| | |
|---|---|
| CPU | Apple M2 Max (12 cores: 8 performance + 4 efficiency) |
| RAM | 96 GB |
| OS | macOS 26.5.1 (Build 25F80), arm64 |
| Rust | rustc 1.97.1 (8bab26f4f 2026-07-14) |
| Criterion | 0.5.1 (default features off: no plotters/rayon) |
| Date of run | 2026-08-06 |
| Crate version | `rusty-agent-runtime` 0.4.0 |
| Load | single-user machine, no other heavy processes |

Absolute numbers are only meaningful on comparable hardware; treat ratios
(scaling behavior) as the portable signal.

## Results

All values are Criterion mean estimates with 95 % confidence intervals in
brackets.

### Graph compilation — `GraphBuilder::compile()`

Linear chains (n nodes, n−1 static edges). Builder wiring excluded; only
`compile()` (validation + assembly) is measured.

| Nodes | Mean | 95 % CI |
|---|---|---|
| 10 | 1.01 µs | [1.01, 1.03] µs |
| 100 | 10.88 µs | [10.78, 11.00] µs |
| 1000 | 126.26 µs | [125.32, 127.50] µs |

### Sequential node execution — chain graph end-to-end

`Executor::run` with no checkpointer. Each node reads the previous channel,
increments, writes its own channel (real work, not a no-op graph).

| Chain length | Mean | 95 % CI | Approx. per super-step |
|---|---|---|---|
| 10 nodes | 106.63 µs | [104.84, 108.98] µs | ~10.7 µs |
| 50 nodes | 593.65 µs | [586.61, 602.57] µs | ~11.9 µs |
| 100 nodes | 1.344 ms | [1.334, 1.357] ms | ~13.4 µs |

### Parallel fan-out / fan-in

Static fan-out: `source → N branch nodes (same super-step, parallel tasks) → sink`,
branch writes merged via `Reducer::Append` at the barrier. `Executor::run`,
no checkpointer.

| Branches | Mean | 95 % CI |
|---|---|---|
| 2 | 33.39 µs | [33.15, 33.76] µs |
| 8 | 49.28 µs | [48.98, 49.68] µs |
| 32 | 144.16 µs | [142.42, 146.72] µs |

### Reducer merge cost — `StateSpec::apply_single`

The real barrier merge path (channel validation + reducer), one write per
measurement.

**Overwrite** (replace value of given size):

| Value size | Mean | 95 % CI |
|---|---|---|
| 1 KB | 206.36 ns | [205.08, 207.66] ns |
| 100 KB | 235.64 ns | [230.36, 242.58] ns |
| 1 MB | 253.12 ns | [242.78, 265.07] ns |

**Append** (push one element onto an existing array):

| Existing array length | Mean | 95 % CI |
|---|---|---|
| 10 | 1.45 µs | [1.40, 1.49] µs |
| 100 | 12.45 µs | [12.13, 12.73] µs |
| 1,000 | 123.50 µs | [120.50, 125.96] µs |
| 10,000 | 1.184 ms | [1.175, 1.193] ms |

**DeepMerge** (merge a 10 %-overlap object into an existing object):

| Existing object keys | Mean | 95 % CI |
|---|---|---|
| 100 | 34.50 µs | [33.66, 35.24] µs |
| 1,000 | 412.24 µs | [404.56, 418.71] µs |
| 10,000 | 3.918 ms | [3.863, 3.981] ms |

### Checkpoint serialization + save

Checkpoint carrying a state with a single string payload of the given size.

**Serialize only** (`serde_json::to_vec_pretty`, pure CPU):

| State size | Mean | 95 % CI |
|---|---|---|
| 1 KB | 843.31 ns | [838.21, 849.33] ns |
| 100 KB | 34.57 µs | [34.32, 34.92] µs |
| 1 MB | 368.49 µs | [366.19, 371.17] µs |

**InMemoryCheckpointer::put** (mutex + move into store; payload-independent
because the checkpoint is moved, not copied):

| State size | Mean | 95 % CI |
|---|---|---|
| 1 KB | 1.76 µs | [1.66, 1.85] µs |
| 100 KB | 1.57 µs | [1.48, 1.65] µs |
| 1 MB | 804.09 ns | [758.04, 842.56] ns |

**JsonFileCheckpointer::put** (serialize + atomic temp-write + rename +
latest-pointer write):

| State size | Mean | 95 % CI |
|---|---|---|
| 1 KB | 487.67 µs | [396.72, 611.72] µs |
| 100 KB | 628.27 µs | [588.89, 669.46] µs |
| 1 MB | 1.138 ms | [1.063, 1.226] ms |

**JsonFileCheckpointer::get_latest** (pointer read + file read + deserialize —
the resume-path load):

| State size | Mean | 95 % CI |
|---|---|---|
| 1 KB | 48.27 µs | [47.63, 49.03] µs |
| 100 KB | 67.79 µs | [67.15, 68.54] µs |
| 1 MB | 256.81 µs | [252.52, 264.34] µs |

### Interrupt / resume round-trip

Full HITL cycle through the real executor + `InMemoryCheckpointer`: phase 1
runs and suspends at the interrupting node (checkpoint persisted), phase 2
restores the checkpoint and completes.

| Carried state | Mean | 95 % CI |
|---|---|---|
| Empty | 38.57 µs | [37.82, 39.62] µs |
| 100 KB blob channel | 85.78 µs | [84.13, 87.74] µs |

### State cloning cost

`State::clone()` (deep clone of the underlying `serde_json` map) and the full
serialize → parse round-trip a durable checkpoint pays in both directions.
Payload: one JSON string of the given size plus a small `meta` object.

| Payload size | `State::clone()` | serde round-trip |
|---|---|---|
| 1 KB | 221.30 ns [220.23, 222.75] | 1.08 µs [1.07, 1.09] |
| 100 KB | 1.92 µs [1.91, 1.94] | 47.39 µs [47.18, 47.63] |
| 1 MB | 17.50 µs [17.29, 17.83] | 483.92 µs [477.16, 492.55] |
| 10 MB | 248.65 µs [246.14, 251.26] | 4.61 ms [4.52, 4.76] |

## Interpretation — what these numbers do and do not show

**What they are.** Single-machine microbenchmarks of the core engine in
isolation: graph compilation, the super-step loop, reducer merges, checkpoint
ser/de and savers, the interrupt/resume protocol, and state cloning. All node
"bodies" are small deterministic computations; there is **no network, no LLM
call, no database, no SSE streaming** anywhere in these measurements.

**What they show.**

- **Engine overhead is small relative to real agent work.** A full super-step
  (plan → snapshot → parallel node run → barrier merge → route) costs on the
  order of **10–13 µs** in the sequential-chain measurements. Any node that
  calls an LLM (hundreds of ms to seconds) dwarfs this by 4–5 orders of
  magnitude; engine overhead is not the bottleneck for LLM-bound workloads.
- **Graph compilation is effectively free at realistic sizes** (~1 µs for 10
  nodes, ~126 µs even for a 1000-node chain) and scales linearly.
- **Fan-out scales sub-quadratically in branch count**: 2→8→32 branches costs
  33 µs → 49 µs → 144 µs — roughly linear with a small fixed per-super-step
  base, as expected for barrier scheduling of trivial tasks.
- **`Reducer::Overwrite` is O(1) in value size** (~250 ns flat from 1 KB to
  1 MB): the update is moved, not merged.
- **`Reducer::Append` and `Reducer::DeepMerge` are O(N) per write** because
  each merge clones the current channel value (Append: ~1.4 µs at 10 elements
  → ~1.18 ms at 10,000; DeepMerge: ~35 µs at 100 keys → ~3.9 ms at 10,000).
  Long-lived `Append` channels that grow unboundedly (e.g. accumulating every
  event of a long run into one array) make each subsequent write linearly
  more expensive — a super-step writing into a 10 k-element array pays ~1.2 ms
  for the merge alone. This is the clearest scaling hazard in the current
  design.
- **Checkpointing is cheap until payloads grow.** Serialization runs at
  ~2.8 GB/s (1 MB in ~370 µs); the JSON-file saver adds a roughly constant
  ~450–600 µs of filesystem work (two atomic writes) on top, so it only
  becomes payload-bound past ~1 MB. `InMemoryCheckpointer::put` is
  payload-independent (move semantics, sub-2 µs).
- **Interrupt/resume protocol overhead is ~39 µs** with an empty state —
  i.e. negligible next to any human-in-the-loop latency — rising to ~86 µs
  when carrying a 100 KB state (the checkpoint write + load of the payload).

**On state cloning specifically.** The executor hands every node a full state
snapshot, so cloning is on the hot path. Measured: a 1 MB state clones in
~17.5 µs and a 10 MB state in ~249 µs. Two honest caveats:

1. These payloads are one large JSON **string** — memcpy-bound, a best case
   per byte. A structurally *deep* 10 MB state (hundreds of thousands of small
   values) will clone measurably slower per byte because of per-value
   allocation. The `Append`/`DeepMerge` numbers above are the better proxy for
   structured data.
2. Even so, full-state cloning stays below ~1 ms per super-step up to ~10 MB
   payload on this machine. It becomes *visible* — i.e. comparable to the
   engine's own per-step overhead and worth avoiding — in the **1–10 MB
   range and beyond** (17–250 µs per clone, multiplied by every snapshot the
   executor takes per super-step, plus the ~0.5–4.6 ms serde round-trip when a
   durable checkpointer is attached). Below ~100 KB it is noise (< 2 µs).

**What they do NOT show (explicitly out of scope).**

- **Server load-testing is not covered yet.** No concurrent threads/sessions,
  no SSE streaming throughput, no Postgres checkpointer contention, no
  multi-tenant executor sharing. These are tracked as follow-up work
  (server-level load suite against `rusty-server`, including
  `PostgresCheckpointer` under concurrent writers).
- No cross-machine or cross-OS comparison; no regression history (this is the
  baseline run); no memory-usage or allocation profiling; no comparison
  against LangGraph or other runtimes.
- Criterion measures wall-clock latency of single operations; throughput
  under contention can behave differently.
