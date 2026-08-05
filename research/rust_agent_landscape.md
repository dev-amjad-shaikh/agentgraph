# Research Brief: The Rust LLM/Agent Framework Landscape (2024–2026)

**Purpose:** Position a new open-source project, `agentgraph` — a LangGraph-style agentic core engine in Rust — against the existing ecosystem.
**Author:** Research analyst (sub-agent), for the whitepaper *"Rust for the Agentic Core Engine"*
**Date:** 2026-07-31
**Method:** Web search + direct verification of GitHub API metadata (stars, forks, last push, license) and crates.io API metadata (latest version, last publish, total downloads) on 2026-07-31. Claims are cross-checked across at least two sources where possible. Star counts are point-in-time snapshots.

---

## 1. Executive summary

The Rust AI-agent ecosystem has matured rapidly through 2025–2026, but it has stratified into clear layers with a conspicuous hole in the middle:

- **Provider/client layer (crowded, healthy):** `rig-core`, `async-openai`, `genai`, plus many single-provider clients.
- **Local inference layer (mature):** Hugging Face `candle`, `burn`, `mistral.rs`.
- **Retrieval/memory tooling (mature):** Qdrant Rust client, `fastembed-rs`, `mcp-server-qdrant`.
- **Agent orchestration layer (fragmented, immature):** `swarms-rs`, `AutoAgents`, `adk-rust`, `rs-graph-llm`/`graph-flow`, and a handful of micro-projects — **none** offers a mature, graph-based, checkpointable, human-in-the-loop orchestration core comparable to LangGraph.

The gap is real and verified: the closest existing projects are either dormant (`llm-chain`, `langchain-rust`), non-graph orchestration frameworks (`swarms-rs`, `AutoAgents`, `adk-rust`), or very young graph experiments (`graph-flow`/`rs-graph-llm`, `juncture`). An ecosystem survey published 2026-03-31 reaches the same conclusion, noting that "LangChain, LlamaIndex, CrewAI, AutoGen have no true Rust equivalents" and that the orchestration layer is the least developed part of the stack. [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)

---

## 2. Framework-by-framework survey

### 2.1 Rig (`0xPlaygrounds/rig`, crate `rig-core`)

- **What it does:** The de-facto flagship Rust LLM *application* framework. Unified trait-based interface over 20+ model providers, 10+ vector-store integrations, agentic workflows with multi-turn streaming, and OpenTelemetry observability. Monorepo: lean `rig-core` plus side crates (e.g., `rig-mongodb`). [GitHub README](https://github.com/0xPlaygrounds/rig), [CONTRIBUTING.md](https://github.com/0xPlaygrounds/rig/blob/main/CONTRIBUTING.md)
- **Maturity:** ~8,113 stars, 913 forks, last push 2026-07-31 (same-day activity), MIT license (GitHub API, 2026-07-31). `rig-core` v0.41.0 published 2026-07-28, ~1.92M total crates.io downloads (crates.io API, 2026-07-31). An independent analytics snapshot put it at ~7,141 stars, consistent with rapid growth. [OSSInsight](https://ossinsight.io/analyze/0xPlaygrounds/rig)
- **Adoption:** Production users include Dria, Neon, Nethermind (NINE multi-agent simulation), Linera Protocol, and St. Jude. [BrightCoding overview](https://www.blog.brightcoding.dev/2025/09/28/building-modular-llm-powered-apps-with-rig-a-rust-framework-overview), [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)
- **What's missing:** Rig is a *provider abstraction + agent builder*, not a graph orchestrator. It has no durable execution state, no checkpointing/resumption, no human-in-the-loop interrupts, and no conditional graph routing as first-class primitives — state machines exist only as community examples. [rig-agent-state-machine-example](https://github.com/0xPlaygrounds/rig-agent-state-machine-example) The README itself warns of ongoing breaking changes ("Here be dragons"). [GitHub README](https://github.com/0xPlaygrounds/rig)
- **Implication for `agentgraph`:** Rig is the natural *complement*, not competitor — `graph-flow` already demonstrates the "graph engine + Rig for LLM calls" pairing. [GraphFlow article](https://ai.gopubby.com/graphflow-rust-native-orchestration-for-multi-agent-workflows-6143a9b767ad)

### 2.2 llm-chain (`sobelio/llm-chain`)

- **What it does:** Early (2023) "toolbox" of crates for chaining LLM calls, prompts, and RAG-style steps — a LangChain-inspired linear-chain model. [Shuttle guide](https://www.shuttle.dev/blog/2024/06/06/llm-chain-langchain-rust)
- **Maturity:** 1,605 stars but effectively **dormant**: last crates.io release v0.13.0 on 2023-11-15; last GitHub push 2024-10-31 (GitHub API / crates.io API, verified 2026-07-31).
- **What's missing:** Maintenance. No graph model, no checkpointing, no HITL; pre-dates the modern tool-calling/streaming agent paradigm. Evidence that first-generation Rust "LangChain ports" did not achieve durable traction.

### 2.3 async-openai (`64bit/async-openai`)

- **What it does:** Fully async, strongly typed OpenAI API client (low- and high-level APIs); the ecosystem's default OpenAI plumbing, wrapped by other SDKs (e.g., Tinfoil's Rust SDK is "a thin wrapper around the `async-openai` crate"). [Tinfoil docs](https://docs.tinfoil.sh/sdk/rust-sdk)
- **Maturity:** 1,980 stars, last push 2026-07-31, v0.41.3 published same day, ~6.8M total crates.io downloads — by far the most-downloaded crate in this survey (GitHub/crates.io APIs, 2026-07-31).
- **What's missing:** Single-provider, no agent abstractions at all. It's infrastructure, not orchestration — but its dominance confirms demand for OpenAI-compatible plumbing that `agentgraph` should interoperate with rather than reimplement.

### 2.4 genai (`jeremychone/rust-genai`)

- **What it does:** Ergonomic multi-provider chat client — single API over OpenAI, Anthropic, Gemini, Ollama, Groq, xAI/Grok, Cohere, DeepSeek; "200+ LLM models, 26+ providers out of the box," with streaming and tool calling. [GitHub](https://github.com/jeremychone/rust-genai)
- **Maturity:** 847 stars, very active (last push 2026-07-31), Apache-2.0; v0.7.0-beta.15 on crates.io, ~287k downloads (verified 2026-07-31). Deliberately minimal — "simplest" option in independent comparisons. [agentsdk alternatives doc](https://github.com/Recusive/agentsdk/blob/dev/docs/providers/RUST_AI_SDK_ALTERNATIVES.md)
- **What's missing:** No agents, no graph, no persistence — a chat client only. Reinforces that the provider layer is well-served and crowded.

### 2.5 Orion

- The most visible "Orion" in this space (`AshishKumar4/Orion`, "multi-agent LLM orchestration framework") has only **7 stars** and was last pushed 2025-02-17 (GitHub API, 2026-07-31). [GitHub](https://github.com/AshishKumar4/Orion) Effectively an abandoned personal experiment — useful only as evidence that the "multi-agent orchestration in Rust" idea recurs but no implementation has consolidated.

### 2.6 Swarms-rs (`The-Swarm-Corporation/swarms-rs`)

- **What it does:** Rust port of the (Python) Swarms "enterprise-grade multi-agent orchestration framework"; agent structs with concurrent swarm patterns. [GitHub](https://github.com/The-Swarm-Corporation/swarms-rs)
- **Maturity:** Only **175 stars**, Apache-2.0, last push 2025-12-15; crates.io `swarms-rs` v0.2.1 (2025-09-08), ~5.3k total downloads (verified 2026-07-31). Marketing language ("first-ever enterprise-grade, production-ready") far outruns adoption.
- **What's missing:** No graph model, no checkpointing, no HITL; small community; development cadence is sporadic. Not a credible LangGraph analogue.

### 2.7 langchain-rust (`Abraxas-365/langchain-rust`)

- **What it does:** Rust port of LangChain concepts: composable prompts, chains, agents, vector-store integrations (uses `fastembed-rs` for local embeddings). [crates.io](https://crates.io/crates/langchain-rust), [Braintrust SDK issue noting fastembed integration](https://github.com/braintrustdata/braintrust-sdk-rust/issues/67)
- **Maturity:** 1,335 stars, MIT — **but stalled**: last crates.io release v4.6.0 on 2024-10-06, ~150k downloads; GitHub pushes resumed only recently (2026-07-23) after a long gap (verified 2026-07-31).
- **What's missing:** A chain/agent model, not a graph runtime; no durable checkpoints, no interrupt/resume semantics. Its stall mirrors `llm-chain`: linear-chain ports of LangChain have not sustained in Rust.

### 2.8 ADK-Rust (`zavora-ai/adk-rust`)

- **What it does:** A native Rust implementation of Google's Agent Development Kit: modular agents/models/tools/memory, event streaming via SSE, A2A (agent-to-agent) protocol support, multi-model backends (Gemini, OpenAI, Anthropic, DeepSeek, Groq, Ollama), plus `adk-studio`, a visual low-code builder with ReactFlow canvas and Rust code generation. [adk-rust.com](https://adk-rust.com/en), [wrenlearnsrust review, 2026-03-11](https://wrenlearnsrust.com/posts/adk-rust-native-agent-framework.html), [adk-studio](https://github.com/zavora-ai/adk-studio)
- **Maturity:** 574 stars, Apache-2.0 (LICENSE file; GitHub flags "NOASSERTION"), very active (last push 2026-07-31); `adk-rust` hit **v1.0.0** on crates.io 2026-06-08, ~19k downloads (verified 2026-07-31). Announced to the Google ADK community 2026-07-29. [google/adk-python discussion #3913](https://github.com/google/adk-python/discussions/3913)
- **What's missing:** Its composition model is ADK's (sequential/parallel/loop/router agents), **not** a general state-graph with checkpointing and interruptible human-in-the-loop execution. Young (2026), single-vendor-driven, unproven at scale.
- **Note:** A separate, unrelated `inference-gateway/rust-adk` builds A2A-compatible agents in Rust. [GitHub](https://github.com/inference-gateway/rust-adk)

### 2.9 Qdrant agent-related tooling

- **Rust client** (`qdrant/rust-client`): official gRPC client, 412 stars, Apache-2.0, actively maintained (last push 2026-07-25). [GitHub](https://github.com/qdrant/rust-client)
- **fastembed-rs** (`Anush008/fastembed-rs`): pure-Rust local embedding/reranking (ONNX, no Python runtime), 978 stars, 30+ models, used as an embedding provider by `langchain-rust`. [Braintrust issue](https://github.com/braintrustdata/braintrust-sdk-rust/issues/67)
- **mcp-server-qdrant**: official MCP server giving agents semantic memory (`qdrant-store`/`qdrant-find` tools) — but written in **Python**, ~1.5k stars, v0.8.1 (2025-12-10). [DEV.co directory](https://dev.co/ai/mcp/mcp-server-qdrant) Notably, it is the only vector-DB MCP server supporting stdio, SSE, and Streamable HTTP, and supports a local embedded mode. [ChatForest review](https://chatforest.com/reviews/qdrant-mcp-server/)
- **Implication:** The memory/retrieval substrate for Rust agents is solid, but the official *agent-facing* memory server isn't Rust — an integration target for `agentgraph`, not competition.

### 2.10 Local inference: candle, burn, mistral.rs

- **candle** (`huggingface/candle`): minimalist Rust ML framework for CPU/GPU/WASM inference; 20,809 stars, Apache-2.0, active (last push 2026-07-30). [GitHub](https://github.com/huggingface/candle)
- **burn** (`tracel-ai/burn`): full deep-learning framework with autodiff and multi-GPU training; 15,688 stars, Apache-2.0, very active (last push 2026-07-31). (GitHub API, 2026-07-31)
- **mistral.rs** (`EricLBuehler/mistral.rs`): candle-based LLM inference engine with an OpenAI-compatible HTTP server; 7,554 stars, MIT, active. (GitHub API, 2026-07-31) [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)
- **Cautionary precedent:** `rustformers/llm` (6,154 stars) is **archived** (last push 2024-06-24) — the inference layer consolidated around candle/burn/mistral.rs. (GitHub API, 2026-07-31)
- **Implication:** Local inference is solved and orthogonal; `agentgraph` should treat candle/mistral.rs endpoints as just another provider behind its traits.

### 2.11 Graph-based agent orchestration crates — the direct competitors

This is the category `agentgraph` would enter. Verified inventory (2026-07-31):

| Project | What it is | Stars / activity | Gap vs. LangGraph-class core |
|---|---|---|---|
| `a-agmon/graph-flow` → renamed **`rs-graph-llm`** | Lean graph execution framework: `Task` trait, `Context` store, conditional edges, `NextAction::WaitForInput` for HITL pauses, session persistence (PostgreSQL), Rig integration | 357 stars; graph-flow v0.6.0 (2026-07-19), ~8.9k downloads — the **most credible existing attempt** | Young, single-maintainer; no checkpoint/versioning model comparable to LangGraph checkpointers; small community |
| `greatwallisme/juncture` | Explicit "Rust implementation of LangGraph's state machine framework" | 23 stars, first pushes 2026-07 | Days old; unproven |
| `Mattbusel/agent-runtime` | Unified tokio agent runtime (orchestration, memory, knowledge graph, ReAct) | 6 stars, last push 2026-03-23 | Personal project |
| `liquidos-ai/AutoAgents` | Multi-agent framework (structured tool calling, configurable memory) | 723 stars, active (2026-07-30) | Agent-executor model, not a state graph; no checkpoint/HITL core |
| `neul-labs/fast-langgraph` | Rust accelerators *for* (Python) LangGraph | 24 stars | Complements Python LangGraph rather than replacing it |
| ruvnet's "LangGraph Rust/WASM Implementation Specification" | Design spec only, with AgentDB integration (gist, 2025-11-11) | n/a | No shipped implementation |

Sources: GitHub API repo metadata and search (2026-07-31); [GraphFlow design article, 2025-06-30](https://ai.gopubby.com/graphflow-rust-native-orchestration-for-multi-agent-workflows-6143a9b767ad); [rs-graph-llm repo](https://github.com/a-agmon/rs-graph-llm); [juncture](https://github.com/greatwallisme/juncture); [agent-runtime](https://github.com/Mattbusel/agent-runtime); [LangGraph spec gist](https://gist.github.com/ruvnet/7bd802bb143df8cd7e5c0fbf3ac7f21a); [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/).

**Baseline for comparison — what LangGraph itself provides:** stateful graph orchestration with persistence (checkpointers), memory, human-in-the-loop interrupts, and resumable execution. [LangChain — LangGraph](https://www.langchain.com/langgraph) No Rust crate today ships all four as first-class, production-hardened primitives.

---

## 3. The verified gap

1. **Provider abstraction is crowded; orchestration is not.** `async-openai` (~6.8M downloads), `rig-core` (~1.9M downloads), and `genai` prove the demand, but every download-count leader sits at the client layer (crates.io API, 2026-07-31).
2. **First-generation "LangChain ports" stalled.** `llm-chain` (last release 2023-11) and `langchain-rust` (last release 2024-10) both went quiet — linear chains without durable state failed to hold users (crates.io API, 2026-07-31).
3. **Graph-based orchestration is being repeatedly attempted but nothing has consolidated.** `graph-flow`/`rs-graph-llm` (357 stars) is the strongest; `juncture` (23 stars) launched July 2026; everything else is sub-10-star personal work. For contrast, Python LangGraph is the industry default. [LangChain — LangGraph](https://www.langchain.com/langgraph)
4. **No Rust crate offers the LangGraph quartet** — state graph + durable checkpointing + human-in-the-loop interrupts + resumable execution — as mature, first-class primitives. Independent ecosystem analysis concurs that the orchestration layer is the ecosystem's weakest link. [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)
5. **The performance case for a Rust core engine is well documented:** deterministic streaming latency without GC pauses, true parallelism without a GIL for concurrent tool calls, compile-time-validated tool schemas, and 10–45x memory footprint reductions (directionally informative figures from aggregator benchmarks, not rigorously verified). [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)

---

## 4. Recommended positioning for `agentgraph`

**One-liner:** *"The checkpointable, human-in-the-loop agent graph runtime for Rust — LangGraph's execution model, rebuilt on tokio, with Rust's safety and single-binary deployment."*

Design and positioning recommendations:

1. **Scope: orchestration core only.** Do not build provider clients. Define provider-agnostic traits (`ChatModel`, `Embedder`, `Tool`) and ship thin adapters for Rig, `async-openai`, and `genai` — the ecosystem's proven pattern (graph-flow integrates Rig rather than competing with it). [GraphFlow article](https://ai.gopubby.com/graphflow-rust-native-orchestration-for-multi-agent-workflows-6143a9b767ad)
2. **Make the LangGraph quartet the headline features:** (a) typed state graphs with conditional edges and cycles; (b) pluggable checkpointers (in-memory, SQLite/Postgres via `sqlx`, Qdrant optional for semantic memory); (c) first-class interrupts/HITL (`WaitForInput`-style suspension that persists to a checkpoint, not just an in-memory pause); (d) resume/replay/time-travel from checkpoints. This is precisely what no Rust crate ships today (Section 3).
3. **Tokio-native, `async-trait`-based, streaming-first** (SSE), with `CancellationToken` trees and bounded-channel backpressure — the idiomatic patterns Rust agent developers already expect. [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)
4. **License: Apache-2.0 OR MIT dual** — the dominant, enterprise-safe choice in this ecosystem (candle, burn, rust-genai are Apache-2.0; rig, async-openai, langchain-rust are MIT; dual-licensing satisfies both camps).
5. **Compile-time correctness as the differentiator vs. Python:** typed node state via serde + generics, tool schemas as traits — errors caught at build time, not at runtime mid-conversation. [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)
6. **Optional FFI bindings (PyO3 / napi-rs) as growth levers, behind feature flags.** The dominant production architecture is already hybrid — Python for research, Rust for the runtime hot path, with PyO3 usage growing ~22% YoY. Bindings let Python teams adopt `agentgraph` as the engine under existing tooling. [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)
7. **Interop surface:** MCP (agent-facing memory like `mcp-server-qdrant`), A2A protocol (as `adk-rust` and `rust-adk` are doing), and OpenTelemetry tracing (Rig's GenAI semantic conventions) — meeting the ecosystem where it is. [GitHub — rust-adk](https://github.com/inference-gateway/rust-adk), [Rig README](https://github.com/0xPlaygrounds/rig)
8. **Learn from the failures:** `llm-chain`/`langchain-rust` show that shallow ports die. `agentgraph` should avoid feature-by-feature parity chasing and instead own one hard problem — durable, interruptible graph execution — better than anyone.
9. **Competitive framing vs. the nearest neighbors:** `adk-rust` (ADK-flavored agent composition, single-vendor, young), `swarms-rs` (low adoption, no graph/checkpointing), `rs-graph-llm` (closest in spirit but single-maintainer, no checkpoint versioning/HITL depth). `agentgraph`'s wedge is *durable execution semantics* (checkpoints, resume, HITL) with multi-adapter neutrality.

---

## 5. Key takeaways for the whitepaper

- **The Rust AI stack has a donut shape:** strong provider clients (async-openai ~6.8M downloads; rig-core ~1.9M) and strong inference engines (candle 20.8k stars, burn 15.7k stars), but a hollow middle where durable agent orchestration should be. (GitHub/crates.io APIs, 2026-07-31)
- **Every prior Rust "LangChain port" stalled** (`llm-chain` unmaintained since Nov 2023; `langchain-rust` stalled since Oct 2024) because linear chains without durable state don't match how production agents actually run. (crates.io API, 2026-07-31)
- **Graph-based orchestration in Rust is an open race:** the best existing attempt (`graph-flow`/`rs-graph-llm`, 357 stars) is a single-maintainer project without LangGraph-grade checkpointing; a fresh LangGraph-state-machine clone (`juncture`) appeared in July 2026 with 23 stars. The market is signaling demand without supply.
- **`agentgraph` should position as the execution/runtime layer, not another client library:** provider-agnostic traits with adapters to Rig/async-openai/genai; tokio-native; Apache-2.0/MIT dual license; optional PyO3/napi bindings to ride the proven "Python research + Rust production" hybrid pattern.
- **The headline feature set is the LangGraph quartet** — state graphs, durable checkpoints, human-in-the-loop interrupts, resumable execution — which no Rust crate ships as mature first-class primitives today.
- **The performance narrative is credible but should be cited carefully:** reported 7–10x throughput and ~9x p99-latency improvements over Python come from aggregator benchmarks and are directionally informative, not rigorously verified. [Zylos Research](https://zylos.ai/research/2026-03-31-rust-ai-agent-frameworks-infrastructure/)
- **Interoperability is table stakes:** MCP memory servers (e.g., `mcp-server-qdrant`), A2A protocol, and OpenTelemetry/GenAI semantic conventions should be supported from day one.

---

*Verification note: all star counts, push dates, license identifiers, version numbers, and download totals in Sections 2–3 were pulled live from the GitHub REST API and crates.io API on 2026-07-31. Publication dates of secondary sources are noted inline.*
