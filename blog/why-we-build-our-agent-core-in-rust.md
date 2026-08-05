# Why We Build Our Agent Core in Rust (and What It Would Take to Go All the Way)

Last week we shipped `agentgraph` v0.1.0 — a LangGraph-style agentic core engine written entirely in Rust. Cyclic state graphs, channel reducers, Pregel-style super-step execution, durable checkpoints, human-in-the-loop interrupts, a prebuilt ReAct agent. 54 passing tests, three runnable examples, dual MIT/Apache-2.0.

Every time we mention this, someone asks the same question: *why not just write it in Python like everyone else?*

This post is our answer — not a benchmark dump, not a whitepaper. Just our reasoning, some real code, and one idea we think the industry is underrating: what happens to the "Python owns the ecosystem" argument when AI starts writing most of the code.

## Agents are infrastructure now

Two years ago an "agent" was a weekend demo: a Python script, a `while` loop, an OpenAI call, a prayer. That era is over.

Production agents in 2026 look nothing like that. They run for hours or days, hold state across restarts, juggle hundreds of concurrent LLM streams and tool calls, pause mid-run for human approval, checkpoint themselves so a deploy doesn't lose the conversation, and fan work across parallel branches that must merge back without corrupting shared state. They are, in every way that matters, **long-running, stateful, concurrent services**.

And we already know what we ask of long-running, stateful, concurrent services: predictable tail latency, honest parallelism, a small memory footprint, boring deployability. Nobody runs their message queue or their proxy layer as an interpreted script. Yet most agent frameworks today are exactly that.

The pattern underneath modern agent frameworks — LangGraph's model, which we deliberately rebuilt — is a graph runtime: nodes publish partial updates to shared state, an executor schedules them in super-steps, and checkpoints make the whole thing durable. That is an *execution engine*, and execution engines are systems software — it's time they were built like it.

## Why we chose Rust for the core

Four reasons, ordered by how often they bite you in production.

**1. Python's orchestration tax is real, and it compounds.** The GIL means your "parallel" tool calls aren't. GC pauses mean your p99 latency is at the mercy of a collector you don't control — ask Discord, whose Go service spiked every two minutes until [their Rust rewrite removed the spikes entirely](https://discord.com/blog/why-discord-is-switching-from-go-to-rust). And then there's checkpoint bloat: pickling live Python state for durable execution is fragile, version-locked, and heavy. An agent runtime spends its whole life doing exactly what Python is worst at: concurrent I/O, state merging, serialization.

**2. Predictable tail latency beats average latency.** Agents are latency-stackers: one user-visible turn is LLM call + tool calls + state merges + checkpoint writes, repeated per super-step. If each layer occasionally takes a GC vacation, the tail of the product is miserable even when the median looks fine. Rust gives you deterministic memory management — deallocation happens at scope exit, not when a collector feels like it. Cloudflare made the same call at proxy scale: [Pingora serves a trillion requests a day at roughly 70% less CPU and 67% less memory](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/) than the NGINX stack it replaced. Agent orchestration is the same shape of workload — massive concurrent I/O over a shared runtime.

**3. The compiler is a design partner.** This one surprises people coming from Python. In `agentgraph`, graph topology is validated by `compile()` *before* any node runs — before any paid LLM call runs. Two nodes writing to a `LastValue` channel in the same super-step? Typed error, at the barrier, every time — not a mid-conversation traceback in someone's production thread at 3 a.m. The borrow checker forces you to make the snapshot semantics of parallel execution explicit, which is precisely the discipline a BSP-style executor needs. Half the bugs in parallel agent frameworks are shared-state races; our compiler treats that category as a compile-time conversation.

**4. Single-binary deployment.** One static artifact. No interpreter, no venv, no dependency resolver at deploy time, no "works on my Python 3.11 but not your 3.12." Our dependency tree is small and auditable: tokio, serde, reqwest+rustls, thiserror. When your agent runtime might be embedded in a sidecar, an edge worker, or eventually a WASM module, that form factor stops being a nice-to-have.

What do we give up? Runtime monkey-patching and edit-run loop speed — the right trade for a component whose whole job is to be *boring and durable*.

## Show me the code

Enough philosophy. Here's what the engine actually looks like — real code from the repo, trimmed for brevity but API-exact.

**Defining a graph with reducers.** Every state key is a typed *channel* with a per-key reducer; nodes are async closures returning partial updates:

```rust
// 1. State schema: channel name -> reducer.
let spec = StateSpec::new()
    .channel("messages", Reducer::AddMessages)
    .channel("done", Reducer::Overwrite);

// 2. Register nodes: any async closure `Fn(NodeContext) -> Result<NodeOutput>`.
let mut builder = GraphBuilder::new();

builder.add_node("greeter", |_ctx: NodeContext| async move {
    Ok(NodeOutput::update(
        "messages",
        json!({"role": "assistant", "content": "Hello from agentgraph!"}),
    ))
});

builder.set_entry_point("greeter");

// 3. Compile: validates entry point + every edge endpoint *before* running.
let graph = builder.compile()?;

// 4. Run.
let outcome = Executor::new()
    .run(&graph, &spec, State::new(), RunConfig::new("thread-1"))
    .await?;
```

Writes to undeclared channels are rejected. Two writers to an `Overwrite` channel in one super-step — rejected. If you've ever debugged a LangGraph graph where a silent key collision corrupted state three super-steps downstream, you know why we made the merge semantics this explicit.

**The prebuilt ReAct agent.** The standard `agent → tools → agent` loop assembles in one call:

```rust
let mut registry = ToolRegistry::new();
registry.register(Calculator);
registry.register(Echo);

let model: Arc<dyn ChatModel> = Arc::new(ScriptedModel::new(vec![/* ... */]));

// The prebuilt graph: agent ⇄ tools over the `messages` channel.
let graph = create_react_agent(model, registry)?;

let spec = StateSpec::new().channel("messages", Reducer::AddMessages);
let config = RunConfig::new("react-demo").with_max_steps(10);
let outcome = Executor::new().run(&graph, &spec, initial, config).await?;
```

The `tools` node dispatches the tool-call batch concurrently — real parallelism, no GIL — preserves call order, and isolates per-tool failures so one bad tool doesn't kill the loop. (Parallel *nodes* within a super-step — that's where the tokio `JoinSet` lives.) The `ChatModel` trait is deliberately minimal: bring Rig, `async-openai`, or the bundled OpenAI-compatible client as your provider layer.

**Interrupt and resume for human-in-the-loop.** This is the feature we're proudest of, because it's where durable execution stops being a buzzword:

```rust
builder.add_node("approve", |ctx: NodeContext| async move {
    match ctx.resume_value() {
        // Phase 2: a human decision was supplied via RunConfig::resume.
        Some(decision) => Ok(NodeOutput::update("approval", decision.clone())),
        // Phase 1: no decision yet — suspend the whole run.
        None => Err(ctx.interrupt(json!({
            "kind": "approval_request",
            "prompt": "Approve this draft for publication?",
        }))),
    }
});

// ... first run suspends at the interrupt and checkpoints to disk ...

// Resume with the SAME thread_id and the human's decision:
let outcome = executor
    .run(&graph, &spec, State::new(),
         RunConfig::new(thread_id).with_resume(human_decision.clone()))
    .await?;
```

On interrupt, the executor unwinds the in-flight super-step transactionally, persists a checkpoint (the example uses a pure-`serde_json` file checkpointer, so the two phases could be *separate process invocations*), and surfaces your payload. On resume, it restores the checkpoint and re-executes the node with `ctx.resume_value()` set. One primitive — thread-scoped, versioned checkpoints — buys you durable execution, human-in-the-loop, time travel, and partial-failure recovery.

## Core in Rust — but what about the whole engine?

Here's where we part ways with the "rewrite everything in Rust" crowd.

Our position is **hybrid core-first**. The execution engine — graph scheduling, state merging, checkpointing, the ReAct loop — belongs in Rust. But a whole agent *platform* is much more than an execution engine: it's hundreds of provider integrations, vector-store clients, document loaders, eval harnesses, and a research community that lives in notebooks. That breadth is Python's, and it was earned.

The cautionary tale is Deno. Deno is, by most technical measures, a better runtime than Node — Rust core, secure by default, single binary. And it still had to swallow [full npm compatibility in Deno 2.0](https://deno.com/blog/npm), because telling developers to abandon 1.4 million packages was an adoption blocker no engineering excellence could overcome. Ecosystems win on breadth. A Rust agent engine that says "rewrite your integrations" will lose to a worse engine that says "bring them."

So the sane architecture is the one the industry keeps converging on: Rust engine underneath, bindings and ecosystem compatibility on top. Polars, pydantic-core, delta-rs — [whose maintainer credits the Python bindings with exploding the contributor base](https://www.buoyantdata.com/blog/2025-03-09-lessons-learned-building-delta-rs.html) — all prove the pattern. That's why PyO3 and napi-rs bindings are on our roadmap, not an afterthought.

End of essay, right? Hybrid wins, ship it.

Not quite. One variable in this equation is changing fast, and deserves a harder look.

## The AI twist: what if AI builds all the gaps?

The standard argument for the hybrid rests on one premise: **porting the ecosystem is too expensive.** Thousands of integrations, adapters, clients, tests — a human-labor wall no startup can climb.

But walls made of *code volume* are exactly what AI is demolishing. So ask the question seriously: what would it take to replace the whole Python stack if AI builds the gaps?

**What AI changes: the code-volume barrier collapses.** Writing a provider adapter, a vector-store client, or a document loader is mostly mechanical — translate an API surface, handle edge cases, write tests. Exactly the high-volume, pattern-bound coding AI does well and cheaply. The "Python has 500 integrations" moat is a moat of *person-years*, and person-years are getting cheaper by the month.

**And Rust is the best AI-target language in existence.** This is the part people miss. When AI generates Python, you get code that *might* work — you find out at runtime, in production, maybe. When AI generates Rust, you get a free, relentless, infinitely patient verifier that rejects the garbage before it ever runs: the compiler. Every borrow-check error, every type mismatch, every unhandled `Result` is a caught hallucination. The feedback loop — generate, compile, fix, repeat — is exactly how AI coding agents converge, and Rust's compiler makes that loop rigorous instead of vibes-based. People are already pointing at agent-written Rust projects of startling size as [evidence that the compiler is a free training signal](https://github.com/MoonshotAI/kimi-cli/issues/2264). We're building on that bet: an AI generating integration adapters *against a typed trait with generated tests* will produce better adapters than a human copying patterns across 50 repos ever did.

**What AI does not change.** Let's stay honest:

- **Design taste.** AI can port an API; it cannot tell you whether the API was well-shaped, or which abstraction will survive three years of ecosystem drift.
- **Maintenance ownership.** Ten thousand generated lines are ten thousand lines someone must own, review, and answer for at 3 a.m.
- **Moving-target drift.** SaaS APIs change constantly. Generated adapters rot; you need CI that regenerates and re-verifies them forever, not a one-time port.
- **Network effects.** The Python agent community isn't just code — it's tutorials, Stack Overflow answers, notebook culture, and hiring pipelines. No compiler fixes that.

So here's our realistic phasing:

- **Phase 1 — done.** The Rust core. This is `agentgraph` today: the execution engine in Rust, a minimal `ChatModel` trait designed to wrap Rig or `async-openai`, plus a bundled OpenAI-compatible client. MCP interop and provider adapters are roadmap items, not shipped. The core is small enough for humans to own completely.
- **Phase 2 — the AI wedge.** AI-generated adapters for the top 50–100 integrations, each one gated by generated tests *and* the compiler — no merge without both. A PyO3 escape hatch absorbs the long tail: anything not yet ported stays callable from Python, so breadth is never blocked on the port queue.
- **Phase 3 — the steady state.** Python keeps the research and notebook surface, permanently. Exploration wants an interpreter; production wants a binary.

The model we end up with: **Python for research, Rust for production.** Not as a grudging compromise, but as a division of labor — with AI continuously draining the integration gap between them, and the Rust compiler keeping the drained result honest.

## Where we go from here

`agentgraph` is v0.1.0. The core is real and tested; the edges are where you come in. The roadmap:

- **Postgres checkpointer** (`sqlx`-backed, behind a feature flag) — the biggest durability gap, and a great first contribution.
- **PyO3 / napi-rs bindings** — call the Rust engine from Python and Node, directly implementing the hybrid this post argues for.
- **MCP / A2A interop** — speak to tool servers and other agents.
- **OpenTelemetry** — spans per super-step, node, and LLM call, following GenAI semantic conventions.
- **WASM target** — run graphs in the browser or at the edge.
- **Provider adapters** — thin `ChatModel` impls over Rig, `async-openai`, `genai`.

If you build agent systems in production, try it: three runnable examples (`react_agent`, `human_in_loop`, `parallel_fanout`), `cargo run --example` and you're in. If the Phase 2 argument above resonates, come argue with us in the issues — especially if you think we're wrong. And if you're a Rust person who's been waiting for agent infrastructure to take itself seriously as systems software: this is your invitation.

Agents grew up. Their runtime should too.

---

*`agentgraph` is dual-licensed MIT/Apache-2.0. The formal version of this argument, with full citations and benchmark methodology, lives in our whitepaper in the same repo.*
