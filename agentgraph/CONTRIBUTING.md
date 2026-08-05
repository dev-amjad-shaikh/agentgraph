# Contributing to agentgraph

Thanks for helping build the durable agent-graph runtime for Rust. This document covers setup, the checks every PR must pass, and how the codebase is organized.

## Setup

You need a stable Rust toolchain (edition 2021; 1.75+ recommended):

```bash
# Install rustup (skip if you already have it)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Components used by CI and this guide
rustup component add rustfmt clippy

# Build and run the test suite
cargo build
cargo test
```

There is no system-level dependency: the crate builds on plain `tokio` + `serde` + `reqwest` (rustls, no OpenSSL).

## The checks (run these before pushing)

CI enforces all of the following on stable Rust, across Ubuntu and macOS:

```bash
# Formatting — must be a no-op
cargo fmt --all -- --check

# Lint — warnings are errors
cargo clippy --all-targets -- -D warnings

# Tests — unit tests live in-module under #[cfg(test)]
cargo test
```

Optional but appreciated:

```bash
# Docs build cleanly and intra-doc links resolve
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

## Module map & ownership

| File | Owns | What to know before editing |
|---|---|---|
| `src/lib.rs` | Crate docs, module wiring, `prelude` re-exports | **Owned by the scaffold engineer.** Do not edit in implementation PRs — request changes instead. Same for `Cargo.toml`. |
| `src/error.rs` | The single `AgentGraphError` type + `Result` alias | `Interrupt` is a *suspend signal*, not a failure — never log it as an error. |
| `src/state.rs` | `State`, `StateSpec`, `Reducer` | The `LastValue` single-write-per-super-step rule and undeclared-channel rejection live in `StateSpec::apply_super_step`; keep super-steps transactional. |
| `src/node.rs` | `Node` trait, `NodeContext`, `NodeOutput`, `Command` | Nodes see only an immutable snapshot; document idempotency requirements when touching interrupt/resume helpers. |
| `src/graph.rs` | `GraphBuilder`, `Graph`, `Edge`, `Route`, `Send` | `compile()` must validate everything that *can* be validated before execution; router/`Send` targets are data-dependent and validated at runtime by the executor. |
| `src/executor.rs` | `Executor`, `RunConfig`, `ExecutionOutcome`, `GraphEvent` | The super-step algorithm is specified in the `run()` doc comment — implement exactly that contract (barrier, transactional merge, routing precedence, best-effort event emission). **Current status: `todo!()` — the top good-first-issue.** |
| `src/checkpoint.rs` | `Checkpointer` trait, `Checkpoint`, `InMemoryCheckpointer`, `JsonFileCheckpointer` | Checkpoints happen only at super-step boundaries. `put` must never overwrite an existing id. `JsonFileCheckpointer` is declared but `todo!()` (pure `serde_json` + `tokio::fs`, no DB deps). |
| `src/llm.rs` | `ChatModel` trait, `ChatMessage`/`ToolCall` wire types, `OpenAiCompatibleClient` | Wire format follows OpenAI chat-completions; keep the trait minimal so Rig/async-openai/genai adapters stay thin. |
| `src/tool.rs` | `Tool` trait, `ToolRegistry`, `ToolExecutor` | `execute_batch` must stay parallel, order-stable, and failure-isolating (a failed call becomes an `ERROR:` tool message, not a batch failure). |

### Good first issues

1. **Executor super-step loop** (`src/executor.rs`) — fully specified in its doc comment, including the interrupt and `Send` handling.
2. **`JsonFileCheckpointer`** (`src/checkpoint.rs`) — signatures are stable; write to temp file + rename for atomicity.
3. **Examples** under `examples/` (quickstart, ReAct agent, HITL approval, map-reduce fan-out).
4. **Provider adapters** — `ChatModel` impls wrapping Rig / `async-openai` / `genai`, behind feature flags.

## PR guidelines

- **Branch & title:** short-lived branches; PR titles in imperative mood (`Add JsonFileCheckpointer::put`, not `added...`).
- **Scope:** one concern per PR. Do not touch `Cargo.toml` or `src/lib.rs` — propose the change in an issue instead.
- **Tests:** every behavioral change ships with unit tests in the same module (`#[cfg(test)]`). Mirror the existing style — table-style reducer tests, `tokio::test` for async.
- **Docs:** public items carry doc comments; document *contracts and invariants* (e.g. "at most one write per super-step"), not just signatures. Code samples in docs must match the real API.
- **Semantics over churn:** changes to super-step, reducer, or interrupt/resume semantics need an issue and design discussion first — these are the project's core promises.
- **Commits:** keep them atomic; `cargo fmt` before committing.
- **CI:** all PRs run fmt, clippy, and tests on stable + beta, Ubuntu + macOS. A red CI is a blocked PR.

## Code of conduct

This project follows the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct). Be kind, be constructive, assume good faith. Report unacceptable behavior by opening a private issue or contacting the maintainers.

## License

By contributing, you agree that your contributions are dual-licensed under MIT OR Apache-2.0, matching the project.
