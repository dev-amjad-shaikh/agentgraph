# agentgraph examples

Four end-to-end examples, ordered from "no network required" to "real LLM
endpoint". Run any of them from the crate root:

```text
cd agentgraph
cargo run --example <name>
```

| Example | Run command | What it demonstrates |
|---|---|---|
| `react_agent` | `cargo run --example react_agent` | The prebuilt ReAct agent (`create_react_agent`) with a **scripted mock model** — the full agent ⇄ tools loop (parallel tool calls, tool results, final answer) plus the `GraphEvent` stream, with zero network access. |
| `parallel_fanout` | `cargo run --example parallel_fanout` | Dynamic fan-out / fan-in (map-reduce) via the `Send` API: a router emits one `Send` per item at runtime, workers run in parallel in a single super-step, and `Reducer::Append` merges results behind the barrier before a `summarize` node. |
| `human_in_loop` | `cargo run --example human_in_loop` | Interrupt / resume with durable checkpoints: an `approve` node suspends the run via `NodeContext::interrupt`, a checkpoint is persisted to JSON files, and the run resumes from the same `thread_id` with the human's decision. |
| `live_agent` | `cargo run --example live_agent` | **The hero live demo** — a real ReAct agent against any OpenAI-compatible endpoint (Ollama, OpenAI, vLLM, LM Studio) with three real tools (`get_current_time`, `calculator`, `word_count`) and a live pretty-printed `GraphEvent` stream. Graceful exit 0 with setup instructions when no endpoint is reachable (CI-safe). |

The first three examples are fully offline and deterministic; only
`live_agent` needs a running model server.

## `live_agent` configuration

The live demo reads three environment variables:

| Variable | Default | Notes |
|---|---|---|
| `AGENTGRAPH_BASE_URL` | `http://localhost:11434/v1` | Any OpenAI-compatible `/v1` base URL |
| `AGENTGRAPH_API_KEY` | `ollama` | Any string works for Ollama; real key for OpenAI |
| `AGENTGRAPH_MODEL` | `llama3.1` | Must support tool calling |

### Option A — Ollama (local, free)

```text
ollama pull llama3.1
ollama serve                       # listens on http://localhost:11434/v1
cargo run --example live_agent
```

### Option B — OpenAI

```text
AGENTGRAPH_BASE_URL=https://api.openai.com/v1 \
AGENTGRAPH_API_KEY=sk-... \
AGENTGRAPH_MODEL=gpt-4o-mini \
cargo run --example live_agent
```

### Option C — vLLM / LM Studio

Point `AGENTGRAPH_BASE_URL` at the server's `/v1` path and set
`AGENTGRAPH_MODEL` to the served model name, e.g.:

```text
AGENTGRAPH_BASE_URL=http://localhost:1234/v1 \
AGENTGRAPH_MODEL=local-model \
cargo run --example live_agent
```

If no endpoint answers, `live_agent` prints these same setup instructions
and exits with status 0 — it never panics, so it is safe to run in CI.
