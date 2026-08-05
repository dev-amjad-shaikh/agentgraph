# Research Brief: Production Evidence for Rust in Infrastructure & AI Systems

**Prepared:** 2026-07-31 · **Purpose:** Evidence base for the whitepaper "Rust for the Agentic Core Engine" and the `agentgraph` open-source project
**Method:** Web research; primary engineering blogs and government guidance were preferred and fetched directly where possible. Figures are quoted only with an inline source. Secondary-blog figures that could not be confirmed against a primary source are flagged.

---

## 1. Documented Case Studies

### 1.1 Discord — Read States service (Go → Rust, 2020)

Discord's Read States service tracks which channels/messages each user has read — billions of read states, an in-memory LRU cache with **tens of millions of entries per server**, hundreds of thousands of cache updates per second, and tens of thousands of Cassandra writes per second. The Go implementation suffered **latency and CPU spikes roughly every 2 minutes**, traced to Go's forced garbage collection, whose cost was proportional to the *live* heap (scanning the whole LRU cache), not to garbage volume — so GC tuning and cache repartitioning could only trade spike size against cache hit rate, never eliminate the problem. The Rust rewrite (deterministic deallocation via ownership/Drop) **removed the spikes entirely**, and with only basic optimization "beat Go on every single performance metric: latency, CPU, and memory." Freed from GC, Discord **raised cache capacity to 8 million read states**, after which "the average time is now measured in microseconds." Discord also reported free CPU gains simply from upgrading tokio 0.1 → 0.2. ([Discord Engineering — "Why Discord is switching from Go to Rust", Feb 2020](https://discord.com/blog/why-discord-is-switching-from-go-to-rust))

> **Caution for the whitepaper:** widely circulated secondary figures ("p99 from 95 ms → 5 ms", "memory 70 GB → 9 GB", or "1 GB → 128 MB") appear in 2026-era recap blogs ([Rustify, May 2026](https://rustify.rs/articles/should-you-rewrite-it-in-rust-2026); [BirJob, May 2026](https://www.birjob.com/blog/rust-at-scale-2026)) but **do not appear in Discord's original post**. The primary source supports: ~2-minute spike cadence, elimination of spikes, wins on latency/CPU/memory, and 8M-entry cache with microsecond averages. Cite only those.

### 1.2 Cloudflare — Pingora (replacing NGINX, 2022; open-sourced 2024)

Cloudflare replaced its NGINX-based proxy with **Pingora, an in-house Rust proxy serving over 1 trillion requests per day while using roughly one-third of the CPU and memory of the previous infrastructure**. Key quantified results from Cloudflare's own blog:

- **~70% less CPU and ~67% less memory** at identical traffic load vs. the old NGINX/Lua service.
- **5 ms lower median TTFB and 80 ms lower p95 TTFB**, driven by a shared, cross-thread connection pool.
- Only **1/3 as many new connections per second** overall; for one major customer, connection reuse rose from 87.1% to **99.92% — 160× fewer new connections**, saving "434 years of handshake time every day."
- Safety: "Since Pingora's inception we've served a few hundred trillion requests and **have yet to crash due to our service code**."
- Architecture: multithreaded + work-stealing scheduling on **Tokio** (chosen explicitly over NGINX's per-process model); Rust chosen because "it can do what C can do in a memory safe way without compromising performance."

([Cloudflare Blog — "How we built Pingora", Sep 2022](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/)). Pingora was open-sourced under Apache 2.0 in Feb 2024 after handling "nearly a quadrillion Internet requests" ([It's FOSS, Feb 2024](https://itsfoss.com/news/cloudflare-pingora/)).

### 1.3 AWS Firecracker — Rust microVM substrate for Lambda & Fargate

Firecracker is an **open-source VMM written in Rust** (Apache 2.0), purpose-built by AWS for secure multi-tenant serverless; it "runs production workloads within AWS" and powers **AWS Lambda and AWS Fargate**. Its minimalist design (tiny device model, seccomp filters, jailer process) reduces memory footprint and attack surface. ([Firecracker GitHub README](https://github.com/firecracker-microvm/firecracker)). AWS's original announcement stated design targets of **microVM startup under 125 ms and < 5 MiB memory overhead per microVM** ([AWS News Blog, Nov 2018](https://aws.amazon.com/blogs/aws/firecracker-lightweight-virtualization-for-serverless-computing/)).

**Agentic-AI relevance (2026):** AWS launched **Lambda MicroVMs** in 2026 — Firecracker VMs with hardware-enforced isolation explicitly positioned for **running untrusted, AI-agent-generated code**, extending execution up to 8 hours with suspend/resume ([Digital Today, Jun 2026](https://www.digitaltoday.co.kr/en/view/75153/aws-launches-lambda-microvm-supports-running-isolated-containers-for-up-to-8-hours); [Serverless Framework blog, Jul 2026](https://www.serverless.com/blog/aws-lambda-microvms-sandboxes)).

### 1.4 Deno — Rust at the heart of a JavaScript/TypeScript runtime

Deno's core (module graph, runtime execution, ops, security sandbox) is implemented in Rust on top of Tokio and V8 via the **`rusty_v8` bindings**, which Deno declared stable and production-ready in Sep 2024 for "building high-performance JavaScript and WebAssembly runtimes in Rust" ([Deno blog, Sep 2024](https://deno.com/blog/rusty-v8-stabilized)). The runtime is itself embeddable as Rust crates (`deno_core`, `deno_runtime`) ([denoland/deno discussion](https://github.com/denoland/deno/discussions/28443)).

### 1.5 Vector (Datadog) — observability pipeline

Vector, originally built by Timber (acquired by Datadog in 2021), is a **Rust-based, single-binary (~10 MB) observability pipeline** claiming "**up to 10× faster than every alternative in the space**" (Logstash, Fluentd, Fluent Bit) ([Vector GitHub README](https://github.com/vectordotdev/vector)). It powers Datadog's Observability Pipelines product and ingests **trillions of data points per day from millions of customer hosts** ([7wData company profile, 2026](https://7wdata.be/company/vector/)). Independent practitioner write-ups report typical memory footprints of **30–100 MB in operation — far below JVM-based Logstash** — with no GC pause jitter ([Jacar, Jul 2026](https://jacar.es/en/vector-a-log-agent-worth-trying/); [PipeCode](https://pipecode.ai/blogs/vector-datadog-rust-log-metric-pipelines)).

### 1.6 Polars — DataFrames

Polars is a **Rust-based, multithreaded columnar DataFrame library** (exposed to Python). Reported wins: **10–30× faster than pandas reading large CSVs** ([Python in Plain English, Oct 2025](https://python.plainenglish.io/pandas-vs-polars-in-2025-should-you-finally-make-the-switch-90fb2756ffe1)) and **3–10× on large ETL workloads** ([Shuttle, Sep 2025](https://www.shuttle.dev/blog/2025/09/24/pandas-vs-polars)); Databricks describes it as "dramatically faster than traditional single-threaded DataFrame libraries like pandas" ([Databricks blog, Jan 2026](https://www.databricks.com/blog/polars-vs-pandas)). (Vendor/community benchmarks; treat exact multiples as workload-dependent.)

### 1.7 Ruff & uv (Astral) — Python tooling in Rust

- **uv** (package/project manager): official docs state **10–100× faster than pip**; replaces pip, pip-tools, pipx, poetry, pyenv, twine, virtualenv ([Astral docs](https://docs.astral.sh/uv/); [astral-sh/uv README](https://github.com/astral-sh/uv)).
- **Ruff** (linter/formatter): widely reported **~100× faster than flake8** (e.g., 30 s → 0.3 s on a large codebase) ([youngju.dev, Mar 2026](https://www.youngju.dev/blog/ai/2026-03-17-rust-for-ai-systems-guide.en)); replaces flake8 + black + isort ([Ali Jabbary, Jun 2026](https://alijabbary.com/blog/ruff-ty-astral-toolchain)).

### 1.8 pydantic-core — Rust inside the Python ecosystem's validation hot path

Pydantic v2 (2023) moved **all validation logic into `pydantic-core`, written in Rust via PyO3**, announced by the maintainers themselves ([pydantic.dev, Apr 2023](https://pydantic.dev/articles/pydantic-v2-alpha)). Reported speedups: **5–50× over v1** overall (validation ~5×, instantiation ~17×, serialization ~10×), plus a Rust JSON parser (`jiter`) 2–5× faster than `json.loads` + validate ([IBM MCP Context Forge docs, 2025](https://github.com/IBM/mcp-context-forge/blob/main/docs/docs/manage/scale.md); [OneUptime, Jan 2026](https://oneuptime.com/blog/post/2026-01-21-python-pydantic-v2-validation/view); [SuperJSON FAQ, Aug 2025](https://superjson.ai/blog/2025-08-24-json-schema-validation-python-pydantic-guide/)). Notable precedent for `agentgraph`: Rust cores wrapped for Python consumption.

### 1.9 AI-native databases in Rust: Qdrant, LanceDB, Meilisearch, Turso/libSQL

- **Qdrant** — open-source **vector database written in Rust, designed specifically for AI applications and semantic search**; Rust-optimized HNSW with quantization for memory reduction ([Meilisearch comparison docs, 2026](https://meilisearch.com/docs/resources/comparisons/qdrant)).
- **LanceDB** — serverless, embedded **multimodal vector database built from the ground up in Rust** ("SQLite for vector DBs"), cloud-native index design ([Conf42 talk by LanceDB's Lei Xu, Aug 2023](https://www.conf42.com/Rustlang_2023_Lei_Xu_lancedb_oss_serverless_vector_db); [The Data Quarry, Jun 2023](https://thedataquarry.com/blog/vector-db-1)).
- **Meilisearch** — search engine whose **engine is 100% Rust**; team cites speed and memory safety as the deciding factors, noting that for search-engine performance "you either go with C++, Rust, or maybe Go" ([Serokell interview with Meilisearch, Apr 2021](https://serokell.io/blog/rust-in-production-meilisearch)).
- **Turso / libSQL** — libSQL (open fork of SQLite with replication and vector search, 12k+ stars) powers the Turso edge database platform; in **Dec 2024 Turso announced Limbo/Turso DB, a complete rewrite of SQLite in Rust** with async I/O (`io_uring`), MVCC-based concurrent writes, native encryption, CDC, and vector search ([Turso blog, Dec 2024](https://turso.tech/blog/introducing-limbo-a-complete-rewrite-of-sqlite-in-rust); [The New Stack, Oct 2025](https://thenewstack.io/why-we-created-turso-a-rust-based-rewrite-of-sqlite/)).

### 1.10 Tauri — Rust-core desktop application framework

Tauri builds **small, fast binaries for all major desktop platforms with the application core/backend in Rust** (system webview instead of bundled Chromium), making it the Rust-native alternative to Electron ([tauri-cc README summarizing official positioning](https://github.com/Cassielxd/tauri-cc); [official site](https://v2.tauri.app/)). Its plugin ecosystem is Rust-native (e.g., Turso/libSQL plugins with encryption held in the Rust layer) ([tauri-plugin-turso](https://github.com/readest/tauri-plugin-turso)).

---

## 2. Quantified Wins (at a glance)

| System | Metric | Reported result | Source |
|---|---|---|---|
| Discord Read States | Latency | ~2-min GC spikes eliminated; avg response in microseconds after 8M-entry cache | [Discord blog](https://discord.com/blog/why-discord-is-switching-from-go-to-rust) |
| Cloudflare Pingora | CPU / Memory | **−70% CPU, −67% memory** at same load | [Cloudflare blog](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/) |
| Cloudflare Pingora | TTFB | −5 ms median, −80 ms p95; 160× fewer new connections (one customer) | same |
| Cloudflare Pingora | Reliability | 0 service-code crashes over ~hundreds of trillions of requests | same |
| AWS Firecracker | Startup / overhead | <125 ms boot, <5 MiB per microVM (design targets) | [AWS, Nov 2018](https://aws.amazon.com/blogs/aws/firecracker-lightweight-virtualization-for-serverless-computing/) |
| Vector | Throughput | Up to 10× faster than alternatives; ~30–100 MB RAM in prod | [README](https://github.com/vectordotdev/vector); [Jacar](https://jacar.es/en/vector-a-log-agent-worth-trying/) |
| Polars | Speed | 10–30× (CSV) / 3–10× (ETL) vs pandas | [PiPE](https://python.plainenglish.io/pandas-vs-polars-in-2025-should-you-finally-make-the-switch-90fb2756ffe1); [Shuttle](https://www.shuttle.dev/blog/2025/09/24/pandas-vs-polars) |
| uv / Ruff | Speed | 10–100× vs pip; ~100× vs flake8 | [Astral docs](https://docs.astral.sh/uv/); [youngju.dev](https://www.youngju.dev/blog/ai/2026-03-17-rust-for-ai-systems-guide.en) |
| pydantic-core | Validation | 5–50× vs v1 | [pydantic.dev](https://pydantic.dev/articles/pydantic-v2-alpha); [IBM docs](https://github.com/IBM/mcp-context-forge/blob/main/docs/docs/manage/scale.md) |
| Android (Rust adoption) | Memory-safety vulns | 76% (2019) → 24% (2024) → <20% (2025); **1000× lower vuln density** vs C/C++ code | [Google Security Blog, Sep 2024](https://security.googleblog.com/2024/09/eliminating-memory-safety-vulnerabilities-Android.html); [THN, Nov 2025](https://thehackernews.com/2025/11/rust-adoption-drives-android-memory.html) |

---

## 3. Async Ecosystem Maturity & Fit for Long-Running Orchestration

- **Tokio is the de-facto async runtime**: 1.0 shipped Dec 2020 and has maintained API stability with no breaking changes since; it provides a work-stealing task scheduler and async I/O primitives ([Rustify async runtimes, Jun 2026](https://rustify.rs/articles/rust-async-runtimes-tokio-vs-async-std-2026)). It is the production runtime at **Discord, AWS, and Cloudflare** ([Generalist Programmer, 2026](https://generalistprogrammer.com/comparisons/tokio-vs-async-std)); its creator Carl Lerche is a Principal Engineer at AWS ([Heavybit podcast](https://www.heavybit.com/library/podcasts/high-leverage/ep-6-async-runtime-for-rust-with-carl-lerche-of-tokio)).
- **Cloudflare's Pingora is a direct proof point for orchestration-style workloads**: Cloudflare explicitly chose a **multithreaded, work-stealing Tokio design** so all threads share one connection pool — precisely the property an agentic engine wants for thousands of concurrent LLM/tool calls ([Cloudflare blog](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/)).
- **tower + axum** provide the service/middleware layer: axum is maintained by the Tokio team and built on Tokio, Tower, and Hyper; Tower supplies modular, reusable middleware (timeouts, retries, rate limits, tracing) — the exact cross-cutting concerns of an agent runtime ([Shuttle axum guide, 2025](https://www.shuttle.dev/blog/2023/12/06/using-axum-rust); [Rustify backend guide, Apr 2026](https://rustify.rs/articles/rust-backend-development-axum-2026)).
- **No GC = predictable tail latency for long-running processes.** Discord's case shows the failure mode of GC'd runtimes on long-lived, heap-heavy services and Rust's elimination of it ([Discord blog](https://discord.com/blog/why-discord-is-switching-from-go-to-rust)); Vector shows the same property (no GC jitter) for 24/7 data pipelines ([Jacar](https://jacar.es/en/vector-a-log-agent-worth-trying/)).
- **Ecosystem maturity consensus**: multiple 2025–2026 assessments describe Rust's async ecosystem as having "reached remarkable maturity" for high-performance concurrent systems ([Dev Genius, Sep 2025](https://blog.devgenius.io/rusts-async-ecosystem-building-scalable-apps-in-2025-7fc3ce1cca56)).
- **Honest caveat (good for whitepaper balance):** Discord had to bet on *nightly* Rust in 2019 because async was immature — a bet that paid off once async/await stabilized ([Discord blog](https://discord.com/blog/why-discord-is-switching-from-go-to-rust)). Today that risk no longer exists, but Rust retains a steeper learning curve ([Generalist Programmer](https://generalistprogrammer.com/comparisons/tokio-vs-async-std)).

---

## 4. Safety Data: Memory-Safety Evidence & Government Guidance

### Industry vulnerability statistics

- **Microsoft:** ~**70% of CVEs** over the past decade are memory-safety issues (MSRC, 2019; reiterated by Prossimo/memorysafety.org and the OpenSSF Open Source Software Security Mobilization Plan) ([memorysafety.org](https://www.memorysafety.org/docs/memory-safety/); [OpenSSF Mobilization Plan PDF](https://8112310.fs1.hubspotusercontent-na1.net/hubfs/8112310/OpenSSF/White%20House%20OSS%20Mobilization%20Plan.pdf)).
- **Google Android:** historically ~90% of vulnerabilities were memory-safety related ([OpenSSF plan](https://8112310.fs1.hubspotusercontent-na1.net/hubfs/8112310/OpenSSF/White%20House%20OSS%20Mobilization%20Plan.pdf)). After adopting Rust for new code, memory-safety vulnerabilities fell from **76% of all Android vulns in 2019 to 24% in 2024** ([Google Security Blog, Sep 2024](https://security.googleblog.com/2024/09/eliminating-memory-safety-vulnerabilities-Android.html)), then **below 20% for the first time in 2025**, with Google reporting a **1000× reduction in memory-safety vulnerability density** in Rust vs. its C/C++ code — plus productivity gains: **4× lower rollback rate, 25% less code-review time, ~20% fewer revisions** than C++ changes ([The Hacker News, Nov 2025](https://thehackernews.com/2025/11/rust-adoption-drives-android-memory.html); [LWN, Nov 2025](https://lwn.net/Articles/1046397/)).
- **Chrome:** ~70% of serious security bugs are memory-safety problems ([arXiv research roadmap citing Chromium data](https://arxiv.org/pdf/2409.17844v1)); Chromium has begun replacing PNG/JSON/font parsers with Rust implementations ([THN, Nov 2025](https://thehackernews.com/2025/11/rust-adoption-drives-android-memory.html)).
- **Apple:** 60–70% of iOS/macOS vulnerabilities are memory-safety related ([arXiv CuFuzz intro, 2026](https://arxiv.org/html/2601.01048v1)).
- **Exploited 0-days:** a 2021 Google Project Zero analysis found **67% of in-the-wild 0-days** were memory-safety issues; Google later reported 75% of exploited zero-day CVEs involve memory safety ([OpenSSF plan](https://8112310.fs1.hubspotusercontent-na1.net/hubfs/8112310/OpenSSF/White%20House%20OSS%20Mobilization%20Plan.pdf); [arXiv CuFuzz](https://arxiv.org/html/2601.01048v1)).

### Government & regulatory guidance

- **NSA (Nov 2022):** Software Memory Safety guidance recommending memory-safe languages, naming Rust among them ([summary: Rustify, 2026](https://rustify.rs/articles/rust-memory-safety-nsa-cisa-2026)).
- **White House ONCD (Feb 2024):** "Back to the Building Blocks" report urging migration to memory-safe languages; characterized C/C++ as unsuitable for new critical software ([arXiv CuFuzz citing ONCD](https://arxiv.org/html/2601.01048v1); [Rustify](https://rustify.rs/articles/rust-memory-safety-nsa-cisa-2026)).
- **CISA + NSA (Jun 24, 2025):** joint guidance **"Memory Safe Languages: Reducing Vulnerabilities in Modern Software Development"** — "the importance of memory safety cannot be overstated"; names Rust and Go; argues MSL adoption increases reliability, reduces attack surface, and decreases long-term costs ([official PDF, media.defense.gov](https://media.defense.gov/2025/Jun/23/2003742198/-1/-1/0/CSI_MEMORY_SAFE_LANGUAGES_REDUCING_VULNERABILITIES_IN_MODERN_SOFTWARE_DEVELOPMENT.PDF); [Infosecurity Magazine, Jun 2025](https://www.infosecurity-magazine.com/news/nsa-cisa-urge-memory-safe-languages/); [Industrial Cyber, Jun 2025](https://industrialcyber.co/secure-by-design/nsa-cisa-guidance-push-for-adoption-of-memory-safe-languages-in-software-development-to-boost-resilience/)).
- **DARPA TRACTOR** program funds automated C-to-Rust translation, signaling defense-sector commitment ([arXiv, 2025](https://arxiv.org/html/2510.03879v3)).

### Balancing note

Rust is not magic: Google found a near-miss RCE-class bug (CVE-2025-48530) in an `unsafe` Rust AVIF parser — but notes vulnerability density in unsafe Rust remains far below C/C++, and `unsafe` blocks do not disable the language's broader checks ([THN, Nov 2025](https://thehackernews.com/2025/11/rust-adoption-drives-android-memory.html)).

---

## 5. Key Takeaways for the Whitepaper

1. **The canonical production stories now cover every claim an agentic-core engine needs:** predictable tail latency (Discord), extreme resource efficiency at Internet scale (Cloudflare: −70% CPU, −67% memory), secure multi-tenant execution of untrusted code (Firecracker → Lambda MicroVMs for AI agents, 2026).
2. **Use primary-source numbers only.** Discord's blog supports "spikes eliminated, all metrics better, microsecond averages, 8M-entry cache" — not the viral "95 ms → 5 ms / 70 GB → 9 GB" figures, which are secondary-blog embellishments.
3. **The safety argument is now government-grade:** Microsoft 70%, Android 76% → <20%, Chrome 70%, and explicit NSA/CISA/White House guidance (2022–2025) naming Rust. For an agent engine that will execute model-generated code and hold credentials, this is a compliance-relevant differentiator.
4. **The async stack (tokio/tower/axum) is production-proven at Discord, Cloudflare, and AWS** and maps directly onto agent orchestration needs: massive concurrent I/O, middleware for retries/timeouts/rate limits, no GC pauses in long-running processes.
5. **Rust is already the substrate of the AI data plane:** Qdrant, LanceDB, Meilisearch, Turso/libSQL, Vector, Polars — vector search, embedded storage, telemetry, and dataframes are Rust-first, so an agentic core in Rust integrates natively with its most likely dependencies.
6. **The Python-interop precedent is proven** (pydantic-core, Polars, uv, Ruff: 5–100× wins): `agentgraph` can credibly offer a Rust core with Python bindings — the pattern the ecosystem already rewards.
7. **Firecracker's 2026 positioning as the sandbox for AI-agent code execution** is the single most on-point infrastructure precedent for the whitepaper's thesis: the industry is standardizing on Rust-built isolation for agentic workloads.
