# Five Days Later: The Whole Engine Is Rust Now

Five days ago we published [Why We Build Our Agent Core in Rust](why-we-build-our-agent-core-in-rust.md), and closed with a phasing plan: Phase 1, the Rust core — done. Phase 2, the interop and platform layer — someday. Phase 3, steady state — eventually.

Someday turned out to be five days. This post is the receipt.

## The thesis, in three sentences

Agents are long-running, stateful, concurrent services, so their runtime is systems software and should be built like it. The moat around Python agent stacks is code volume, and code-volume moats are exactly what AI demolishes — with Rust's compiler acting as a free, relentless verifier on every generated line. Therefore: Rust engine underneath, and let AI drain the integration gap instead of conceding it.

That was the argument. The fair pushback we got was that the argument is cheap and the surface area isn't. A "platform" is a server, a protocol, interop, observability, a debug story. Building that is the part that's supposed to take quarters.

So here's what exists now.

## The inventory, five days in

Everything below is in the repo, tested, and dual MIT/Apache-2.0. Four crates, ~140 tests green, live-Postgres integration tests included.

**`agentgraph` v0.4.0 — the core.** Everything from v0.1 (state channels with reducers, the Pregel/BSP super-step executor, checkpoints, HITL interrupts, `Send` fan-out, the prebuilt ReAct agent), plus: a `sqlx`-backed Postgres checkpointer behind a feature flag; real token streaming (`ChatModel::chat_stream` decoding SSE deltas off the wire, surfaced as `GraphEvent::Token` — the LangGraph `messages` stream mode); an MCP client; `RemoteNode` for executing graph nodes on remote services, with interrupts crossing the wire; sandboxed `WasmNode` execution via Wasmtime behind the same `Node` trait; time travel (`get_by_id`, `fork_thread`, replay from any checkpoint id); and `tracing` spans through the super-step loop.

**`agentgraph-server` v0.3.0 — the network face.** An axum library crate — `serve(registry, config)` from your own `main.rs`, one static binary — implementing an Agent-Protocol subset: threads, background/blocking/SSE runs, checkpoint history, a per-thread run queue, run-status polling, assistants, crons, a KV store, API-key auth, and CORS for browser clients. `ServerConfig::with_postgres(url)` moves checkpoints *and* the assistants/crons/KV surface into Postgres in one call. Time travel is a wire protocol: `POST /threads/{id}/fork`, plus `"checkpoint": {"checkpoint_id": …}` replay on all three run endpoints. Fork first, replay on the fork.

**`agentgraph-worker` v0.1.0 — polyglot interop.** A worker SDK that serves your node handlers over HTTP so `RemoteNode` can call them. HITL interrupts cross the wire, so a remote node suspends and resumes runs exactly like a local one.

**`agentgraph-otel` v0.1.0 — observability.** One-call tracing subscriber setup with optional OTLP span export.

**Plus Studio:** a zero-build, single-file debug UI — vanilla JS, no npm — that connects to any `agentgraph-server`: create threads, run/wait/stream, inspect state and checkpoint history, fork and replay from any checkpoint.

Two items from the old roadmap deserve a footnote, because we changed our minds — and it's the same thesis that made us do it.

**MCP is the escape hatch now.** v0.1's plan said provider adapters, one by one. Instead, any MCP tool server registers into our `ToolRegistry` exactly like a native tool, and the prebuilt ReAct agent drives MCP tools with no graph changes. The long tail of integrations doesn't need porting; it needs a protocol, and MCP is the one the industry actually converged on.

**The HTTP/SSE server is the bindings layer.** We explicitly rejected PyO3 and napi-rs — they'd freeze a trait surface that's still moving and split maintenance across three ecosystems. If your workers speak HTTP, your language doesn't matter. Python and TypeScript client SDKs are in flight against that surface, which is a better hybrid story than FFI: the interop boundary is a versioned wire protocol, not a shared object file.

## What five days actually proved about AI writing Rust

Transparent about process: most of the code across v0.2 through v0.4 was AI-written, under human direction. Which makes this project a small controlled experiment in the first post's bet. Three findings.

**The compiler-as-verifier loop is real, and it's the whole game.** Parallel workstreams — MCP client, remote nodes, server API completion, WASM nodes, time travel — landed concurrently without merge hell, because Rust's type system is an interface contract with an enforcement arm. When a generated node held a reference across an `await` it shouldn't have, or a stream decoder mishandled a borrow, the compiler caught it before a human ever saw the diff. In a Python codebase, that same five-day sprint produces five days of integration debugging. Here it produced a changelog.

**Spec-first beats prompt-first.** What made parallel agents work wasn't better prompting; it was that each workstream had a written contract — a design doc for the server endpoints (SSE frame shapes, `stream_mode` filtering, frame ids as `{checkpoint_id}:{step}:{seq}`), a trait for the checkpointer, a wire shape for the worker SDK. Agents implementing against a spec can be parallelized; agents implementing against vibes cannot. The specs were human-written. That was not an accident.

**Humans still own three things: design, taste, and the weird ones.** The one bug that made the changelog this cycle is instructive: concurrent cold boots against a fresh Postgres raced their auto-migrations and died on `duplicate key value violates unique constraint "pg_type_typname_nsp_index"`. No compiler catches a distributed-systems race; a human recognized the failure mode, and the fix — migrations inside a transaction holding a transaction-scoped advisory lock — is a design decision, not a syntax correction. Same story for API taste: "fork first, replay on the fork," making the server a library crate you embed rather than a daemon we operate, rejecting the FFI bindings outright — those were arguments between humans, with AI as a very fast typist on the winning side.

The velocity claim we'd actually defend is narrower than "AI wrote a platform in five days": **AI plus a verifying compiler plus human-written specs compressed the mechanical 80% of platform work into days — and surfaced the non-mechanical 20% faster**, because we hit the hard problems with working code in hand instead of with a design review.

## What we have not solved

No triumphalism this time either. The honest list:

**Ecosystem breadth.** MCP gives us the protocol, not the content. The Python stacks still have years of provider adapters, retrievers, loaders, and eval harnesses. Our escape hatch means you're never *blocked*, but "callable through a tool server" is not the same as "native and tuned."

**Community.** Five days of AI-assisted velocity produces zero tutorials, zero Stack Overflow answers, zero blog posts by people who aren't us, zero hiring pipeline. That network effect was the strongest counterargument in the first post, and nothing we shipped touches it.

**Trust.** A v0.4.0 with ~140 tests is a promising artifact, not a production track record. Nobody should bet a company on a five-day-old server crate. What we'd claim: the architecture is the right shape, the failure modes are designed-for — checkpoint everything, replay anything — and the codebase is small enough to audit. The whole core is human-readable in an afternoon.

## Next

- **Publishing.** crates.io releases of all four crates once the API surface settles past v0.4; docs.rs as the reference.
- **Client SDKs.** Python and TypeScript clients against the HTTP/SSE surface — the hybrid, delivered over HTTP instead of FFI.
- **Hosted control plane.** The server crate operated as a managed multi-tenant service: tenant isolation, durable queues, autoscaling workers.
- **WASM target for graphs themselves** — run whole graphs in the browser or at the edge, not just sandboxed nodes.

The first post ended with "agents grew up; their runtime should too." Five days later we'd amend it: the runtime grew up faster than we planned, and the compiler held the ladder the whole way.

The repo: [github.com/dev-amjad-shaikh/agentgraph](https://github.com/dev-amjad-shaikh/agentgraph) *(link live at publish)*. Come argue in the issues — especially about the rejected-bindings call.

---

*Dual-licensed MIT/Apache-2.0. Previous in this series: [Why We Build Our Agent Core in Rust (and What It Would Take to Go All the Way)](why-we-build-our-agent-core-in-rust.md).*
