# Rust for the Agentic Core Engine: Why Agent Orchestration Belongs in Systems Code

*2026-07-31*

*This whitepaper accompanies the open-source `agentgraph` project — a LangGraph-style agentic core engine rebuilt in Rust. Vendor-published benchmark figures are labeled as such throughout; figures without independent reproduction are treated as directional.*

---

## Table of Contents

1. Executive Summary
2. The Agentic Engine Is the New Systems Software
3. The Case for Rust
4. Production Evidence
5. Mapping Agentic Primitives to Rust Strengths
6. Core in Rust vs. Whole Engine in Rust: The Central Tradeoff
7. The Hybrid Reference Architecture
8. Introducing agentgraph
9. Risks & Mitigations
10. Conclusion

---

## 1. Executive Summary

Agent orchestration is systems software. When a platform runs tens of thousands of concurrent agent graphs — each a long-lived, stateful process that fans out LLM calls, tool invocations, and human-in-the-loop pauses over hours or days — the orchestration engine is no longer application glue. It is the scheduler, the storage layer, the concurrency substrate, and the security boundary of the entire platform. The properties that determine whether that platform scales, survives, and stays profitable are the properties of systems software: predictable tail latency, deterministic concurrency, memory safety under adversarial inputs, and low cost per unit of work.

Today, most agentic cores are written in Python. That choice made sense when agents were research demos and orchestration was a dozen asyncio tasks. At production scale, Python exacts a compounding tax: the GIL caps true parallelism on CPU-bound work, per-process memory overhead inflates infrastructure bills, garbage collection and interpreter jitter degrade tail latency, and dynamic typing defers whole classes of bugs — schema mismatches, concurrent-write conflicts, merge-semantics errors — to runtime, where they surface in production. The Python ecosystem has already conceded this point at every layer around the orchestrator: validation (pydantic-core), dataframes (Polars), packaging (uv), linting (Ruff), and vector search (Qdrant, LanceDB) are all Rust underneath. The orchestration core is the last holdout.

This whitepaper argues that Rust is the right language for the agentic core engine, and it does so on three legs. First, the workload characteristics of agent platforms — durable execution, massive concurrency, stateful checkpointing, streaming, and sandboxed execution of untrusted model-generated code — are precisely the workloads where Rust has production evidence at Internet scale, from Discord to Cloudflare to AWS. Second, the architectural primitives of the leading agent framework (LangGraph's Pregel-style kernel: actors, typed channels, a super-step scheduler, and versioned checkpoints) map one-to-one onto Rust's type system, ownership model, and tokio async runtime — with several of Python's most notorious production failure modes becoming compile-time errors instead. Third, we are building `agentgraph`, an open-source Rust implementation of that kernel, as executable proof of the thesis: a LangGraph-style agentic core that keeps the durable-execution substrate and removes the Python tax.

The claim is not that Rust is faster in the abstract. It is that the specific guarantees Rust enforces at compile time are the specific guarantees an agentic engine must deliver at runtime — and that every hour of engineering spent fighting the language to obtain them is repaid by an entire class of production incidents that can no longer occur.

---

## 2. The Agentic Engine Is the New Systems Software

The agentic core engine — the component that schedules graph nodes, merges state, persists checkpoints, and routes control flow — has a workload profile that looks nothing like a web application and everything like an operating system kernel or a database engine. Six characteristics define it.

**Long-running durable execution.** A production agent run is not a request-response. It is a session that may span minutes to hours, survive process restarts, pause for human approval, and resume from serialized state. LangGraph's checkpointing model makes this explicit: state is snapshotted at every super-step boundary, keyed by `thread_id`, and any historical checkpoint can be reloaded or forked for time travel ([LangGraph persistence docs](https://docs.langchain.com/oss/python/langgraph/persistence)). Durable execution means the engine holds live state for thousands of sessions simultaneously, for arbitrarily long periods — the exact regime where garbage-collected runtimes degrade most visibly.

**Thousands of concurrent graph executions.** A platform operator is not running one graph; it is running a fleet. Each graph's super-steps are themselves parallel: LangGraph's Pregel-inspired runtime executes all active nodes concurrently within a step, then barriers, merges, and routes ([LangGraph Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api)). The concurrency requirement is therefore multiplicative — thousands of sessions × parallel nodes per super-step × concurrent LLM and tool I/O per node. This is a scheduler problem, and schedulers are systems software.

**Stateful checkpointing as a hot path.** Checkpoints are not an occasional backup; they are written at every super-step boundary of every running graph. The serialization layer is therefore on the critical path of orchestration, and its efficiency is a direct infrastructure cost. The measured evidence is unflattering to the incumbent: a reproducible 2026 study of LangGraph checkpoint serialization found 85% storage bloat and 37.8% token overhead on a 16-turn ReAct agent, with no opt-out path ([langgraph issue #7714](https://github.com/langchain-ai/langgraph/issues/7714)). Checkpoint encoding, delta compression, and compaction are storage-engine problems.

**Streaming as a first-class output.** Agent platforms stream everything: full state snapshots, per-node deltas, LLM token chunks, custom events, task lifecycle events — LangGraph exposes seven composable stream modes ([LangGraph streaming docs](https://docs.langchain.com/oss/python/langgraph/streaming)). The engine is a fan-out event broker layered over its own execution loop, serving backpressure-sensitive subscribers continuously.

**Untrusted code execution.** Agents increasingly write and run code — generated by models, supplied by third-party tools, orchestrated across tenants. The engine and its surrounding sandbox boundary hold credentials, touch production APIs, and execute instructions no human reviewed. AWS's 2026 launch of Lambda MicroVMs — Firecracker-based hardware isolation explicitly positioned for running untrusted, AI-agent-generated code — signals that the industry now treats agent sandboxes as a security perimeter ([Serverless Framework blog, Jul 2026](https://www.serverless.com/blog/aws-lambda-microvms-sandboxes)). A memory-safety vulnerability in the engine is not a crash; it is a perimeter breach.

**Cost per token-orchestrated.** The unit economics of an agent platform are dominated by two terms: model inference (paid to the LLM provider) and orchestration overhead (paid to the infrastructure provider). Orchestration overhead is CPU, memory, and storage consumed per agent step — a number that scales linearly with every graph, every super-step, and every checkpoint. At scale, the language of the core engine is a line item.

Taken together, these characteristics describe systems software, and history has a consistent answer for what language systems software at this scale should be written in. The rest of this paper makes that case.

---

## 3. The Case for Rust

### 3.1 Performance and tail latency

Agent engines are long-lived, heap-heavy, latency-sensitive processes — the profile that exposes garbage collection at its worst. Discord's Read States service is the canonical demonstration: its Go implementation suffered latency and CPU spikes roughly every two minutes because GC cost was proportional to the *live* heap (an LRU cache of tens of millions of entries), not to garbage volume — so tuning could trade spike size against cache hit rate but never eliminate the problem. The Rust rewrite removed the spikes entirely and, with only basic optimization, "beat Go on every single performance metric: latency, CPU, and memory"; freed from GC, Discord raised cache capacity to 8 million read states, after which average response time was "measured in microseconds" ([Discord Engineering, Feb 2020](https://discord.com/blog/why-discord-is-switching-from-go-to-rust)). (Widely circulated secondary figures for this migration — "p99 95 ms → 5 ms," "70 GB → 9 GB" — do not appear in Discord's original post and should not be cited; the primary-source claims above are strong enough.)

For an agentic engine holding thousands of checkpointed session states in memory, this is not an anecdote; it is the failure mode the engine will otherwise inherit. Rust's deterministic deallocation means p99 latency is governed by the workload, not the collector.

### 3.2 Fearless concurrency: tokio and the super-step

The super-step execution model (Section 5) is a barriered batch of parallel node futures over an immutable state snapshot. This is exactly what Rust's async stack was built for. Tokio — the production async runtime at Discord, AWS, and Cloudflare ([Generalist Programmer, 2026](https://generalistprogrammer.com/comparisons/tokio-vs-async-std)) — provides a work-stealing scheduler with API stability since its 1.0 release in December 2020 ([Rustify, Jun 2026](https://rustify.rs/articles/rust-async-runtimes-tokio-vs-async-std-2026)). Cloudflare chose a multithreaded, work-stealing Tokio design for Pingora explicitly so that all threads share one connection pool — precisely the property an agentic engine wants for thousands of concurrent LLM and tool calls ([Cloudflare blog, Sep 2022](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/)).

The decisive difference from Python is that Rust's concurrency safety is static. The `Send`/`Sync` trait system makes data races a compile error; the borrow checker makes it impossible for two parallel nodes to mutably alias shared state. LangGraph's super-step isolation — "no node sees another's in-progress work" — is a convention in Python (copy-on-read) and a theorem in Rust. There is no GIL, so CPU-bound node work (serialization, embedding, local tool logic) parallelizes for real.

### 3.3 Memory safety — the compliance-grade argument

An agent engine executes model-generated code, holds tenant credentials, and brokers tool calls. Memory-safety vulnerabilities in such a component are existential. The industry data is consistent and sobering: roughly 70% of Microsoft's CVEs over the past decade are memory-safety issues ([memorysafety.org](https://www.memorysafety.org/docs/memory-safety/)), and about 70% of Chrome's serious security bugs are memory-safety problems ([arXiv roadmap citing Chromium data](https://arxiv.org/pdf/2409.17844v1)).

The Android experiment is the strongest controlled evidence that Rust changes this. After adopting Rust for new code, memory-safety vulnerabilities fell from 76% of all Android vulnerabilities in 2019 to 24% in 2024 ([Google Security Blog, Sep 2024](https://security.googleblog.com/2024/09/eliminating-memory-safety-vulnerabilities-Android.html)), then below 20% for the first time in 2025, with Google reporting a 1000× lower memory-safety vulnerability density in its Rust code versus its C/C++ code — alongside productivity gains: 4× lower rollback rate and 25% less code-review time ([The Hacker News, Nov 2025](https://thehackernews.com/2025/11/rust-adoption-drives-android-memory.html)). Honesty requires the balancing note: a near-miss RCE-class bug (CVE-2025-48530) was found in an `unsafe` Rust AVIF parser — but Google itself notes that vulnerability density in unsafe Rust remains far below C/C++, and `unsafe` blocks do not disable the language's broader checks ([The Hacker News, Nov 2025](https://thehackernews.com/2025/11/rust-adoption-drives-android-memory.html)).

This is now government-grade guidance, not vendor advocacy. The NSA recommended memory-safe languages in November 2022; the White House ONCD's "Back to the Building Blocks" report (February 2024) urged migration away from memory-unsafe languages for critical software; and in June 2025 CISA and NSA jointly published "Memory Safe Languages: Reducing Vulnerabilities in Modern Software Development," naming Rust and arguing that memory-safe adoption increases reliability, reduces attack surface, and decreases long-term cost ([CISA/NSA joint guidance, Jun 2025](https://media.defense.gov/2025/Jun/23/2003742198/-1/-1/0/CSI_MEMORY_SAFE_LANGUAGES_REDUCING_VULNERABILITIES_IN_MODERN_SOFTWARE_DEVELOPMENT.PDF)). For platform vendors selling into regulated enterprises, a memory-safe orchestration core is a procurement differentiator.

### 3.4 Deployment: single binaries and WASM

Rust compiles to a static, dependency-free binary — the entire engine, scheduler, checkpoint codecs, and HTTP/gRPC surface in one artifact. Vector ships as a ~10 MB single binary ([Vector README](https://github.com/vectordotdev/vector)); the contrast with a Python runtime plus its dependency tree (the LangChain stack's weight is a standing community complaint, [GitHub discussion #182015, Dec 2025](https://github.com/orgs/community/discussions/182015)) is operationally significant for cold starts, container images, and edge deployment. Rust is also a first-class WASM target: the same engine compiled to WASM can run sandboxed at the edge, inside a browser, or embedded in another runtime — a distribution story no Python core can match.

### 3.5 Total cost of ownership

The TCO argument is arithmetic, not ideology. Cloudflare's Pingora serves over a trillion requests per day on roughly 70% less CPU and 67% less memory than the NGINX-based service it replaced, at identical traffic load ([Cloudflare blog, Sep 2022](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/)). Discord eliminated spike-driven over-provisioning. Vector runs observability pipelines in 30–100 MB of RAM where JVM-based alternatives need multiples of that ([Jacar, Jul 2026](https://jacar.es/en/vector-a-log-agent-worth-trying/)). Orchestration overhead is a per-step, per-session cost; a 2–3× reduction in CPU and memory footprint is a direct, permanent reduction in the cost of every token the platform orchestrates — and the Android data (fewer rollbacks, less review time) suggests the maintenance side of the ledger improves as well.

---

## 4. Production Evidence

Rust's fitness for agentic-core workloads is not a projection; each required property already has a named production proof.

**Tail latency at heap scale — Discord.** As detailed in Section 3.1, Discord's Rust Read States service eliminated ~2-minute GC spikes, improved latency, CPU, and memory simultaneously, and scaled its in-memory cache to 8 million entries with microsecond average response times ([Discord Engineering](https://discord.com/blog/why-discord-is-switching-from-go-to-rust)). An agent engine's session-state store is the same shape of problem.

**Connection-dense proxying at Internet scale — Cloudflare Pingora.** Pingora serves over a trillion requests per day with ~70% less CPU and ~67% less memory than its NGINX predecessor, cut median TTFB by 5 ms and p95 TTFB by 80 ms via a shared cross-thread connection pool, and — most striking for reliability claims — has served a few hundred trillion requests without a single crash attributable to its service code ([Cloudflare blog](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/)). An agent gateway brokering LLM API calls for thousands of tenants is, architecturally, Pingora's problem.

**Sandboxed multi-tenant execution — AWS Firecracker and Lambda MicroVMs.** Firecracker, an open-source VMM written in Rust, powers AWS Lambda and Fargate, with design targets of microVM startup under 125 ms and under 5 MiB memory overhead per microVM ([AWS, Nov 2018](https://aws.amazon.com/blogs/aws/firecracker-lightweight-virtualization-for-serverless-computing/); [Firecracker README](https://github.com/firecracker-microvm/firecracker)). In 2026 AWS extended this line with Lambda MicroVMs — Firecracker isolation explicitly for untrusted, AI-agent-generated code, with executions up to 8 hours and suspend/resume ([Digital Today, Jun 2026](https://www.digitaltoday.co.kr/en/view/75153/aws-launches-lambda-microvm-supports-running-isolated-containers-for-up-to-8-hours); [Serverless Framework, Jul 2026](https://www.serverless.com/blog/aws-lambda-microvms-sandboxes)). The industry's answer to "how do we sandbox agents" is already Rust-built.

**High-throughput telemetry — Vector.** Datadog's Rust observability pipeline claims up to 10× the throughput of every alternative in its space (vendor-reported; [Vector README](https://github.com/vectordotdev/vector)) and runs 24/7 in 30–100 MB of memory with no GC jitter, per independent practitioner reports ([Jacar, Jul 2026](https://jacar.es/en/vector-a-log-agent-worth-trying/)) — the same always-on, no-pause profile an agent engine requires for its own event streaming.

**The AI data plane is already Rust.** The systems an agentic core integrates with most tightly are Rust-first: Qdrant, a vector database written in Rust and designed for AI applications ([Meilisearch comparison docs, 2026](https://meilisearch.com/docs/resources/comparisons/qdrant)); LanceDB, an embedded multimodal vector database "built from the ground up in Rust" ([Conf42, Aug 2023](https://www.conf42.com/Rustlang_2023_Lei_Xu_lancedb_oss_serverless_vector_db)); and Turso, which announced a complete rewrite of SQLite in Rust with async I/O and concurrent writes ([Turso blog, Dec 2024](https://turso.tech/blog/introducing-limbo-a-complete-rewrite-of-sqlite-in-rust)). A Rust orchestration core composes natively with its most likely dependencies — same async runtime, same serialization ecosystem, zero FFI friction.

**The hybrid precedent — Polars, Ruff, uv, pydantic-core.** The Python ecosystem has repeatedly voted for Rust cores with Python ergonomics: pydantic-core moved all validation into Rust for 5–50× speedups over v1 ([pydantic.dev, Apr 2023](https://pydantic.dev/articles/pydantic-v2-alpha); [IBM MCP Context Forge docs](https://github.com/IBM/mcp-context-forge/blob/main/docs/docs/manage/scale.md)); uv is 10–100× faster than pip (vendor-reported; [Astral docs](https://docs.astral.sh/uv/)); Ruff is widely reported at ~100× flake8 ([youngju.dev, Mar 2026](https://www.youngju.dev/blog/ai/2026-03-17-rust-for-ai-systems-guide.en)); Polars posts 10–30× over pandas on large CSV reads (community benchmarks; [PiPE, Oct 2025](https://python.plainenglish.io/pandas-vs-polars-in-2025-should-you-finally-make-the-switch-90fb2756ffe1)). This is the exact strategy `agentgraph` adopts: a Rust core with Python bindings, letting teams keep the language of their agents while removing the tax from the engine.

---

## 5. Mapping Agentic Primitives to Rust Strengths

The portability argument rests on a fact that is under-appreciated: LangGraph's kernel is small. Strip away the ergonomic surface and the runtime is four things — actors (`PregelNode`s), typed channels with reducers, a Bulk-Synchronous-Parallel super-step scheduler, and a versioned checkpoint store ([INTERNALS.md, Apr 2026](https://internals.laxmena.com/p/langgraph-internals-how-production); [Dhanave, Jul 2026](https://pratikdhanave.com/blog/posts/langgraph-01-message-passing-vs-shared-state.html)). Each maps onto a Rust strength, and in two cases the mapping *upgrades* a runtime guarantee to a compile-time one.

**Typed channels and reducers → trait-bound state structs.** In Python, a state key is a channel whose merge semantics are declared dynamically — `Annotated[list, operator.add]` — and validated at runtime. In Rust, the state schema is a struct whose fields carry their reducers in the type system: `trait Reducer<V> { fn reduce(left: &V, right: V) -> V }`, with channel kinds (`LastValue`, `BinaryOperatorAggregate`, `Topic`, `EphemeralValue`) as concrete types. The payoff is the elimination of an entire documented production bug class: concurrent writes to a `LastValue` channel raise `InvalidUpdateError: Can receive only one value per step` at runtime in LangGraph — a failure that bit both the official `deepagentsjs` research example (November 2025) and CopilotKit (fixed August 2025) when parallel branches were added to previously single-path graphs ([INTERNALS.md, Apr 2026](https://internals.laxmena.com/p/langgraph-internals-how-production)). In a Rust core, a channel that cannot accept multiple writes per super-step *cannot be written twice* — the type system refuses to compile the graph. The bug class does not get caught earlier; it ceases to exist.

**Snapshot isolation → the borrow checker.** The super-step contract is that every active node reads state as of the start of the step and no node observes another's partial writes ([LangGraph Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api)). Python achieves this by convention — copy-on-read. Rust achieves it structurally: nodes receive shared immutable borrows (`&State`) of the super-step snapshot, and the borrow checker statically forbids any mutable alias for the duration. Isolation is no longer a discipline every contributor must remember; it is a property the compiler enforces on every compile.

**The super-step scheduler → tokio JoinSet + barrier.** One super-step is: run all active nodes concurrently, wait for all of them (a transactional barrier — if any actor fails, the step's writes are discarded), then merge writes through reducers and route the next active set ([Dhanave, Jul 2026](https://pratikdhanave.com/blog/posts/langgraph-01-message-passing-vs-shared-state.html)). In Rust this is a `tokio::task::JoinSet` (or `FuturesUnordered`) of node futures over the immutable snapshot, awaited to a barrier, followed by a reduce/route phase. The work-stealing tokio scheduler — the same one Cloudflare runs a trillion requests a day on — multiplexes the LLM- and tool-call-bound node futures across cores, with no GIL anywhere in the picture. Graph cycles (the ReAct loop) are not call-stack recursion in either implementation — they are re-scheduling across super-steps — so the engine's `recursion_limit` equivalent is a super-step counter, exactly as in LangGraph.

**Versioned checkpoints → serde-versioned snapshots at barrier boundaries.** LangGraph's `Checkpoint` is a versioned snapshot (`channel_values`, `channel_versions`, `versions_seen`, metadata with `source`/`step`/`parents`) written at super-step boundaries, keyed by `thread_id`, powering durable execution, human-in-the-loop interrupts, time travel, and partial-failure resume ([INTERNALS.md](https://internals.laxmena.com/p/langgraph-internals-how-production); [LangGraph checkpointers docs](https://docs.langchain.com/oss/python/langgraph/checkpointers)). In Rust this is a `serde`-serialized, schema-versioned struct written at each barrier — plus an opportunity the Python incumbent has demonstrably left open: replacing the measured 85% checkpoint storage bloat and 37.8% token overhead ([langgraph issue #7714](https://github.com/langchain-ai/langgraph/issues/7714)) with compact binary encoding and delta checkpoints. Interrupts and resume become explicit states in a typed execution state machine — `Suspend(payload)` / `Resume(value)` — rather than control-flow exceptions.

**Streaming modes → a typed event broadcast.** LangGraph's seven stream modes (`values`, `updates`, `messages`, `custom`, `checkpoints`, `tasks`, `debug`) are all views over the same super-step event stream ([LangGraph streaming docs](https://docs.langchain.com/oss/python/langgraph/streaming)). In Rust, one `tokio::sync::broadcast` channel of typed `GraphEvent` enums, with mode-based filtering at each subscriber, reproduces all seven — with the compiler guaranteeing that every event variant is exhaustively handled.

The mapping is not approximate. Actors, channels, barriers, checkpoints, and event streams are the vocabulary of Rust's async ecosystem; LangGraph's architects independently arrived at the same vocabulary in Python and paid runtime prices for guarantees Rust gives at compile time. `agentgraph` is the experiment that closes the loop: the same kernel, re-expressed in the language its invariants were always written in.

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

---

## References

1. LangGraph persistence documentation — https://docs.langchain.com/oss/python/langgraph/persistence
2. LangGraph Graph API documentation — https://docs.langchain.com/oss/python/langgraph/graph-api
3. langgraph issue #7714 (checkpoint serialization overhead) — https://github.com/langchain-ai/langgraph/issues/7714
4. LangGraph streaming documentation — https://docs.langchain.com/oss/python/langgraph/streaming
5. Serverless Framework blog, "AWS Lambda MicroVMs sandboxes" (Jul 2026) — https://www.serverless.com/blog/aws-lambda-microvms-sandboxes
6. Discord Engineering, "Why Discord is switching from Go to Rust" (Feb 2020) — https://discord.com/blog/why-discord-is-switching-from-go-to-rust
7. Generalist Programmer, "Tokio vs async-std" (2026) — https://generalistprogrammer.com/comparisons/tokio-vs-async-std
8. Rustify, "Rust async runtimes: Tokio vs async-std 2026" (Jun 2026) — https://rustify.rs/articles/rust-async-runtimes-tokio-vs-async-std-2026
9. Cloudflare blog, "How we built Pingora" (Sep 2022) — https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/
10. memorysafety.org, "Memory safety" — https://www.memorysafety.org/docs/memory-safety/
11. arXiv roadmap citing Chromium memory-safety data — https://arxiv.org/pdf/2409.17844v1
12. Google Security Blog, "Eliminating memory safety vulnerabilities at the source" (Sep 2024) — https://security.googleblog.com/2024/09/eliminating-memory-safety-vulnerabilities-Android.html
13. The Hacker News, "Rust adoption drives Android memory safety" (Nov 2025) — https://thehackernews.com/2025/11/rust-adoption-drives-android-memory.html
14. CISA/NSA joint guidance, "Memory Safe Languages: Reducing Vulnerabilities in Modern Software Development" (Jun 2025) — https://media.defense.gov/2025/Jun/23/2003742198/-1/-1/0/CSI_MEMORY_SAFE_LANGUAGES_REDUCING_VULNERABILITIES_IN_MODERN_SOFTWARE_DEVELOPMENT.PDF
15. Vector README (Datadog) — https://github.com/vectordotdev/vector
16. GitHub community discussion #182015 (Dec 2025) — https://github.com/orgs/community/discussions/182015
17. Jacar, "Vector: a log agent worth trying" (Jul 2026) — https://jacar.es/en/vector-a-log-agent-worth-trying/
18. AWS blog, "Firecracker: lightweight virtualization for serverless computing" (Nov 2018) — https://aws.amazon.com/blogs/aws/firecracker-lightweight-virtualization-for-serverless-computing/
19. Firecracker README — https://github.com/firecracker-microvm/firecracker
20. Digital Today, "AWS launches Lambda MicroVM" (Jun 2026) — https://www.digitaltoday.co.kr/en/view/75153/aws-launches-lambda-microvm-supports-running-isolated-containers-for-up-to-8-hours
21. Meilisearch comparison docs, Qdrant (2026) — https://meilisearch.com/docs/resources/comparisons/qdrant
22. Conf42, "LanceDB: OSS serverless vector DB" (Aug 2023) — https://www.conf42.com/Rustlang_2023_Lei_Xu_lancedb_oss_serverless_vector_db
23. Turso blog, "Introducing Limbo: a complete rewrite of SQLite in Rust" (Dec 2024) — https://turso.tech/blog/introducing-limbo-a-complete-rewrite-of-sqlite-in-rust
24. pydantic.dev, "Pydantic V2 alpha" (Apr 2023) — https://pydantic.dev/articles/pydantic-v2-alpha
25. IBM MCP Context Forge docs — https://github.com/IBM/mcp-context-forge/blob/main/docs/docs/manage/scale.md
26. Astral docs, uv — https://docs.astral.sh/uv/
27. youngju.dev, "Rust for AI systems guide" (Mar 2026) — https://www.youngju.dev/blog/ai/2026-03-17-rust-for-ai-systems-guide.en
28. PiPE, "Pandas vs Polars in 2025" (Oct 2025) — https://python.plainenglish.io/pandas-vs-polars-in-2025-should-you-finally-make-the-switch-90fb2756ffe1
29. INTERNALS.md, "LangGraph internals: how production works" (Apr 2026) — https://internals.laxmena.com/p/langgraph-internals-how-production
30. Dhanave, "Message passing vs shared state" (Jul 2026) — https://pratikdhanave.com/blog/posts/langgraph-01-message-passing-vs-shared-state.html
31. LangGraph checkpointers documentation — https://docs.langchain.com/oss/python/langgraph/checkpointers
32. dskrzypiec.dev, Polars — https://dskrzypiec.dev/polars/
33. delta-rs repository — https://github.com/delta-io/delta-rs
34. Tauri blog, "Tauri 2.0" (Oct 2024) — https://v2.tauri.app/blog/tauri-20/
35. Buoyant Data, "Lessons learned building delta-rs" (Mar 2025) — https://www.buoyantdata.com/blog/2025-03-09-lessons-learned-building-delta-rs.html
36. Astral blog, uv (Feb 2024) — https://astral.sh/blog/uv
37. Deno blog, "Deno is open source" (Oct 2025) — https://deno.com/blog/open-source
38. prrao87, pydantic-benchmarks — https://github.com/prrao87/pydantic-benchmarks
39. U. Maryland, Rust adoption study — https://www.cs.umd.edu/~mwh/papers/rust-adoption.pdf
40. kernel-lore-mcp, Rust FFI standards — https://github.com/mjbommar/kernel-lore-mcp/blob/main/docs/standards/rust/ffi.md
41. Socratopia, "Python programming in the AI era," chapter 8 — https://www.socratopia.app/library/python-programming-ai-era-en/chapter-8
42. blog.serghei.pl, "A quick dive into FFI in Python" (Jul 2026) — https://blog.serghei.pl/posts/a-quick-dive-into-ffi-in-python/
43. Zylos AI research, "Rust-native AI agent frameworks ecosystem 2026" (Apr 2026) — https://zylos.ai/research/2026-04-01-rust-native-ai-agent-frameworks-ecosystem-2026/
44. Masaryk University thesis — https://is.muni.cz/th/tmmii/thesis.pdf
45. i-programmer, "Rust on the rise, Python in decline" (Jun 2026) — https://www.i-programmer.info/news/245-view-point/18925-rust-on-the-rise-python-in-decline.html
46. Rustify, "Rust vs Python in 2026" — https://rustify.rs/articles/rust-vs-python-in-2026
47. Microsoft Azure Architecture, "Strangler Fig pattern" — https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig
48. Choubey, "Internals of Deno" — https://choubey.gitbook.io/internals-of-deno/introduction/about
49. PyO3 performance guide — https://pyo3.rs/main/performance
50. krun.pro, "Rust-Python" (Feb 2026) — https://krun.pro/rust-python/
51. Zylos AI research, "Rust AI agent frameworks infrastructure" (Mar 2026) — https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/
52. ai.gopubby.com, "graph-flow: Rust-native orchestration for multi-agent workflows" — https://ai.gopubby.com/graphflow-rust-native-orchestration-for-multi-agent-workflows-6143a9b767ad
53. cnblogs, Turbopack vs Vite methodology dispute — https://www.cnblogs.com/xgqfrms/p/16858655.html
