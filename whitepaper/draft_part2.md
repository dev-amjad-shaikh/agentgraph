# Rust for the Agentic Core Engine — Part II (Sections 6–10)

*Draft, 2026-07-31. Sections 1–5 are in `draft_part1.md`. Vendor-published benchmark figures are labeled as such; figures without independent reproduction are treated as directional.*

---

## 6. Core in Rust vs. Whole Engine in Rust: The Central Tradeoff

Once a team accepts that an agentic engine needs a Rust component, the real decision is *where to stop*: only the performance-critical core in Rust behind Python/TypeScript bindings, or the entire engine?

The empirical record from 2024–2026 is unambiguous: the dominant, repeatedly validated pattern is the **hybrid**. Polars, pydantic-core, delta-rs, swc/Turbopack, and Tauri all follow it (https://dskrzypiec.dev/polars/; https://github.com/delta-io/delta-rs; https://v2.tauri.app/blog/tauri-20/). It works because it separates two concerns with different optimization targets: the *hot path* (throughput, memory, latency determinism) goes to Rust; the *adoption surface* (developer experience, ecosystem gravity, hiring pool) stays in Python or TypeScript. As delta-rs's maintainer put it, the Python bindings "exploded the possible user and contributor base" — bindings are a growth strategy, not just an API (https://www.buoyantdata.com/blog/2025-03-09-lessons-learned-building-delta-rs.html, 2025-03-09).

Full-Rust platforms win under a different condition: when the deliverable *is* a self-contained artifact. uv ships as a single static binary with no Python dependency (https://astral.sh/blog/uv, 2024-02-15); Deno is a full-Rust runtime distributed as one executable (https://deno.com/blog/open-source, 2025-10-16); Tauri's value proposition *is* its Rust-core form factor (https://v2.tauri.app/blog/tauri-20/, 2024-10-08).

### 6.1 Option A: Rust core only (hybrid)

*Architecture: Rust engine + Python/TypeScript face via PyO3/maturin or napi-rs.*

| Dimension | Pros | Cons |
|---|---|---|
| **Performance ceiling** | Captures most of the Rust win: no GIL on the hot path, no GC pauses; pydantic-core delivered a vendor-claimed 4–50× over v1, independently measured at 5× drop-in / ~14× tuned (https://github.com/prrao87/pydantic-benchmarks) | Ceiling is bounded by boundary design; a chatty FFI can erase the gains |
| **Time-to-market** | Fastest path: host SDK ships first; the Rust core arrives incrementally behind the existing API | Two languages, two toolchains, two CI paths to maintain |
| **Hiring / talent pool** | Rust work concentrates in a small, senior-reviewable surface; most contributors stay in Python/TS — directly mitigating the 42–50% hiring/learning-curve concern in the U. Maryland adoption study (https://www.cs.umd.edu/~mwh/papers/rust-adoption.pdf) | Still needs a small group fluent in both Rust and FFI |
| **Ecosystem / libraries** | Full PyPI/npm access from the face layer; the core calls proven provider SDKs instead of reimplementing them | The Rust core draws on a younger ecosystem (fewer examples, thinner docs) |
| **FFI overhead & boundary design** | ~25 ns per crossing is negligible *if* the boundary is coarse — one call = one unit of work: compile a graph, run a super-step, write a checkpoint (https://github.com/mjbommar/kernel-lore-mcp/blob/main/docs/standards/rust/ffi.md) | Discipline is mandatory: batching, typed `#[pyclass]` returns over dicts, zero-copy for bulk data, GIL release, no panics across FFI. Per-node or per-token callbacks into Python would be lethal |
| **Distribution form factor** | Native wheels via maturin / npm packages via napi-rs; installs like any other package | Not a single binary; requires a compatible host runtime |
| **Migration risk** | Low: strangler-fig migration behind the existing API with shadow-mode verification; pydantic v2 is the canonical completed case (~6–12 months of ecosystem pain, then judged "overwhelmingly" worth it) (https://www.socratopia.app/library/python-programming-ai-era-en/chapter-8) | A core rewrite can still force breaking host-layer API changes, as pydantic v2 did — plan the migration path |
| **Free-threaded Python nuance** | Python 3.13/3.14 free-threading weakens "FFI for parallelism" but not "FFI for speed": removing the GIL does not shrink a ~27× compute gap to 1× (https://blog.serghei.pl/posts/a-quick-dive-into-ffi-in-python/, 2026-07-24) | Extensions must be audited for GIL-dependent thread-safety assumptions under free-threaded builds |

### 6.2 Option B: Whole engine in Rust

| Dimension | Pros | Cons |
|---|---|---|
| **Performance ceiling** | Highest: no boundary tax, no host-runtime overhead; Rust agent frameworks report ~5× lower peak memory and ~18–32× faster cold starts vs. Python frameworks (vendor-published, directional — independent reproduction is thin) (https://zylos.ai/research/2026-04-01-rust-native-ai-agent-frameworks-ecosystem-2026/) | For I/O-bound agent workloads the theoretical ceiling is mostly unreachable — the bottleneck is waiting on LLM APIs, not computing |
| **Time-to-market** | — | Slower: the full API surface must be designed in Rust; edit-compile-run iteration loses to an interpreted loop (https://is.muni.cz/th/tmmii/thesis.pdf). AI-assisted development is compressing this, but that is 2025–2026 commentary, not longitudinal measurement |
| **Hiring / talent pool** | Rust attracts enthusiastic contributors — delta-rs credits "people *wanting* to write Rust"; Apache DataFusion had more monthly commit authors than the more mature Spark | Fewer than a third of Rust's ~4M users call it their primary language (https://www.i-programmer.info/news/245-view-point/18925-rust-on-the-rise-python-in-decline.html, 2026-06-10); full Rust means committing to Rust-native hiring against the 42–50% concern data |
| **Ecosystem / libraries** | Rust AI infrastructure is maturing fast (Rig, async-openai, candle, burn) | The application-layer ecosystem is thin; Python still owns the AI/ML library landscape in 2026 (https://rustify.rs/articles/rust-vs-python-in-2026) |
| **FFI overhead & boundary design** | None — no boundary to defend | The boundary reappears at provider/tool layers, and if Python users later want bindings, the FFI work arrives anyway as a bolt-on |
| **Distribution form factor** | The decisive advantage: single static binary, megabyte-scale, near-zero startup, WASM/edge/embedded targets with no host runtime at all | Binary distribution forfeits "import and go" PyPI/npm ergonomics |
| **Migration risk** | A clean-sheet design avoids migration entirely *if* starting fresh | For an existing engine, big-bang rewrites have a notorious failure record (Netscape: 3 years, market share lost); the strangler pattern is the proven alternative (https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig) |
| **Ecosystem gravity** | — | Deno's lesson: even a technically superior full-Rust platform had to add npm compatibility in Deno 2.0 because losing 1.4M packages was an adoption blocker. Rust core ≠ escape from ecosystem compatibility obligations (https://choubey.gitbook.io/internals-of-deno/introduction/about) |

### 6.3 Verdict

**Hybrid core-first is the default right answer.** The deciding variables are where the users live, how chatty the boundary would be, hiring, and form factor — and for an agentic engine whose users live in Python and TypeScript, the hybrid wins on every axis except absolute ceiling. Full Rust is justified when the *form factor demands it*: single-binary distribution, edge/WASM/embedded targets, cold-start-sensitive ephemeral execution, or a homogeneous systems workload. The correct posture: build the core in Rust, ship Python-first bindings, and keep the door open to full-Rust single-binary and WASM targets.

---

## 7. The Hybrid Reference Architecture

The reference architecture layers host SDKs over a Rust core over provider adapters, with the FFI boundary drawn at coarse units of work:

```
┌─────────────────────────────────────────────────────────────┐
│  Host SDKs (the adoption surface)                            │
│  ┌──────────────────────┐   ┌─────────────────────────────┐  │
│  │  Python SDK          │   │  TypeScript SDK             │  │
│  │  (pip install        │   │  (npm package)              │  │
│  │   agentgraph)        │   │                             │  │
│  └──────────┬───────────┘   └──────────────┬──────────────┘  │
├─────────────┼──────────────────────────────┼─────────────────┤
│  FFI BOUNDARY (~25 ns/crossing; coarse calls only)           │
│  ┌──────────▼───────────┐   ┌──────────────▼──────────────┐  │
│  │  PyO3 + maturin      │   │  napi-rs                    │  │
│  │  (typed pyclass,     │   │  (typed objects,            │  │
│  │   GIL release,       │   │   async tasks)              │  │
│  │   zero-copy buffers) │   │                             │  │
│  └──────────┬───────────┘   └──────────────┬──────────────┘  │
├─────────────┴──────────────────────────────┴─────────────────┤
│  agentgraph core (pure Rust, tokio)                          │
│  ┌─────────────┐ ┌──────────────┐ ┌───────────────────────┐  │
│  │ Graph       │ │ Super-step   │ │ Checkpoint store      │  │
│  │ runtime     │ │ executor     │ │ (versioned snapshots; │  │
│  │ (channels,  │ │ (plan →      │ │  in-memory / SQLite / │  │
│  │  reducers,  │ │  parallel    │ │  Postgres via sqlx)   │  │
│  │  routing)   │ │  compute →   │ ├───────────────────────┤  │
│  │             │ │  barrier →   │ │ Tool bus              │  │
│  │             │ │  reduce)     │ │ (typed tool schemas,  │  │
│  │             │ │              │ │  cancellation tokens) │  │
│  └─────────────┘ └──────────────┘ └───────────────────────┘  │
├──────────────────────────────────────────────────────────────┤
│  Provider adapters (thin traits; no reimplemented clients)   │
│  ┌──────────────┐ ┌──────────────────┐ ┌──────────────────┐  │
│  │ Rig adapter  │ │ async-openai     │ │ genai adapter    │  │
│  └──────┬───────┘ └────────┬─────────┘ └────────┬─────────┘  │
├─────────┼──────────────────┼────────────────────┼────────────┤
│  LLM APIs (OpenAI, Anthropic, Gemini, local via candle/…)    │
└─────────┴──────────────────┴────────────────────┴────────────┘
```

**FFI boundary design rules** (convergent across the PyO3 performance guide and precedent projects; https://pyo3.rs/main/performance, https://krun.pro/rust-python/, 2026-02-27):

1. **Batch across the boundary.** One FFI call = one meaningful unit of work — compile a graph, run a super-step, write a checkpoint. Never per-node, per-token, or per-record. pydantic-core compiles the whole schema into a Rust validation plan once at class-definition time for exactly this reason.
2. **Return typed `#[pyclass]` objects, not dicts** — dict returns lose type information and breed "where did this field come from" bugs.
3. **Zero-copy for bulk data** — buffer protocols / Arrow-style sharing (the numpy-crate and pyo3-polars pattern), not JSON on hot paths.
4. **Release the GIL for long-running Rust work** via `py.allow_threads` / `Python::detach`, so a super-step does not block the host interpreter.
5. **Never let a Rust panic cross the FFI boundary** — that is undefined behavior. `catch_unwind` or `PyResult` everywhere at the seam.
6. **Treat serde/JSON as a tax, not a protocol** — serialize once per batch, not per item.

---

## 8. Introducing agentgraph

**agentgraph** is the open-source project accompanying this whitepaper: *the checkpointable, human-in-the-loop agent graph runtime for Rust — LangGraph's execution model, rebuilt on tokio, with Rust's safety and single-binary deployment.*

### 8.1 Feature set: the LangGraph quartet, plus

LangGraph's essence is a small kernel: typed channels with reducers, a Pregel/BSP super-step scheduler, a versioned checkpoint log keyed by thread, and an interrupt/resume protocol (https://docs.langchain.com/oss/python/langgraph/graph-api). agentgraph implements that kernel as first-class primitives:

- **Typed state graphs with channels and reducers.** Python's `TypedDict` + `Annotated[..., reducer]` becomes typed state structs with per-field reducer traits — converting runtime `InvalidUpdateError`-class bugs into compile-time errors.
- **A super-step executor on tokio.** Each super-step is a barriered batch of node futures over an immutable state snapshot (plan → parallel compute → barrier → reduce/route). Rust's ownership model statically guarantees the snapshot isolation Python achieves only by convention.
- **Versioned checkpointers.** A `Checkpointer` trait with in-memory and SQLite implementations at launch and Postgres (via `sqlx`) on the roadmap; `serde`-versioned snapshots written at super-step barriers, with compact binary encoding to attack the measured 85% storage-bloat / 37.8% token-overhead problem reported against Python LangGraph (https://github.com/langchain-ai/langgraph/issues/7714).
- **Interrupts, HITL, and `Command` resume.** Interruption is an explicit `Suspend(payload)` / `Resume(value)` state in the execution state machine, persisted to a checkpoint — not an in-memory pause — with idempotency documented as a node contract.
- **`Send`-API dynamic fan-out** for data-dependent map-reduce patterns not knowable at graph-build time.
- **Streaming `GraphEvent`s.** A single typed event stream over `tokio::sync::broadcast`, with mode-based filtering at the subscriber, reproducing LangGraph's seven stream modes as views over one stream.
- **A prebuilt ReAct agent** — agent node + tool node + `should_continue` edge over a messages channel — the parity benchmark against LangGraph's `create_react_agent`.

### 8.2 Positioning in the Rust ecosystem

The Rust AI stack has a donut shape: strong provider clients (`async-openai` ~6.8M crates.io downloads; `rig-core` ~1.9M) and strong local inference (candle, burn, mistral.rs), but a hollow middle where durable orchestration should be (GitHub/crates.io APIs, verified 2026-07-31; https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/). agentgraph occupies that middle:

- **Rig is a partner, not a competitor.** Rig is the de-facto provider-abstraction layer; it has no durable execution state, checkpointing, or HITL interrupts. agentgraph defines provider-agnostic traits (`ChatModel`, `Embedder`, `Tool`) and ships thin adapters for Rig, async-openai, and genai — the pairing graph-flow already demonstrates (https://ai.gopubby.com/graphflow-rust-native-orchestration-for-multi-agent-workflows-6143a9b767ad).
- **llm-chain and langchain-rust are stalled** (last releases 2023-11 and 2024-10); linear chains without durable state failed to hold users. The lesson: own one hard problem — durable, interruptible graph execution — rather than chase feature parity.
- **graph-flow / rs-graph-llm** (357 stars) is the most credible existing attempt but is young, single-maintainer, and lacks a LangGraph-grade checkpoint/versioning model; **adk-rust** implements Google's ADK composition model, which is not a general state graph with checkpointing.

No Rust crate today ships the LangGraph quartet — state graph + durable checkpointing + HITL interrupts + resumable execution — as mature, first-class primitives.

### 8.3 License and roadmap

agentgraph is dual-licensed **Apache-2.0 OR MIT**, the enterprise-safe norm in this ecosystem (Apache-2.0: candle, burn, rust-genai; MIT: rig, async-openai). The public roadmap:

1. **PyO3/maturin Python bindings** (the growth engine, per the delta-rs lesson) behind a feature flag.
2. **Postgres checkpointer** via `sqlx` for production durability.
3. **MCP and A2A interop** — meeting the ecosystem where it is.
4. **OpenTelemetry tracing** aligned with the GenAI semantic conventions Rig already uses.
5. **WASM target** for edge deployment — the full-Rust form-factor play justified in Section 6.

---

## 9. Risks & Mitigations

An honest assessment, in descending order of severity:

**Rust talent scarcity.** The U. Maryland adoption study found 50% of interviewed companies cited the learning curve as a top concern and 42% worried about hiring Rust developers; fewer than a third of Rust's ~4M users call it their primary language (https://www.cs.umd.edu/~mwh/papers/rust-adoption.pdf; https://www.i-programmer.info/news/245-view-point/18925-rust-on-the-rise-python-in-decline.html, 2026-06-10). *Mitigation:* keep the Rust surface small and senior-reviewable; let the Python/TS face absorb most contributions; cultivate the delta-rs contributor funnel (Python user → binding tinkerer → core contributor). AI-assisted development helps but is no substitute for senior Rust review.

**Async Rust learning curve.** Cancel-safe async (dropping futures must not corrupt state), `Pin`/`Send` bounds, and tokio's work-stealing semantics are genuinely hard for newcomers. *Mitigation:* hide async complexity behind the graph API — node authors write simple async functions while the executor owns the `JoinSet`, cancellation-token trees, and backpressure. Document the executor's invariants rather than asking users to rediscover them.

**Ecosystem churn.** Rig's own README warns "here be dragons" about breaking changes; first-generation ports (llm-chain, langchain-rust) have already died. *Mitigation:* depend on provider clients only behind thin adapter traits, so a breaking Rig release touches one adapter crate, not the core; keep the core's dependency tree minimal (tokio, serde, petgraph-class essentials); gate upgrades in CI with a compatibility matrix.

**Benchmark honesty.** The most-cited Rust-vs-Python agentic numbers (~5× memory, ~13× throughput, ~18–32× cold start) are framework-published and lack broad independent reproduction; the Turbopack-vs-Vite dispute shows how vendor claims get challenged on methodology (https://www.cnblogs.com/xgqfrms/p/16858655.html). *Mitigation:* agentgraph ships its benchmark harness in-repo, publishes methodology with every number, labels vendor-sourced figures as directional, and invites third-party reproduction. Credibility compounds; inflated claims do not.

---

## 10. Conclusion

The agentic core engine is becoming infrastructure, and infrastructure rewards what Rust was built for: memory safety without a garbage collector, concurrency without a GIL, compile-time correctness, and self-contained distribution. The question was never *whether* Rust belongs in the agentic stack — the provider, inference, and retrieval layers already answered that — but *where* and *how much*.

The evidence answers clearly. Adopt the hybrid: a Rust core that owns the hard, hot, correctness-critical kernel — typed state graphs, super-step execution, versioned checkpointing, interruptible HITL — behind a coarse FFI boundary and a Python-first face that meets developers where they live. Reserve full Rust for the form factors that demand it; migrate by strangulation, not big-bang; defend the FFI boundary with batching, typed objects, zero-copy, GIL release, and a no-panics rule; and publish benchmarks with methodology attached — in this ecosystem, credibility is the scarcest resource.

agentgraph exists to prove the thesis in code: LangGraph's durable-execution substrate, rebuilt on tokio, filling the verified hole in the middle of the Rust AI stack — open-sourced under Apache-2.0/MIT so the ecosystem can build on it. The donut gets its middle.
