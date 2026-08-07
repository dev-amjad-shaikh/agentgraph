# Content Brief — Quickstart, Studio & Live Demo (R2_Quickstart)

**Purpose:** Source-faithful facts for the aboutrusty.com landing site covering (1) the 10-minute zero-to-served-graph flow, (2) interrupt/resume over HTTP, (3) Rusty Studio, and (4) live demo highlights usable as website demo-script beats.
**Sources:** `docs/server-quickstart.md`, `docs/studio.md`, `docs/live-demo-transcript.md`. Nothing below is invented; commands and curl examples are verbatim.

---

## 1. The Ten-Minute Quickstart — Zero to a Served Agent Graph

### One-line pitch
> "Rusty Server quickstart — 10 minutes to a served agent graph"

**Prerequisites (verbatim):** "a Rust toolchain (`rustup`), `curl`, and ~10 minutes. No Docker, no database, no Redis — everything runs in one process."

**Memorable tagline (use as a pull-quote):**
> **"Cargo.toml is the new langgraph.json."** — "There is no config file declaring your graphs — the declaration is your `main.rs`, checked by the compiler."

### The flow at a glance (5 steps, time-boxed in the doc)

| Step | Time | What happens |
|---|---|---|
| 1. Create the project | 1 min | `cargo new agent-server-demo`; add path deps |
| 2. Define the graph | 3 min | Two-node graph `draft → approve` with a human-in-the-loop interrupt |
| 3. Run it | 1 min | `cargo run`; server on `localhost:8080`, dev mode, no API key |
| 4–6. Create thread → run to interrupt → resume over SSE | ~4 min | All via curl |
| 7–8. Time travel | 2 min | Checkpoint history, rollback, fork & replay |

### Step 1 — Create the project (verbatim)

```bash
cargo new agent-server-demo
cd agent-server-demo
```

`Cargo.toml`:

```toml
[dependencies]
rusty-agent-runtime = { path = "../rusty-core" }
rusty-server = { path = "../rusty-server" }
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

### Step 2 — The graph (key facts, full code in source doc §2)

- State schema: one channel per key, each with a reducer — `draft` and `approval`, both `Reducer::Overwrite` ("each channel here has exactly one writer node, so Overwrite (last-value) semantics are correct everywhere").
- Two nodes: `draft` (writes a draft) and `approve` (human-in-the-loop).
- **Canonical interrupt/resume pattern (verbatim rule):** "check `ctx.resume_value()` FIRST; only interrupt when there is none." Phase 1: no decision → `ctx.interrupt(...)` suspends the run and the payload surfaces to the HTTP caller as the run's `interrupt` value. Phase 2: decision arrives via `command.resume`.
- `builder.compile()?` — "validates topology before serving anything."
- `GraphRegistry` is described as "the Rust analog of langgraph.json's `graphs` map. A name plus the two things the executor needs — Graph + StateSpec."
- Server: `ServerConfig::new("0.0.0.0:8080".parse()?, "./data/checkpoints")` — bind address and checkpoint store directory are code; a `JsonFileCheckpointer` rooted at `store_path` is wired in automatically. `serve(registry, config).await?` blocks on the axum/tokio runtime.
- **API surface fact:** the server crate adds "exactly three names you call directly: `GraphRegistry`, `ServerConfig`, `serve` (plus `router` if you want to embed the routes in a larger axum app)." Everything else is the same `rusty-agent-runtime` API as the library examples.

### Step 3 — Run + health checks (verbatim)

```bash
cargo run
# listening on http://localhost:8080 (dev mode: no API key set)

curl localhost:8080/ok
# {"ok":true}

curl localhost:8080/info
# {"service":"rusty-server","version":"0.4.0","checkpointer":"json_file",
#  "store_path":"./data/checkpoints",
#  "graphs":[{"channels":["approval","draft"],"name":"publisher"}]}
```

Auth note: `ServerConfig::new(…).with_api_key("secret")` → add `-H "X-Api-Key: secret"` to every request.

### Step 4 — Create a thread (verbatim)

"A thread binds to one registered graph at creation":

```bash
curl -s -X POST localhost:8080/threads \
  -H 'Content-Type: application/json' \
  -d '{"graph": "publisher", "metadata": {"owner": "quickstart"}}'
# 201 {"thread_id": "3f2b9c4e-…", "graph": "publisher",
#      "metadata": {"owner": "quickstart"}, "created_at": "2026-08-05T…Z"}

TID=<paste the thread_id here>
```

### Step 5 — Run to the interrupt (verbatim)

`POST /threads/{id}/runs/wait` "blocks until the run finishes — or suspends":

```bash
curl -s -X POST localhost:8080/threads/$TID/runs/wait \
  -H 'Content-Type: application/json' \
  -d '{"input": {"draft": "Rust agents, one binary."}}'
```

Terminal JSON response:

```json
{
  "run_id": "7c1e…",
  "thread_id": "3f2b9c4e-…",
  "status": "interrupted",
  "interrupt": {
    "kind": "approval_request",
    "prompt": "Approve this draft for publication?",
    "draft": "Rust agents, one binary."
  },
  "checkpoint_id": "a94f…",
  "state": { "draft": "Rust agents, one binary." }
}
```

**Durability pull-quote:** "The run suspended inside `approve`, and the executor persisted a checkpoint for the thread — this is durable, so you could restart the server right now and lose nothing." (Caveat: thread records live in memory; checkpoints are durable on disk — re-create the thread with the same `thread_id` after a restart.)

Inspect parked state:

```bash
curl -s localhost:8080/threads/$TID/state
# {"values": {"draft": "Rust agents, one binary."}, "next": ["approve"],
#  "checkpoint": {"checkpoint_id": "a94f…", "thread_id": "3f2b…", "step": 1,
#                 "created_at": "…"}}
```

> "`next: ["approve"]` tells you exactly where the run is parked."

---

## 2. Interrupt/Resume Over HTTP — The Walkthrough

### Resume, streamed over SSE (verbatim)

Resume with the human's decision via `command.resume` (`-N` disables curl buffering — required for SSE):

```bash
curl -N -X POST localhost:8080/threads/$TID/runs/stream \
  -H 'Content-Type: application/json' \
  -d '{"command": {"resume": {"approved": true, "reviewer": "alice"}},
       "stream_mode": ["updates", "values"]}'
```

SSE frames (verbatim sample; frame ids are `{checkpoint_id}:{step}:{seq}`; frames before the run's first checkpoint use `-` as the checkpoint component):

```text
event: metadata
id: -:0:1
data: {"run_id": "…", "thread_id": "3f2b9c4e-…", "graph": "publisher",
       "attempt": 2, "metadata": null}

event: updates
id: a94f…:2:2
data: {"step": 2, "updates": {"approve": {"approval": {"approved": true, "reviewer": "alice"}}}}

event: values
id: a94f…:2:3
data: {"draft": "Rust agents, one binary.", "approval": {"approved": true, "reviewer": "alice"}}

event: end
id: a94f…:2:4
data: {"status": "success"}
```

**What actually happens on resume (verbatim):** "The executor restored the checkpoint, re-executed `approve` from its start with `ctx.resume_value()` set (this is why node logic must be idempotent), and the run completed."

### Last-Event-ID dedup (resumable streams)

- Every SSE frame carries `id: {checkpoint_id}:{step}:{seq}` where `seq` is a per-run monotonically increasing sequence number.
- A reconnecting client sends `Last-Event-ID`; the server skips frames already seen, replaying the run's in-memory event-log tail (capacity: `ServerConfig::event_log_capacity`, **default 1000 frames**) before streaming live frames.

```bash
curl -N -X POST localhost:8080/threads/$TID/runs/stream \
  -H 'Content-Type: application/json' \
  -H 'Last-Event-ID: a94f…:2:2' \
  -d '{"command": {"resume": {"approved": true}}, "stream_mode": ["updates", "values"]}'
```

### Time travel — history, rollback, fork & replay (verbatim commands)

Checkpoint history (newest first):

```bash
curl -s -X POST localhost:8080/threads/$TID/history \
  -H 'Content-Type: application/json' \
  -d '{"limit": 10}'
```

Rollback (undo a finished run — deletes its checkpoints, re-anchors the thread to the pre-run checkpoint):

```bash
curl -s -X DELETE localhost:8080/threads/$TID/runs/$RUN_ID
# {"run_id": "…", "thread_id": "…", "deleted_checkpoints": 2,
#  "remaining_checkpoints": 1}
```

Fork & replay — "Rollback rewinds a thread; fork branches it":

```bash
# Pick an earlier checkpoint from the history listing
CP_ID=<a checkpoint_id from the history above>

# Fork the thread at that checkpoint (omit checkpoint_id for a full-history fork)
curl -s -X POST localhost:8080/threads/$TID/fork \
  -H 'Content-Type: application/json' \
  -d '{"new_thread_id": "branch-a", "checkpoint_id": "'$CP_ID'"}'
# 201 {"thread_id": "branch-a", "checkpoints_copied": 1}

# Replay the run from the same checkpoint, on the fork
curl -s -X POST localhost:8080/threads/branch-a/runs/wait \
  -H 'Content-Type: application/json' \
  -d '{"checkpoint": {"checkpoint_id": "'$CP_ID'"}}'
```

**Safe-pattern guidance (verbatim):** "The safe pattern is fork first, replay on the fork: the branch gets its own thread id and its own history, while replaying on the original thread appends new checkpoints on top of the old timeline (supported, but rarely what you want)."
**Error semantics:** `404` unknown thread/checkpoint id, `400` source thread has no checkpoints to copy, `409` `new_thread_id` already taken.

### Extra endpoints worth mentioning on the site

- Background runs: `POST /threads/{id}/runs` → `202` + `run_id` immediately; concurrency control via `"multitask_strategy": "enqueue" | "reject"`.
- Serve a real LLM agent: register `create_react_agent(model, tools)` the same way, with `OpenAiCompatibleClient` (e.g. `OpenAiCompatibleClient::from_env("https://api.openai.com/v1", "OPENAI_API_KEY", "gpt-4o-mini")`). Add `"messages"` to `stream_mode` to stream LLM token deltas when the node uses `ChatModel::chat_stream`.
- Deploy: `cargo build --release` produces **one static binary**; a `FROM scratch` Dockerfile exists in the rusty-server README.

---

## 3. Rusty Studio — The Zero-Build Debug UI

### Positioning (verbatim)

> "A **zero-build, single-file debug UI** for `rusty-server`. One HTML file, vanilla JS + CSS, no npm, no framework, no bundler — open it and point it at a running server."

File layout (verbatim):

```
studio/
├── index.html   ← the entire UI (open this)
└── serve.py     ← optional same-origin static host + API proxy
```

### Capability checklist (for feature cards)

- **Connect bar** — server base URL (default `http://127.0.0.1:8100`) + optional API key (`X-Api-Key` header). Connect calls `GET /info` and shows service version, checkpointer kind, and every registered graph with channel names. URL, key, and thread list persist in `localStorage`.
- **Graphs panel** — one card per registered graph, each with a **New thread** button (`POST /threads`).
- **Threads panel (local-only)** — server API (as of v0.4) has **no list-threads endpoint**, so threads live in the browser, keyed by server URL. **Attach by id** re-connects a thread the server already knows (and can re-create it with the same id after a server restart so on-disk checkpoints re-attach). ✕ *forget* only removes the local entry; nothing is deleted server-side.
- **Per-thread workspace:**
  - **Current state** — `GET /threads/{id}/state`, pretty-printed JSON grouped by channel, with `next` nodes and the current checkpoint ref (step, id, timestamp).
  - **Checkpoint history** — `POST /threads/{id}/history` as a newest-first clickable timeline (step, timestamp, checkpoint id, next nodes).
  - **Run (background)** — `POST /threads/{id}/runs` + live-polls `GET /runs/{run_id}` with a pulsing status badge until terminal state.
  - **Run & wait** — `POST /threads/{id}/runs/wait`; terminal JSON (`output` / `interrupt` / `error`) rendered as a result card.
  - **Stream run** — `POST /threads/{id}/runs/stream` via `fetch` + `ReadableStream` (because "EventSource can't POST"), rendered as a live colored event feed: `metadata` (grey), `updates` (amber), `values` (sage), `messages` (clay), `error` (red), `end` (rust) — with each frame's `{checkpoint}:{step}:{seq}` id. `stream_mode` checkboxes and `multitask_strategy` selector map straight onto the run payload.
  - **Fork at a checkpoint** — calls the real `POST /threads/{id}/fork` with `{new_thread_id, checkpoint_id}`; server copies history up to and including the selected checkpoint into a new thread (`{thread_id}-fork-{step}`), returning `201 {thread_id, checkpoints_copied}`.
  - **Replay & run from a checkpoint** — background run whose payload carries `"checkpoint": {"checkpoint_id": …}`; executor replays from that checkpoint's state and next-node set. UI itself advises: "Prefer replaying on a fork."
  - **Interrupt/resume helper** — when a run ends `interrupted`, the interrupt payload is shown with a resume input; the value goes back as `{"command": {"resume": <value>}}` (parsed as JSON when possible), via *wait* or *stream*.
- **Status badges** — `pending` / `running` / `success` / `interrupted` / `error`.

### How the zero-build UI works (three ways to open)

1. **Option A — `serve.py` (same-origin static host + proxy):**

```bash
# terminal 1: the demo server
cargo run --example server_demo          # http://127.0.0.1:8100

# terminal 2: the studio
python3 studio/serve.py                  # http://127.0.0.1:8000/
```

Open `http://127.0.0.1:8000/`, connect with base URL **`/api`** (the proxy forwards `/api/*` to `127.0.0.1:8100`; it also flushes SSE per chunk and sets `X-Accel-Buffering: no` so streams render live).

2. **Option B — any static host:** `cd studio && python3 -m http.server 8000` → connect to `http://127.0.0.1:8100`. Works cross-origin because `rusty-server` v0.3+ sends permissive CORS headers.
3. **Option C — double-click `index.html` (file://):** "Works too: the page runs from `file://` (origin `null`) and the server's permissive CORS layer answers those cross-origin calls as well."

**CORS fact (verbatim):** `rusty-server` v0.3+ layers `tower_http::cors::CorsLayer::permissive()` as the outermost middleware — every response carries `access-control-allow-origin: *`, and OPTIONS preflights are answered before the API-key middleware. "**Production deployments should restrict this** (the permissive layer is a dev convenience)."

### Studio limitations (honesty box — good for docs, not hero copy)

- Thread list is local-only (no `GET /threads` server-side as of v0.4); server restarts drop the in-memory thread registry — **Attach** re-creates a thread with the same id to re-attach to on-disk checkpoints.
- Replay on the original thread appends history (checkpoint history is append-only); fork first to branch. Rollback (`DELETE /threads/{id}/runs/{run_id}`) is **not** exposed in the UI.
- Pre-v0.3 servers: fork falls back to client-side composition; replay `checkpoint` field is silently ignored — "upgrade the server for real replay."
- SSE resume (`Last-Event-ID`) is implemented server-side but not surfaced in the UI.
- Single-process server with in-memory run registry: background-run polling 404s for runs created before a server restart.
- The demo (`examples/server_demo`) registers two graphs on `127.0.0.1:8100`: `pipeline` (channel `log`, nodes `first → second`, no network) and `react_agent` (channel `messages`, scripted model + echo tool, no network).

---

## 4. Live Demo Transcript — Website Demo-Script Beats

**Source:** `docs/live-demo-transcript.md` — "First Real End-to-End LLM Run", dated 2026-08-05 (PDT). Verdict (verbatim): "✅ **Real LLM — YES.** The `live_agent` example ran against a live Ollama endpoint, completed the ReAct loop (`agent ⇄ tools`), executed real tool calls, and produced a correct final answer."

### Setup facts (for credibility sidebar)

| Item | Value |
|---|---|
| Endpoint | `http://localhost:11434/v1` (Ollama OpenAI-compatible shim) |
| Models | `qwen2.5:0.5b` (397 MB), `llama3.2:latest` (2.0 GB, 3B) |
| Command | `RUSTY_MODEL=<model> cargo run --example live_agent` |
| Run config | `RunConfig::new("live-demo")`, `max_steps = 12` |
| Demo prompt (hardcoded) | "What time is it right now in UTC? Then multiply 128 by 46, and count the words in 'the quick brown fox jumps over the lazy dog'." |
| Ground truth | `128 × 46 = 5888`; the pangram has **9 words** (43 characters, 1 line) |

### Beat-by-beat story arc (three runs = a ready-made narrative)

**Beat 1 — "It works with a real model, no code changes."** Run 1 (`qwen2.5:0.5b`, cold, 23 s): the ReAct loop itself worked perfectly — super-steps, node start/end, state merges on the `messages` channel, clean `Done` termination — but the tiny 0.5B model mis-called tools and hallucinated answers (5952 ≠ 5888; 37 ≠ 9 words). Honest assessment line: "graph/runtime ✅, tool-calling reliability of qwen2.5:0.5b ❌."

**Beat 2 — "Correct end-to-end answer, traceable to real tool output."** Run 2 (`llama3.2:latest`, cold, 19 s): final answer fully correct — "The current time in UTC is 2026-08-06 06:42:24. 128 × 46 = 5888. The phrase … contains 9 words." Note from the doc: "the time answer is traceable to the real tool output — not hallucinated."

**Beat 3 — "Warm model, all three tools fire in one parallel batch, 2 seconds."** Run 3 (`llama3.2:latest`, warm, **2 s**): all three tools fired in a single parallel tool batch in step 1 — "the full intended ReAct behavior." `word_count` returned exact ground truth: `{"characters":43,"lines":1,"words":9}`.

### Latency table (great for a "why Rust" / performance section)

| Run | Model | Wall clock | Notes |
|---|---|---|---|
| 1 | qwen2.5:0.5b | 23 s | cold start |
| 2 | llama3.2 (3B) | 19 s | cold start; model load dominates |
| 3 | llama3.2 (3B) | **2 s** | warm — model resident in RAM |

Key observation (verbatim): "The graph overhead itself (2 nodes, super-step scheduling, state merges) is negligible relative to LLM latency — the event stream shows no gaps between node end and state merge."

### Verbatim event-stream snippet (perfect for an animated terminal component on the site)

```text
[step 0] ▶ active: agent
  ├─ agent ▶ start (step 0)
  ├─ agent ✔ end   (step 0)
  ├─ state merge (step 0): channels [messages]
[step 1] ▶ active: tools
  ├─ tools ▶ start (step 1)
    [tool:get_current_time] -> 2026-08-06 06:42:52 UTC
    [tool:calculator] 0 multiply 0 = 0
    [tool:word_count] -> {"characters":43,"lines":1,"words":9}
  ├─ tools ✔ end   (step 1)
  ├─ state merge (step 1): channels [messages]
[step 2] ▶ active: agent
  ├─ agent ▶ start (step 2)
  ├─ agent ✔ end   (step 2)
  ├─ state merge (step 2): channels [messages]

--- final answer ---
The current time in UTC is August 6, 2026, 06:42:52.
128 multiplied by 46 equals 5888.
The phrase 'the quick brown fox jumps over the lazy dog' contains 9 words.
```

### Beat 4 (optional, honesty-as-a-feature) — "We found a bug live, and fixed it."

- During the demo, `calculator` received broken arguments (`0 multiply 0`) across both models. Root cause (confirmed in a post-fix follow-up run): tool-call numbers arrived **quoted** (`"128"`, `"46"`) and `Value::as_f64()` returned `None`, with `unwrap_or(0.0)` silently swallowing it.
- Fix (in `examples/live_agent.rs`): `coerce_f64` accepts JSON numbers **and** numeric strings, tolerates alias keys (`op`/`operation`/`operator`; `a`/`lhs`/`x`, `b`/`rhs`/`y`), and logs raw args on uncoercible payloads. Five new unit tests lock it in (`cargo test --example live_agent`).
- Post-fix run: "`calculator` received **correct operands for the first time**: `128 multiply 46 = 5888` ✅ — and this time the model's final `5888.0` traces to the real tool output."
- Caveats documented honestly: tool-choice stochasticity ("Demos and tests should not assert a specific set of tool calls from a live small model"); small models hallucinate around failed tools.

### Reproduce-it-yourself commands (for a "Try it" section)

```bash
# 1. Start Ollama (if the app daemon isn't already on :11434):
ollama serve &
SERVE_PID=$!
for i in $(seq 1 15); do curl -s -m 2 http://localhost:11434/api/tags >/dev/null && break; sleep 1; done

# 2. Model (one-time; llama3.2 recommended over qwen2.5:0.5b):
ollama pull llama3.2        # or: ollama pull qwen2.5:0.5b  (~400 MB)

# 3. Run the live demo:
cd rusty-core
export PATH="$HOME/.cargo/bin:$PATH"
RUSTY_MODEL=llama3.2:latest cargo run --example live_agent

# 4. Clean up:
kill $SERVE_PID
```

Environment knobs: `RUSTY_BASE_URL` (default `http://localhost:11434/v1`), `RUSTY_API_KEY` (any string for Ollama), `RUSTY_MODEL` (must support tool calling). CI-safety note (verbatim): "The example never panics: if no endpoint answers it prints setup instructions and exits 0, so it is CI-safe."

---

## 5. Quick Reference — Numbers, Names & Pull-Quotes

**Key numbers:** 10 minutes total · 2 nodes (`draft → approve`) · 3 server-crate names (`GraphRegistry`, `ServerConfig`, `serve`) · port 8080 (quickstart) / 8100 (studio demo) / 8000 (studio UI) · 1000-frame default SSE event-log capacity · 2 s warm LLM demo run · ~10× cold-vs-warm difference · `128 × 46 = 5888` · 9 words / 43 chars / 1 line.

**HTTP endpoint inventory (all demonstrated):** `GET /ok`, `GET /info`, `POST /threads`, `GET /threads/{id}/state`, `POST /threads/{id}/history`, `POST /threads/{id}/runs` (202 background), `POST /threads/{id}/runs/wait`, `POST /threads/{id}/runs/stream` (SSE), `GET /runs/{run_id}` (polling), `DELETE /threads/{id}/runs/{run_id}` (rollback), `POST /threads/{id}/fork`.

**Pull-quotes for the site:**
1. "Cargo.toml is the new langgraph.json."
2. "No Docker, no database, no Redis — everything runs in one process."
3. "…you could restart the server right now and lose nothing."
4. "`next: ["approve"]` tells you exactly where the run is parked."
5. "Rollback rewinds a thread; fork branches it."
6. "One HTML file, vanilla JS + CSS, no npm, no framework, no bundler — open it and point it at a running server." (Rusty Studio)
7. "The graph overhead itself … is negligible relative to LLM latency."
8. "Real LLM — YES." (demo verdict)

**Terminology glossary (keep consistent across the site):** thread, run, checkpoint, super-step, channel, reducer (`Reducer::Overwrite`), interrupt / `command.resume`, `resume_value()`, `stream_mode` (`updates` / `values` / `messages`), `multitask_strategy` (`enqueue` / `reject`), `GraphRegistry`, `JsonFileCheckpointer`, fork vs. replay vs. rollback, SSE frame id `{checkpoint_id}:{step}:{seq}`, `Last-Event-ID`.
