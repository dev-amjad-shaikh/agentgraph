# Plan — Rust for Agentic Core Engine: Whitepaper + Open-Source Agentic Core (`agentgraph`)

Workspace: `/Users/amjad.shaikh/claude-work/claude-white-papers/05 - RUST` (path contains spaces — always quote it).

## Deliverables
1. **Whitepaper** (`whitepaper/Rust_Agentic_Core_Whitepaper.md` + `.docx`):
   - The case for Rust as the language for an agentic core engine (performance, safety, concurrency, TCO).
   - Pros/cons of Rust for the **core only** (hybrid, FFI bindings to Python/TS) vs the **whole engine** in Rust.
   - Reference architecture of the open-source core built alongside.
2. **Open-source project** `agentgraph/` — a LangGraph-style agentic core engine in Rust:
   typed state graph, nodes/edges/conditional routing, async executor on tokio,
   checkpointing, tool & LLM-provider abstractions, examples, tests, README, dual MIT/Apache-2.0 license.

## Stage 1 — Research (4 × explore agents, parallel)
Output: briefs in `research/*.md`. No writing yet.
- R1 `research/rust_infra_evidence.md` — production evidence for Rust in infra/AI (Discord, Cloudflare, AWS Firecracker, Deno, Vector, Polars, Ruff, Qdrant, LanceDB), performance/memory/safety data points.
- R2 `research/langgraph_architecture.md` — LangGraph design deep-dive: StateGraph, channels/reducers, nodes, conditional edges, checkpointers, human-in-the-loop, streaming, Command/Send API — the feature surface `agentgraph` must mirror.
- R3 `research/rust_agent_landscape.md` — existing Rust agent/LLM frameworks (Rig, llm-chain, async-openai, orion, Swarms-rs…), maturity, gaps → positioning for a new open-source core.
- R4 `research/core_vs_whole_engine.md` — hybrid-core vs full-Rust tradeoffs, FFI strategy (PyO3/maturin, napi-rs), precedent projects (Polars, Ruff, delta-rs, Tauri, pydantic-core), rewrite-cost and ecosystem arguments.

## Stage 2 — Draft + Scaffold (parallel, 3 × coder)
- W1 `whitepaper/draft_part1.md` — §1–5: exec summary; agentic engines as systems software; the Rust case (perf, fearless concurrency, safety, WASM); production evidence (uses R1, R2 input).
- W2 `whitepaper/draft_part2.md` — §6–10: core-vs-whole-engine analysis w/ pros-cons tables; hybrid FFI architecture; reference architecture of `agentgraph`; risks & roadmap (uses R3, R4).
- S1 Scaffold `agentgraph/` crate: Cargo.toml, lib.rs, all module files with **complete type definitions + stub bodies**, so parallel implementers can fill files without API drift. Modules: `error, state, node, graph, executor, checkpoint, llm, tool`.

## Stage 3 — Implementation (parallel coders, one file-scope each, non-conflicting)
- state.rs (channels + reducers), graph.rs (builder + conditional edges), executor.rs (tokio super-step runtime),
  checkpoint.rs (trait + SQLite/JSON file impl), llm.rs + tool.rs (provider/tool abstractions),
  examples/ (react_agent, parallel_fanout, human_in_loop), tests/, README+licenses+CI yaml.

## Stage 4 — Verify & Package (1 coder)
- `cargo build`, `cargo test`, `cargo clippy` clean; fix all errors.
- Integrate draft_part1+2 → final whitepaper .md; convert to `.docx` (python-docx via managed python).

## Stage-Gates
Research briefs validated before Stage 2; scaffold compiles (`cargo check`) before Stage 3; all tests green before packaging.
