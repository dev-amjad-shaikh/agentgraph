# Rust Core vs. Whole-Engine Rust: Research Brief

**Prepared for:** "Rust for the Agentic Core Engine" whitepaper + `agentgraph` (LangGraph-style agentic core in Rust)
**Date:** 2026-07-31 · Sources prioritized from 2024–2026; publication dates noted inline.
**Question:** Should a platform write only the performance-critical core in Rust (with Python/TypeScript bindings via PyO3/maturin or napi-rs), or write the whole engine/platform in Rust?

---

## 1. Executive summary

The dominant, empirically validated pattern in 2024–2026 is the **hybrid**: a Rust core exposed through thin, coarse-grained bindings to Python or TypeScript. Polars, pydantic-core, delta-rs, Ruff/uv, swc/Turbopack, and Tauri all follow it. Full-Rust platforms (Deno, uv-as-CLI, single-binary tools) win when the deliverable is itself a self-contained artifact — a runtime, CLI, edge/WASM module, or embedded component — where a host-language runtime is dead weight. The deciding variables are: (a) where the users live (Python/TS ecosystems vs. binary consumers), (b) how "chatty" the language boundary would be, (c) hiring and iteration speed, and (d) deployment form factor. For `agentgraph`, the evidence strongly supports: Rust graph-execution/checkpointing core + Python-first bindings, keeping the door open to full-Rust for edge/WASM and single-binary deployment targets.

---

## 2. Precedent hybrid projects and their lessons

### 2.1 Polars (Rust core, Python-first API)
- Polars is written entirely in Rust and exposed to Python via PyO3; the author even maintains a helper crate `pyo3-polars` (`PyDataFrame`, `PySeries`) to make boundary-crossing cheap and ergonomic. (https://crates.io/crates/pyo3-polars/0.2.1/dependencies, https://dskrzypiec.dev/polars/, 2023-04-15)
- Reported speedups vs. pandas: commonly **10–100×** on CPU-bound data work, attributed to a compiled Rust kernel (no GIL, no GC), Arrow columnar zero-copy memory, lazy query optimization, and default multithreading. (https://cloud.tencent.com/developer/article/2659182, 2026-04-23; corroborated as "10-100× faster than pandas for many operations" at https://smarttldr.com/en/topic/python-pyo3-rust-bindings/core, 2026-03-28)
- **Lesson:** users never touch Rust; the Python API *is* the product. The Rust core is replaceable infrastructure; the host-language API surface is the moat.

### 2.2 pydantic-core (pydantic v2)
- pydantic v2 (released mid-2023) moved validation/serialization into `pydantic-core`, written in Rust and bound via PyO3; model definitions remain pure Python. (https://alijabbary.com/blog/pydantic-v2-clean-data-models, 2026-06-06)
- Pydantic's own materials claim **4×–50× faster than v1.9.1**; independent benchmarks measured **5×** for a drop-in upgrade and up to **~14×** with a tuned v2 schema (Wine Reviews dataset, ~130k records), with successive v2.x releases compounding gains at the Rust level. (https://alijabbary.com/blog/pydantic-v2-clean-data-models, 2026-06-06; https://github.com/prrao87/pydantic-benchmarks, 2023-06-30, updated through pydantic 2.10)
- Migration cost was real: v2 broke v1 API conventions (validators, config, method names), and the ecosystem took ~6–12 months to restabilize — but "was it worth it? Yes, overwhelmingly." (https://www.socratopia.app/library/python-programming-ai-era-en/chapter-8)
- **Lesson:** rewriting the *hot core* (not the whole library) captured most of the win while preserving the Python developer experience; also shows a core rewrite can force breaking API changes at the host layer — plan the migration path.

### 2.3 delta-rs (Delta Lake in Rust, Python bindings)
- delta-rs is "a native Rust library for Delta Lake, with bindings into Python," published to both PyPI and crates.io. (https://github.com/delta-io/delta-rs)
- The maintainer's retrospective (2025-03) is explicit that the **Python bindings "exploded the possible user and contributor base"**, with Python users later growing into Rust contributors; Rust was chosen because it is "fast, efficient, and easily embeddable" — a portable kernel usable from Python, Node, Ruby, etc. (https://www.buoyantdata.com/blog/2025-03-09-lessons-learned-building-delta-rs.html, 2025-03-09)
- Scribd's production experience: Rust ingestion path proved "Rust is way more efficient for data ingestion workloads," while query/processing stayed in Spark — a deliberate hybrid split. (https://brokenco.de/2024/11/15/deltalake-the-definitive-guide.html, 2024-11-15; https://tech.scribd.com/blog/2021/growing-delta-ecosystem-with-rust.html, 2021-07-20)
- **Lesson:** the bindings are a *growth and community strategy*, not just an API. A Rust core with Python bindings recruits contributors from the much larger Python pool.

### 2.4 Ruff, uv, ty (Astral) — full-Rust tools *serving* a Python ecosystem
- Ruff (2022) lints Python **10–100×** faster than flake8/pylint; uv (Feb 2024) resolved/installed **8–10× faster than pip cold, 80–115× with a warm cache**, and creates virtualenvs ~80× faster than `python -m venv`. (https://astral.sh/blog/uv, 2024-02-15; corroborated at https://www.devtoolsacademy.com/blog/uv-and-ruff-turbocharging-python-development-with-rust-powered-tools, 2025-11-12, and https://raychen.uk/blog/batch-image-processor-python/, 2026-05-05)
- Independent real-world test: flake8+plugins 4m12s → ruff 2.5s (~100×) on a 250k-line Django monolith; pip cold install 2m48s → uv 26s, warm 30s → 1.8s. (https://softverdict.com/ruff-uv-by-astral-vs-alternatives-2026/, 2026-03-21)
- Key architectural detail: uv "ships as a single static binary... has no direct Python dependency," enabling installation independent of any Python version. (https://astral.sh/blog/uv, 2024-02-15)
- **Lesson:** these are *full-Rust* products, but their user-facing surface is Python's ecosystem (PyPI, requirements files, Python source). Full Rust worked because the deliverable is a standalone CLI, not an embeddable library. Single-binary distribution is a decisive product advantage.

### 2.5 Turbopack/Turborepo, swc, oxc (Rust cores for the JS toolchain)
- Vercel's Turbo (Turbopack bundler + Turborepo build system) and swc are written in Rust; swc is used by Next.js, Parcel, and Deno; oxc claims 50–100× over ESLint. (https://juejin.cn/post/7205561335554768956, 2023-03-01; https://github.com/thegdsks/awesome-modern-cli)
- Vercel claimed Turbopack up to 10× faster than Vite (and 700× vs. webpack on some benchmarks), though the Vite author publicly contested methodology — treat vendor numbers cautiously. (https://www.cnblogs.com/xgqfrms/p/16858655.html, 2022-11-05, citing turbo.build blog and the vite-vs-next-turbo discussion)
- **Lesson:** in the JS world the same hybrid pattern dominates: Rust engine underneath, npm-distributed packages on top. Vendor benchmark disputes also warn: publish methodology with numbers.

### 2.6 Tauri (Rust core + web frontend, vs. Electron)
- Tauri's core is Rust (system communication, app building, IPC); the UI is any web frontend. Tauri 2.0 (Oct 2024) extended this to mobile with plugins in Swift/Kotlin exposed to the frontend through Rust commands. (https://v2.tauri.app/blog/tauri-20/, 2024-10-08)
- The value proposition vs. Electron is precisely the Rust-core form factor: no bundled Chromium, smaller bundles, fewer resources. (https://betterprogramming.pub/tauris-use-of-javascript-and-rust-850cc6d542c8, 2022-09-13)
- **Lesson:** a *stable Rust core + replaceable frontend/plugin layer* is a deliberate end-state architecture ("define a definition of done for Tauri's core"), not a compromise.

### 2.7 Deno vs. Node (full-Rust runtime)
- Deno is a full-Rust runtime on V8 (via `rusty_v8` and `deno_core`, which maps JS Promises to Rust Futures and uses V8's Fast API for cheap JS↔Rust "ops" crossings), distributed as a single executable bundling runtime + linter + formatter + test runner. (https://deno.com/blog/open-source, 2025-10-16; https://choubey.gitbook.io/internals-of-deno/introduction/about, 2024-07-12)
- Deno's own history shows the ecosystem gravity problem: it initially rejected npm, then had to add full npm compatibility (Deno 2.0, late 2024) because losing "1.4 million npm packages" was an adoption blocker. (https://choubey.gitbook.io/internals-of-deno/introduction/about, 2024-07-12; https://cssauthor.com/best-rust-tools-for-javascript-developers/, 2026-05-02)
- **Lesson:** even a technically superior full-Rust platform must meet the incumbent ecosystem on its own turf. Rust core ≠ escape from ecosystem compatibility obligations.

### 2.8 Rust-native agent frameworks (directly relevant to `agentgraph`)
- By 2026, Rust agent frameworks (Rig, AutoAgents, OpenFANG) reached "stable APIs and substantial documentation," with reported benchmarks vs. Python frameworks: peak memory ~1.1 GB vs. ~5.1 GB (LangGraph-class), 43.7% lower latency than LangGraph, ~13× throughput vs. CrewAI (~2,400 vs. ~180 tasks/s), cold start ~180 ms vs. 3.2–5.8 s, single ~22 MB binary. (https://zylos.ai/research/2026-04-01-rust-native-ai-agent-frameworks-ecosystem-2026/, 2026-04-01 — vendor/framework-published numbers; independent reproduction still thin, treat as directional)
- GraphBit is a direct precedent for `agentgraph`'s exact shape: a graph-based agentic workflow engine with a Rust core (petgraph, tokio) and a full Python API via PyO3 with async support, benchmarking itself against LangGraph and CrewAI. (https://github.com/InfinitiBit/graphbit/blob/main/CHANGELOG.md, 2025-06/2026-05)
- Structural arguments for Rust at the agent-orchestration layer: no GIL for concurrent tool calls, cancel-safe async via `Future` + `Drop`, compile-time tool-schema safety via derive macros (eliminating malformed-schema runtime errors common in Python frameworks). (https://zylos.ai/research/2026-04-01-rust-native-ai-agent-frameworks-ecosystem-2026/, 2026-04-01)

---

## 3. FFI overhead and boundary-design best practices

### 3.1 The cost model
- Crossing the boundary is cheap per call but lethal when chatty: measured FFI round-trip overhead is on the order of **~25 ns per call** on top of the actual work — negligible for coarse operations, fatal for per-item loops. (https://github.com/mjbommar/kernel-lore-mcp/blob/main/docs/standards/rust/ffi.md)
- Compute-bound work amortizes the crossing: a recursive-Fibonacci benchmark (1M calls of fib(12)) ran **27× faster** in C via ctypes than pure Python, because the whole call tree moved into compiled code and "the Python side only pays for the initial boundary crossing per call." For I/O-bound work the gap "narrows dramatically." (https://blog.serghei.pl/posts/a-quick-dive-into-ffi-in-python/, 2026-07-24)
- Academic comparison of ctypes/cffi/PyO3 over the same Rust implementation: PyO3 had the lowest per-call overhead of the three (serial runs: PyO3 ~6.4×10³ ms vs. cffi ~7.3×10³ ms vs. ctypes ~1.98×10⁵ ms on the paper's workload); the paper recommends "specialized constructor" APIs (opaque containers passed by pointer) over per-call conversion. (https://arxiv.org/pdf/2507.00264, 2025)

### 3.2 Best practices (convergent across sources)
1. **Batch across the boundary.** Do all work in native Rust over `Vec<T>`, convert to Python objects in one pass at the end. "One FFI call" APIs (`score_many(query, Vec<String>) -> Vec<f32>`) beat "N FFI calls" (`score_one`) designs. (https://krun.pro/rust-python/, 2026-02-27; https://github.com/mjbommar/kernel-lore-mcp/blob/main/docs/standards/rust/ffi.md)
2. **Prefer typed `#[pyclass]` returns over dicts** — dict returns lose type information and breed "where did this field come from" bugs. (https://github.com/mjbommar/kernel-lore-mcp/blob/main/docs/standards/rust/ffi.md)
3. **Release the GIL for CPU-heavy Rust work** via `py.allow_threads` / `Python::detach`; PyO3's own performance guide recommends detaching for long-running Rust-only work. (https://smarttldr.com/en/topic/python-pyo3-rust-bindings/core, 2026-03-28; https://pyo3.rs/main/performance)
4. **Avoid unnecessary error conversions**: use `cast` instead of `extract` when the error is ignored — converting `PyDowncastError` to `PyErr` is "quite costly." (https://pyo3.rs/main/performance)
5. **Use zero-copy where the data is large**: the `numpy` crate shows NumPy arrays as ndarray views with zero copy; pyo3-polars passes DataFrames via Arrow. (https://stackoverflow.com/questions/71496561/how-to-exchange-polars-dataframe-between-rust-and-python, 2022-03-16)
6. **Never let a Rust panic cross the FFI boundary** (undefined behavior / segfault); use `catch_unwind` or return `PyResult`. Watch reference counting for silent leaks in long-lived processes. (https://krun.pro/rust-python/, 2026-02-27)
7. **Serde/JSON as a boundary protocol is a tax, not free**: serialize-once-per-batch, not per-item; for hot paths prefer typed conversions or Arrow/buffer protocols over JSON. (Synthesis of the batching/zero-copy guidance above; PyO3 auto-conversions documented at https://smarttldr.com/en/topic/python-pyo3-rust-bindings/core, 2026-03-28)
8. **Watch the free-threading shift**: Python 3.13 added experimental free-threading (PEP 703), 3.14 made it officially supported (PEP 779); "I need FFI because of the GIL" is weakening as a motivation, but "a 27× speed ratio does not shrink to 1× because you removed a lock." Extensions must be audited for GIL-dependent thread-safety assumptions. (https://blog.serghei.pl/posts/a-quick-dive-into-ffi-in-python/, 2026-07-24)

### 3.3 Implication for boundary design
The boundary should be drawn where **one call = one meaningful unit of work** (compile a graph, run a checkpoint step, execute a superstep) — never per-node, per-token, or per-record. Every precedent project converges on this: Polars hands whole DataFrames across; pydantic compiles the whole schema into a Rust validation plan once at class-definition time and only re-enters Python for custom validators. (https://www.socratopia.app/library/python-programming-ai-era-en/chapter-8)

---

## 4. Hiring, ecosystem, and library availability

### 4.1 Against full-Rust
- Empirical study of Rust adoption (U. Maryland): **50%** of interviewed companies cited the steep learning curve as a top concern, **42%** worried about hiring Rust developers ("we don't have a huge pool of Rust programmers"), 29% worried about productivity loss and ecosystem maturity/longevity. (https://www.cs.umd.edu/~mwh/papers/rust-adoption.pdf)
- Rust's ML/data ecosystem is measurably younger: fewer examples, thinner docs, verbose type conversions between crates, slower edit-compile-run iteration — a 2025 master's thesis implementing an ML workload in Rust documents all four as concrete costs vs. Python. (https://is.muni.cz/th/tmmii/thesis.pdf)
- Talent data (2024–2026): ~4M developers used Rust in the past year, but **fewer than a third (~709k) call it their primary language** — Rust is mostly used *alongside* Python/JS/TS "to optimize performance-critical modules," i.e., the hybrid pattern is literally the labor-market norm. Rust commands 15–25% salary premiums (UK median £90k vs. £75k for Python). (https://www.i-programmer.info/news/245-view-point/18925-rust-on-the-rise-python-in-decline.html, 2026-06-10)
- Python still dominates AI/ML libraries (PyTorch, TensorFlow, the scientific stack); Rust ML (candle, burn, tch-rs) is growing but "the practical choice for AI/ML work in 2026 is still Python... Rust is used at the infrastructure layer." (https://rustify.rs/articles/rust-vs-python-in-2026, 2026-02-18)

### 4.2 For full-Rust (or for Rust generally)
- Rust attracts contributors: delta-rs's maintainer credits Rust's appeal ("people *wanting* to write Rust code") for community growth, noting Apache DataFusion had more monthly commit authors (82) than the far-more-mature Apache Spark (70). (https://www.buoyantdata.com/blog/2025-03-09-lessons-learned-building-delta-rs.html, 2025-03-09)
- The AI-assisted-development shift is changing the effort calculus: Rust's strict compiler is "a free training signal for models"; cited examples include a 100k-line C compiler written in Rust by orchestrated agents (~$20k) and Ladybird's 25k-line JS-engine port in two weeks with zero regressions across 65k+ tests. (https://github.com/MoonshotAI/kimi-cli/issues/2264, 2026-05-13 — secondary reporting of these claims)
- Rust tooling quality (cargo, built-in tests) de-risks maintenance: delta-rs credits built-in testing for good coverage since day one, which enabled fast, safe iteration. (https://www.buoyantdata.com/blog/2025-03-09-lessons-learned-building-delta-rs.html, 2025-03-09)

### 4.3 Net read
Hiring risk scales with the *fraction* of the codebase that is Rust. A thin Rust core behind a Python API concentrates Rust work in a small, senior-reviewable surface and lets the majority of contributors stay in Python/TS — this is exactly the delta-rs contributor funnel (Python user → tinkers with bindings → contributes to Rust core). (https://www.buoyantdata.com/blog/2025-03-09-lessons-learned-building-delta-rs.html, 2025-03-09)

---

## 5. Rewrite cost and migration evidence

- **Big-bang rewrites have a notorious failure record** (Netscape Navigator: 3 years, market share lost); the strangler-fig pattern — facade/proxy routing traffic incrementally from old to new system — is the standard, lower-risk alternative, with typical timelines of 12–24 months for a mid-size monolith and first slices in 4–8 weeks. (https://scopeforged.com/blog/strangler-fig-migration-pattern, 2026-04-22; https://techdebt.guru/playbooks/strangler-fig/, 2026-02-24; https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig, 2026-06-02)
- The pattern maps naturally onto the hybrid strategy: the Rust core is introduced behind the existing Python API (the "facade"), one hot path at a time, with shadow-mode output comparison before cutover. (https://techdebt.guru/playbooks/strangler-fig/, 2026-02-24)
- pydantic v2 is the canonical completed case: full core rewrite behind a *mostly* preserved API, ~6–12 months of ecosystem migration pain, then stabilization; judged "overwhelmingly" worth it. (https://www.socratopia.app/library/python-programming-ai-era-en/chapter-8)
- Slice-selection criteria from practitioners: pick modules with low data coupling, clear API boundaries, high change frequency (ROI), and low transaction criticality first — *not* the highest-value or highest-complexity module. (https://gartsolutions.com/strangler-fig-pattern/, 2026-04-10)
- Watch-outs: the facade must not become a bottleneck or single point of failure; cross-system calls during coexistence need an anti-corruption layer. (https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig, 2026-06-02)

---

## 6. Developer-productivity comparisons

- **Iteration speed**: Python's interpreted loop beats Rust's edit-compile-run cycle for exploratory/research work; documented as a concrete cost in a Rust ML reimplementation. (https://is.muni.cz/th/tmmii/thesis.pdf)
- **Correctness-per-iteration**: Rust front-loads cost into compile time (borrow checker, types) and pays it back in fewer runtime failures — companies adopting Rust cite safety/reliability as the primary driver, with learning curve as the primary cost. (https://www.cs.umd.edu/~mwh/papers/rust-adoption.pdf)
- **The hybrid as productivity hedge**: PyO3's maintainers and community note that "many PyO3 projects involve straightforward Rust... the complex part (unsafe FFI, ABI alignment) is handled by PyO3 itself" — i.e., a thin core lowers the Rust-expertise bar. (https://smarttldr.com/en/topic/python-pyo3-rust-bindings/core, 2026-03-28)
- **AI coding assistants are compressing the Rust productivity penalty** (compiler as feedback signal), which weakens the historical "Rust is slow to write" argument — but this is 2025–2026 commentary, not longitudinal measurement. (https://github.com/MoonshotAI/kimi-cli/issues/2264, 2026-05-13)
- **Raw throughput gap**: Rust runs 10–100× faster than Python on equivalent CPU-bound workloads; a tight interpreted loop is roughly 10–100× slower than compiled code. (https://rustify.rs/articles/rust-vs-python-in-2026, 2026-02-18; https://blog.serghei.pl/posts/a-quick-dive-into-ffi-in-python/, 2026-07-24)

---

## 7. When a full-Rust engine IS justified

Convergent criteria across sources:

1. **Single-binary distribution is the product.** CLIs and agents invoked repeatedly: no Python runtime, no venv, megabyte-scale binaries, near-zero startup. (https://github.com/MoonshotAI/kimi-cli/issues/2264, 2026-05-13; https://astral.sh/blog/uv, 2024-02-15)
2. **Cold start matters (serverless/ephemeral/agent spawning).** Rust agent frameworks report ~180 ms cold starts vs. 3.2–5.8 s for Python frameworks — "structurally irreducible for Python." (https://zylos.ai/research/2026-04-01-rust-native-ai-agent-frameworks-ecosystem-2026/, 2026-04-01)
3. **Edge/WASM deployment.** Rust→WASM is a first-class, `no_std`-capable path; an IBM-cited study found Rust+WASM could improve Node.js execution speed by 1200–1500% for certain data-processing algorithms; embedded/edge targets often have no Python runtime at all. (https://www.secondstate.io/articles/deno-webassembly-rust-wasi/, 2020-08-04; e.g., `no_std` builds for embedded/WASM at https://github.com/ext-sakamoro/ALICE-Physics, 2026-02-23)
4. **The whole workload is homogeneous systems code** (runtime, database, ingestion service): delta-rs's kafka-delta-ingest ran its entire ingestion service in Rust at Scribd. (https://tech.scribd.com/blog/2021/growing-delta-ecosystem-with-rust.html, 2021-07-20)
5. **The boundary would be pathologically chatty.** If the design requires constant fine-grained crossings (per-node callbacks into Python during graph execution), the FFI tax (~25 ns/call × frequency) plus serialization can erase the Rust advantage — either redesign the boundary (batch) or move that layer into Rust too. (https://github.com/mjbommar/kernel-lore-mcp/blob/main/docs/standards/rust/ffi.md; https://krun.pro/rust-python/, 2026-02-27)
6. **Memory footprint is constrained.** ~5× peak-memory advantage reported for Rust agent frameworks vs. Python equivalents. (https://zylos.ai/research/2026-04-01-rust-native-ai-agent-frameworks-ecosystem-2026/, 2026-04-01)

And when it is NOT: the product's value is ecosystem integration (PyPI/npm), the team is small and Python-native, the workload is I/O-bound (FFI "improves throughput, not latency" — the bottleneck is waiting, not computing), or the API surface is still churning. (https://blog.serghei.pl/posts/a-quick-dive-into-ffi-in-python/, 2026-07-24; https://www.cs.umd.edu/~mwh/papers/rust-adoption.pdf)

---

## 8. Concrete examples: Python-callable Rust cores with reported speedups

1. **pydantic-core (pydantic v2, 2023)** — validation/serialization core in Rust via PyO3; official claim 4–50× over v1; independent benchmarks: 5× drop-in, up to ~14× with a tuned schema on 130k records. (https://alijabbary.com/blog/pydantic-v2-clean-data-models, 2026-06-06; https://github.com/prrao87/pydantic-benchmarks, 2023-06-30)
2. **Polars (2020– )** — DataFrame engine fully in Rust, PyO3 + Arrow zero-copy bindings; 10–100× vs. pandas on CPU-bound analytical workloads (multiple independent benchmarks). (https://cloud.tencent.com/developer/article/2659182, 2026-04-23; https://smarttldr.com/en/topic/python-pyo3-rust-bindings/core, 2026-03-28)
3. **uv (Astral, 2024)** — Python package resolver/installer as a Rust binary: 8–10× faster than pip cold, 80–115× warm-cache (vendor benchmarks, corroborated by independent tests: cold 2m48s→26s, warm 30s→1.8s). (https://astral.sh/blog/uv, 2024-02-15; https://softverdict.com/ruff-uv-by-astral-vs-alternatives-2026/, 2026-03-21)
   - *Bonus/tokenizer-class examples:* OpenAI's tiktoken and orjson are both Rust (PyO3/maturin) Python extensions; the `cryptography` package mixes cffi and PyO3 Rust primitives. (https://blog.serghei.pl/posts/a-quick-dive-into-ffi-in-python/, 2026-07-24)

---

## 9. Key takeaways for the whitepaper

1. **The hybrid is the industry default, and it is a strategy, not a compromise.** Every major success story (Polars, pydantic, delta-rs, swc/Turbopack, Tauri) is "Rust engine, host-language face." For `agentgraph`, a Rust core with Python bindings (PyO3/maturin) matches the precedent and the LangGraph-adjacent user base.
2. **The bindings are the growth engine.** delta-rs shows Python bindings multiply users and create a contributor funnel into the Rust core; the Python API surface is the moat, the Rust core is replaceable infrastructure.
3. **Boundary design determines whether the hybrid works.** Batch crossings, typed objects over dicts/JSON, zero-copy for bulk data, release the GIL, never panic across FFI. Draw the boundary at whole units of work (graph compile, superstep, checkpoint), never per-node callbacks.
4. **Full-Rust is justified by form factor, not by speed alone.** Single binary, cold start, edge/WASM/embedded, homogeneous systems workloads, or a pathologically chatty boundary. Rust-native agent frameworks (Rig, AutoAgents, OpenFANG, GraphBit) already demonstrate the full-Rust agentic engine is viable in 2026, with reported ~5× memory, ~13× throughput, ~18–32× cold-start advantages over Python frameworks (vendor numbers — flag as directional in the whitepaper).
5. **Hiring risk scales with Rust surface area.** 42–50% of companies cite hiring/learning-curve concerns; only ~1/3 of Rust users have it as a primary language. A thin core contains that risk; a full-Rust platform requires committing to Rust-native hiring or AI-assisted development leverage.
6. **If migrating an existing engine, strangle, don't rewrite.** Big-bang rewrites fail (Netscape); incremental facade-routed migration with shadow-mode verification is the proven path; pydantic v2 is the canonical successful core-rewrite case (~6–12 months of ecosystem pain, then stabilization).
7. **Free-threaded Python (3.13/3.14) weakens "FFI for parallelism" but not "FFI for speed"** — a 27× compute gap doesn't disappear when the lock does. The performance argument for a Rust agentic core survives; the pure-parallelism argument needs updating in the whitepaper.
8. **Publish benchmark methodology.** The Turbopack-vs-Vite dispute shows vendor speed claims get challenged; the whitepaper's own numbers should ship with reproducible harnesses.

---

*Source-quality note: vendor benchmarks (Astral, Vercel, OpenFANG/AutoAgents) are labeled as such above and, where possible, corroborated with at least one independent source. Framework-published agent benchmarks (section 2.8) currently lack broad independent reproduction and should be cited as directional.*
