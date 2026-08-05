# LangGraph Architecture Deep-Dive
## Research brief for "Rust for the Agentic Core Engine" / the `agentgraph` project

**Prepared:** 2026-07-31 · **Scope:** LangGraph (Python) core architecture, with brief contrast against CrewAI / AutoGen / OpenAI Agents SDK, and known pain points motivating a Rust re-implementation. Sources are primarily 2024–2026; publication dates noted where known.

---

## 1. Executive summary

LangGraph models agent workflows as **cyclic graphs over typed shared state**. Nodes are functions that read the full state and return *partial updates*; edges decide which nodes run next. Underneath the friendly `StateGraph` API sits a runtime explicitly modeled on **Google's Pregel / Bulk-Synchronous-Parallel (BSP)** model: execution proceeds in discrete **supersteps** (compute → barrier → route), with state held in versioned **channels** whose per-key **reducers** define merge semantics. Persistence (checkpointers), interrupts/human-in-the-loop, streaming, and time travel all fall out of "things that happen at a superstep boundary." ([LangGraph docs — Graph API overview](https://docs.langchain.com/oss/python/langgraph/graph-api); [Pratik Dhanave — "LangGraph Is a Pregel Program", Jul 2026](https://pratikdhanave.com/blog/posts/langgraph-01-message-passing-vs-shared-state.html); [INTERNALS.md — "Most people misunderstand LangGraph", Apr 2026](https://internals.laxmena.com/p/langgraph-internals-how-production))

The single most important architectural insight for a Rust port: **the public graph API is a thin builder; the real system is actors (`PregelNode`) + channels + a BSP scheduler + a versioned checkpoint store.** That substrate maps naturally onto Rust (`tokio` tasks, channels as typed state cells with reducer functions, `serde`-versioned snapshots).

---

## 2. StateGraph and typed state schema

- `StateGraph` is the main graph class, parameterized by a user-defined `State`. The `State` consists of a **schema** plus **reducer functions** specifying how node updates are applied. The schema is the input schema to all nodes and edges, and may be a `TypedDict`, a `dataclass` (for defaults), or a Pydantic `BaseModel` (for recursive validation — "though note that Pydantic is less performant than a TypedDict or dataclass"). ([Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api))
- Nodes do **not** return whole state; they return a **partial update** dict `{key: value}`. The framework merges it into shared state via per-key reducers. This is the "shared-state" model, contrasted with message-passing frameworks where a node emits a payload routed along an edge. ([Dhanave, Jul 2026](https://pratikdhanave.com/blog/posts/langgraph-01-message-passing-vs-shared-state.html))
- **Multiple schemas:** a graph can declare an internal "overall" schema plus `input_schema` / `output_schema` subsets; nodes may also read/write **private channels** (`PrivateState`) not in the input/output schema. Caveat: private channels are **not** redacted when streaming with `stream_mode="values"` — use `output_keys` to filter. ([Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api))
- `MessagesState` is a prebuilt state with one key, `messages: Annotated[list[AnyMessage], add_messages]`, meant to be subclassed. ([Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api))
- **Rust design note:** Python's dynamic `TypedDict` + `Annotated[..., reducer]` becomes, in Rust, a typed state struct where each field's merge semantics are expressed via a trait (e.g. `trait Reducer<V> { fn reduce(left: &V, right: V) -> V }`) or per-field attribute macro; compile-time typing eliminates the entire class of runtime schema errors.

## 3. Channels and reducers

Under the hood, **every state key is a channel**; a reducer is that channel's update function. Nodes don't call each other — they publish to channels; other nodes subscribe. "State is a set of channels, each with its own update semantics… This is message passing, not function calling." ([INTERNALS.md, Apr 2026](https://internals.laxmena.com/p/langgraph-internals-how-production))

**Built-in channel types** (confirmed by both official JS docs and community source-analysis):

| Channel | Behavior | Typical use |
|---|---|---|
| `LastValue` | Stores most recent value; **at most one write per superstep** (else `InvalidUpdateError`) | Simple overwritten fields |
| `BinaryOperatorAggregate` | Persistent value updated by applying a binary operator to current value + each update | Accumulation (counters, list append) |
| `Topic` | PubSub topic; can receive multiple values, broadcast to multiple subscribers; optional `accumulate` | Event streams, audit logs |
| `EphemeralValue` | Resets between supersteps | Temporary computation state |

([LangGraph JS docs — Channels API](https://docs.langchain.com/oss/javascript/langgraph/use-graph-api); [Pregel API reference (Mintlify mirror)](https://mintlify.com/langchain-ai/langgraph/api/pregel); [PyShine — LangGraph State Channels, May 2026](https://pyshine.com/LangGraph-Stateful-AI-Agent-Orchestration-Framework/))

**State-to-channel mapping** (from Python source analysis): `key: str` → `LastValue`; `key: Annotated[list, operator.add]` → `BinaryOperatorAggregate`; `Annotated[list, add_messages]` → append-with-ID-aware-overwrite semantics. Mapping logic lives in `graph/state.py::_create_channels`. ([Tink's Blog — LangGraph 源码剖析, Apr 2026](https://www.cyub.vip/blog/2026/04/18/langgraph-%E5%AE%8C%E5%85%A8%E6%8C%87%E5%8D%97%E4%BB%8E%E5%85%A5%E9%97%A8%E5%88%B0%E7%B2%BE%E9%80%9A%E4%B8%8E%E6%BA%90%E7%A0%81%E7%BA%A7%E5%8E%9F%E7%90%86%E5%89%96%E6%9E%90/); [CSDN 源码剖析 (二): Channel 与 Reducer](https://blog.csdn.net/qq_73472828/article/details/160875078))

**Reducer contract:** every reducer is a binary function `reduce(left=current_state[key], right=node_update[key])`. Default reducer discards `left` (overwrite). `add_messages` additionally (a) tracks message IDs so existing messages can be *updated* rather than appended, and (b) deserializes dict-shaped updates into LangChain `Message` objects. ([Graph API docs — Reducers](https://docs.langchain.com/oss/python/langgraph/graph-api))

**Production failure mode worth citing:** concurrent writes to a `LastValue` channel raise `InvalidUpdateError: Can receive only one value per step. Use an Annotated key to handle multiple values.` The silent variant — adding a parallel branch later to a previously single-path graph — is a recurring production bug; as of Nov 2025 the official `deepagentsjs` research example hit exactly this on its `todos` key, and CopilotKit shipped the identical fix in Aug 2025 (PR #2276). ([INTERNALS.md, Apr 2026](https://internals.laxmena.com/p/langgraph-internals-how-production))

## 4. Nodes and edges

- **Nodes** are Python functions (sync or async) receiving `state`, optionally `config: RunnableConfig` (thread_id, tags, tracing) and `runtime: Runtime` (context, store, stream_writer, execution_info, etc.). Functions are wrapped in `RunnableLambda` for batch/async/tracing support. `START` and `END` are virtual nodes marking entry/terminal edges. ([Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api))
- **Edge types:** normal edges (`add_edge`), **conditional edges** (`add_conditional_edges(node, routing_function[, path_map])`), entry point (`add_edge(START, node)`), and conditional entry point (`add_conditional_edges(START, fn)`). A routing function reads state and returns a node name, list of names (parallel fan-in to next superstep), or `END`. ([Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api); [MachineLearningPlus — Conditional Edges guide, Mar 2026](https://machinelearningplus.com/gen-ai/langgraph-conditional-edges-routing-decisions/))
- **Multiple outgoing normal edges = parallel execution** of all destination nodes in the next superstep. Official guidance: do **not** mix static edges and dynamic routing (`Command`/conditional edges) from the same node — both paths will execute. ([Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api))
- **`Command`** unifies state update + routing: `Command(update={...}, goto="node", graph=Command.PARENT)` from a node (or tool), and `Command(resume=value)` as *input* to `invoke`/`stream` to resume after an interrupt. Nodes returning `Command` must annotate `Command[Literal["target_node"]]` for graph rendering. `Command(resume=...)` is the only Command pattern valid as invoke input. ([Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api))
- **`compile()`** validates graph structure (orphaned nodes, conditional-edge targets), builds channel topology, constructs `PregelNode` actors, and freezes the graph — the returned `Pregel` instance is what actually runs. Errors are caught before any LLM call. ([INTERNALS.md](https://internals.laxmena.com/p/langgraph-internals-how-production); [Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api))
- **Recursion limit:** max supersteps per run; raises `GraphRecursionError`. Default is 1000 steps starting v1.0.6. `RemainingSteps` is a managed value letting nodes proactively degrade. ([Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api))

## 5. The Pregel-inspired super-step execution model

LangGraph's runtime is named `Pregel` after Google's Pregel system, an implementation of **Bulk Synchronous Parallel**. One superstep:

1. **Plan/Compute** — all active nodes run concurrently, each reading the state as of the *start* of the step (immutable snapshot; no node sees another's in-progress work).
2. **Barrier** — engine waits for all active nodes; a superstep is **transactional** — if any actor raises, the entire step's writes are discarded.
3. **Update/Route** — writes merge into channels via reducers; the engine determines next-step active nodes from edges.

Nodes become active when a subscribed channel receives new data; the loop ends when all nodes vote to halt (no incoming messages). The barrier is what makes shared state safe: same-step nodes can never observe each other's partial writes, so parallelism of independent nodes is free and deterministic. ([Dhanave, Jul 2026](https://pratikdhanave.com/blog/posts/langgraph-01-message-passing-vs-shared-state.html); [INTERNALS.md, Apr 2026](https://internals.laxmena.com/p/langgraph-internals-how-production); [Graph API docs — super-step description](https://docs.langchain.com/oss/python/langgraph/graph-api))

- A graph **cycle** (e.g. the ReAct loop `agent → tools → agent`) is not call-stack recursion — it's nodes being re-scheduled across supersteps, which is why the guard is a superstep `recursion_limit`, not a stack limit. ([Dhanave](https://pratikdhanave.com/blog/posts/langgraph-01-message-passing-vs-shared-state.html))
- **Rust design note:** BSP supersteps map directly to an async scheduler: each superstep = `JoinSet`/`FuturesUnordered` of node futures over an immutable state snapshot, then a barrier + reduce phase. Rust's ownership model *statically guarantees* the snapshot isolation Python only achieves by convention (copy-on-read).

## 6. Checkpointer interface and thread-scoped persistence

- **Two persistence systems:** **Checkpointers** (thread-scoped graph state snapshots: conversation continuity, HITL, time travel, fault tolerance) and **Stores** (cross-thread long-term memory: user preferences, facts). ([Persistence docs](https://docs.langchain.com/oss/python/langgraph/persistence))
- Every checkpointer conforms to `BaseCheckpointSaver` with methods: `put` (store a checkpoint with config + metadata), `get_tuple`, `list`, and `put_writes` (store pending writes from nodes that succeeded while a sibling failed — enabling partial-failure resume where only the failed node re-runs). ([Checkpointers docs](https://docs.langchain.com/oss/python/langgraph/checkpointers); [LangGraph checkpoints reference](https://reference.langchain.com/python/langgraph/checkpoints))
- A `Checkpoint` is a versioned snapshot: `v` (format version), `id` (monotonic UUID6, time-sortable), `ts`, `channel_values`, `channel_versions`, `versions_seen` (per-node-per-channel map driving "what's ready"), plus metadata with `source` (`input`/`loop`/`update`/`fork`), `step`, and `parents` (namespace → checkpoint_id, supporting subgraphs). ([dbrowneup/Linus repo notes on LangGraph checkpoint internals](https://github.com/dbrowneup/Linus/blob/main/docs/repo-notes/langgraph.md))
- `thread_id` namespaces a session: one agent session = one thread; threads cannot see each other's state. Checkpointing happens **at superstep boundaries, not mid-node** — so on resume, the affected node re-runs from its start; node logic must be idempotent. ([INTERNALS.md](https://internals.laxmena.com/p/langgraph-internals-how-production); [Graph API docs — re-execution and idempotency](https://docs.langchain.com/oss/python/langgraph/graph-api))
- Implementations: `InMemorySaver`/`MemorySaver` (RAM, lost on restart — dev only), `SqliteSaver` (local file, dev), `PostgresSaver`/`AsyncPostgresSaver` (production; `thread_id` column limited — keep under 255 chars). Known operational issue: checkpoints grow unboundedly and need pruning/retention. ([Persistence docs — troubleshooting](https://docs.langchain.com/oss/python/langgraph/persistence))
- One primitive, four use cases: durable execution (crash → resume), human-in-the-loop (interrupt → serialize → approve → resume), time travel (load any historical checkpoint, fork alternate paths), partial-failure recovery (pending writes). ([INTERNALS.md](https://internals.laxmena.com/p/langgraph-internals-how-production))
- **Graph migrations:** topology can change freely for finished threads; interrupted threads tolerate all changes except renaming/removing nodes. Adding/removing state keys is backward/forward compatible; renamed keys lose saved state. ([Graph API docs — graph migrations](https://docs.langchain.com/oss/python/langgraph/graph-api))

## 7. Interrupts and human-in-the-loop

- `interrupt(payload)` called inside a node **pauses the graph**, persists state via the checkpointer (required), and surfaces the payload to the caller (as `__interrupt__` in the result). Resuming with `Command(resume=value)` makes the `interrupt()` call *return* `value` inside the node — the node function re-executes from its start, so side effects before `interrupt()` must be idempotent. ([Interrupts docs](https://docs.langchain.com/oss/python/langgraph/interrupts); [Graph API docs](https://docs.langchain.com/oss/python/langgraph/graph-api); [Univ. of Padua thesis on agentic frameworks (academic cross-check)](https://thesis.unipd.it/retrieve/ecc36f73-ac30-440c-8e89-6df0408d050c/Russo_Christian_Francesco.pdf))
- Requirements for resume: checkpointer + stable `thread_id`. ([mcpservers.org — langgraph-human-in-the-loop skill summary](https://mcpservers.org/agent-skills/langchain-ai/langgraph-human-in-the-loop))
- Static breakpoints (`interrupt_before`/`interrupt_after` at compile or invoke time) complement dynamic `interrupt()`.

## 8. The Send API — dynamic fan-out / map-reduce

- `Send(node_name, state)` returned from a conditional edge lets routing create **data-dependent, per-item edges** not known at graph-build time — the canonical map-reduce pattern: a node generates a list; each item gets its own node invocation with its own scoped input state; results fan back in through reducers. ([Graph API docs — Send](https://docs.langchain.com/oss/python/langgraph/graph-api); [LangChain Forum — supervisor parallel execution, Sep 2025](https://forum.langchain.com/t/parallel-execution-with-supervisor-pattern/1665))
- Real-world internal usage: in `create_react_agent` `version="v2"`, each tool call in an AI message is dispatched to a separate `ToolNode` invocation via `Send`, vs v1's batched single `ToolNode` call. ([DeepWiki — create_react_agent, citing chat_agent_executor.py](https://deepwiki.com/langchain-ai/langgraph/8.1-project-setup))

## 9. Streaming modes

LangGraph exposes execution through `stream`/`astream` with composable modes:

| Mode | Emits |
|---|---|
| `values` | Full state snapshot after each superstep |
| `updates` | Per-node state deltas after each step |
| `messages` | LLM token chunks + node metadata (for chat UIs) |
| `custom` | User-defined events from nodes via `StreamWriter` |
| `checkpoints` | State snapshots |
| `tasks` | Task execution events |
| `debug` | Max-detail debug events |

Modes can be combined (`stream_mode=["values","updates"]`). ([LangGraph streaming docs](https://docs.langchain.com/oss/python/langgraph/streaming); [Mintlify streaming concepts mirror, Mar 2026](https://mintlify.com/langchain-ai/langgraph/concepts/streaming); [LobeHub skill summary, Mar 2026](https://lobehub.com/nl/skills/neversight-skills_feed-langgraph-streaming))

- **Rust design note:** these modes are all views over the same superstep event stream — in Rust a single `tokio::sync::broadcast` (or `async_stream`) of typed `GraphEvent` enums, with mode-based filtering at the subscriber, reproduces all seven modes.

## 10. Subgraphs

- A compiled graph can be added as a node of a parent graph. Subgraphs maintain their own checkpoint namespace (checkpoint `parents` map namespace → checkpoint_id). Shared keys flow between parent and subgraph when schemas overlap; **when a subgraph node routes to a parent node via `Command(goto=..., graph=Command.PARENT)` and updates a shared key, the parent must define a reducer for that key.** ([Graph API docs — Command/graph](https://docs.langchain.com/oss/python/langgraph/graph-api); [dbrowneup checkpoint notes](https://github.com/dbrowneup/Linus/blob/main/docs/repo-notes/langgraph.md))
- Known gotcha: because each subgraph has its own checkpoint namespace, parent graphs may not immediately see subgraph state changes; the recommended fix is a Store for cross-graph data. ([Persistence docs — troubleshooting](https://docs.langchain.com/oss/python/langgraph/persistence))

## 11. Prebuilt ReAct agent (`create_react_agent`)

- Factory in `langgraph-prebuilt` building a `StateGraph` with an `agent` node (calls the tool-bound chat model) and a `tools` node (`ToolNode`), wired by a `should_continue` conditional edge: if the last AI message has `tool_calls` → route to tools; else → END (or a `generate_structured_response` node when `response_format` is set). Optional `pre_model_hook` / `post_model_hook` nodes insert context trimming or HITL approval around the model call. ([DeepWiki — create_react_agent internals](https://deepwiki.com/langchain-ai/langgraph/8.1-project-setup); [langgraph-prebuilt on PyPI](https://pypi.org/project/langgraph-prebuilt/))
- Default state `AgentState` = `messages` (with `add_messages` reducer) + `remaining_steps` (managed channel; when exhausted the agent returns "Sorry, need more steps to process this request."). Chat history is validated (every tool_call must have a ToolMessage). ([DeepWiki](https://deepwiki.com/langchain-ai/langgraph/8.1-project-setup))
- Note: `create_react_agent` is now **deprecated in favor of `create_agent`** (langchain v1). ([LangChain reference docs](https://reference.langchain.com/python/langgraph.prebuilt/chat_agent_executor/create_react_agent))
- For `agentgraph`: the entire ReAct agent is ~3 nodes + 1 conditional edge over a messages channel — an ideal reference implementation/test case proving parity with the Rust core.

## 12. Contrast: CrewAI, AutoGen, OpenAI Agents SDK

| Framework | Core abstraction | Control flow | State/persistence |
|---|---|---|---|
| **LangGraph** | Typed-state graph; nodes/edges over channels | Explicit graph, cycles, Pregel supersteps | First-class checkpointers, interrupts, time travel |
| **CrewAI** | Role-based "crews" of agents with tasks | Sequential / hierarchical process orchestration | Minimal; no durable execution model |
| **AutoGen (AG2)** | Conversational agents exchanging messages | Conversation patterns / agent topologies | Conversation-centric, no superstep/checkpoint substrate |
| **OpenAI Agents SDK** | Lightweight primitives: agents, handoffs, guardrails | Simple Python-based loops with handoffs | Sessions; thinnest layer, tied to OpenAI ecosystem |

LangGraph "excels in communication via graph edges… its graph structure makes parallel execution smoother"; CrewAI is "the most approachable way to build role-based multi-agent teams"; OpenAI Agents SDK is the "fastest path from idea to running agent" but "lacks built-in parallel execution." ([Composio comparison](https://composio.dev/blog/openai-agents-sdk-vs-langgraph-vs-autogen-vs-crewai); [Langfuse framework comparison](https://langfuse.com/blog/2025-03-19-ai-agent-comparison); [theLLMs framework comparison, May 2026](https://thellms.dev/diff/llm-agent-frameworks-compared-langchain-crewai-autogen-openai-agents-sdk/); [Bizz comparison, Jul 2026](https://www.bizz.ai/blog/openai-agents-sdk-vs-langgraph-vs-crewai-vs-autogen-vs-semantic-kernel/))

An independent 2026 engineering comparison rated LangChain/LangGraph and CrewAI/AutoGen as lacking built-in production-grade state persistence and DAG support respectively — reinforcing that LangGraph's checkpoint substrate is its key differentiator, and its complexity is the price. ([toolsku.com — building an agent workflow engine, Jun 2026](https://www.toolsku.com/en/blog/ai-agent-workflow-engine-2026/))

## 13. Known pain points (the case for a Rust core)

1. **Python overhead & the GIL.** Superstep parallelism is asyncio/task-based; CPU-bound node work and serde contend on the GIL. The broader ecosystem trend shows the pattern: ChromaDB's 2025–2026 Rust-core rewrite "eliminates Python GIL bottlenecks, delivering up to 4x performance improvement for writes and queries." ([jitendrazaa.com, Feb 2026](https://www.jitendrazaa.com/blog/salesforce/talk-to-salesforce-data-using-openai-langchain-chroma/))
2. **Checkpoint serialization bloat.** A reproducible study (GitHub issue #7714, May 2026) measured LangGraph checkpoint serialization producing **85% storage bloat and 37.8% token overhead** with no opt-out path, on a 16-turn ReAct agent (65 messages). ([langgraph issue #7714](https://github.com/langchain-ai/langgraph/issues/7714))
3. **Debugging complexity / opacity.** "LangGraph's debugging model differs from standard Python" — state flows through channels and supersteps, not call stacks, making failures harder to trace. ([Kalvium Labs, May 2026](https://www.kalviumlabs.ai/blog/langgraph-for-founders-agent-framework-pays-back/)) The canonical ecosystem critique — Octomind's "Why we no longer use LangChain" (12 months of production use, then removed, citing high-level abstractions and debugging difficulty) — is widely cited as the watershed. ([Octomind via skywork.ai summary, Oct 2025](https://skywork.ai/skypage/en/octomind-great-migration-teams-langchain/1976832104900653056); [tianpan.co, Apr 2026](https://tianpan.co/blog/2026-04-19-orchestration-framework-trap-langchain-production))
4. **Dependency weight / bloat perception.** Ongoing community sentiment that the LangChain stack is bloated for simple applications; many teams prefer vanilla SDK calls. ([GitHub community discussion #182015, Dec 2025](https://github.com/orgs/community/discussions/182015))
5. **Runtime fragility of the dynamic schema model.** The `InvalidUpdateError` concurrent-write class of bugs (Section 3) exists precisely because channel/reducer semantics are checked at runtime, not compile time — a Rust type system eliminates it statically. ([INTERNALS.md](https://internals.laxmena.com/p/langgraph-internals-how-production))
6. **Operational toil:** checkpoints grow unboundedly (need pruning cron jobs); `MemorySaver` loses everything on restart; subgraph checkpoint namespaces surprise users. ([Persistence docs](https://docs.langchain.com/oss/python/langgraph/persistence))

---

## Key takeaways for the whitepaper

- **The kernel is small and portable.** LangGraph's essence is: typed channels + reducers, a BSP superstep scheduler (plan → parallel compute → barrier → reduce/route), a versioned checkpoint log keyed by `thread_id`, and an interrupt/resume protocol. Everything else (Send, streaming modes, subgraphs, prebuilt ReAct) is composition over these primitives. That kernel is language-agnostic and an excellent fit for Rust's ownership, trait, and async-task model.
- **Rust upgrades LangGraph's runtime guarantees to compile-time guarantees.** TypedDict + `Annotated` reducers → typed state structs with reducer traits; runtime `InvalidUpdateError` → compile-time merge-semantics checking; copy-based snapshot isolation → borrow-checker-guaranteed immutability during a superstep.
- **BSP maps directly onto tokio.** Superstep = a barriered batch of node futures over an immutable state snapshot; deterministic, data-parallel, and free of the GIL. Checkpoint = `serde`-versioned snapshot written at each barrier — with an opportunity to fix the measured 85% storage bloat via compact binary encoding and delta checkpoints.
- **Interrupts/resume are a serialization problem, not a control-flow hack** — in Rust, model them as explicit `Suspend(payload)` / `Resume(value)` variants in the execution state machine, with idempotency documented as a node contract.
- **The ReAct agent is the parity benchmark:** agent node + tool node + `should_continue` edge + `add_messages` reducer — roughly a day-one example for `agentgraph`, and directly comparable against `create_react_agent`/`create_agent`.
- **Positioning:** CrewAI/AutoGen/OpenAI Agents SDK optimize for ergonomics (roles, conversations, handoffs); LangGraph is the only one with a durable-execution substrate — but pays for it in Python overhead, GIL-bound parallelism, debugging opacity, and dependency weight. A Rust agentic core keeps the substrate and removes the tax.
