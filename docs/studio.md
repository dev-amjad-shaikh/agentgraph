# agentgraph Studio

A **zero-build, single-file debug UI** for [`agentgraph-server`](../agentgraph-server). One HTML file, vanilla
JS + CSS, no npm, no framework, no bundler — open it and point it at a running server.

```
studio/
├── index.html   ← the entire UI (open this)
└── serve.py     ← optional same-origin static host + API proxy
```

## What it does

- **Connect bar** — server base URL (default `http://127.0.0.1:8100`) + optional API key (`X-Api-Key`
  header). Connect calls `GET /info` and shows the service version, checkpointer kind, and every registered
  graph with its channel names. URL, key, and thread list persist in `localStorage`.
- **Graphs panel** — one card per registered graph, with a **New thread** button (`POST /threads`).
- **Threads panel (local-only)** — the v0.3 API has **no list-threads endpoint**, so threads you create or
  attach are remembered in your browser, keyed by server URL. **Attach by id** re-connects a thread the
  server already knows, and offers to re-create it with the same id when the in-memory thread registry has
  forgotten it (e.g. after a server restart — on-disk checkpoints then re-attach). ✕ *forget* only removes
  the entry from your local list; nothing is deleted server-side.
- **Per-thread workspace**
  - **Current state** — `GET /threads/{id}/state`, pretty-printed JSON grouped by channel, with `next`
    nodes and the current checkpoint ref (step, id, timestamp).
  - **Checkpoint history** — `POST /threads/{id}/history` rendered as a newest-first timeline (step,
    timestamp, checkpoint id, next nodes). Click a checkpoint to select it.
  - **Run (background)** — `POST /threads/{id}/runs`, then live-polls `GET /runs/{run_id}` with a pulsing
    status badge until the run reaches a terminal state.
  - **Run & wait** — `POST /threads/{id}/runs/wait`; the terminal JSON (`output` / `interrupt` / `error`)
    is rendered as a result card.
  - **Stream run** — `POST /threads/{id}/runs/stream` read via `fetch` + `ReadableStream` (EventSource
    can't POST), rendered live as a colored event feed: `metadata` (grey), `updates` (amber), `values`
    (sage), `messages` (clay), `error` (red), `end` (rust) — with each frame's `{checkpoint}:{step}:{seq}`
    id. `stream_mode` checkboxes and the `multitask_strategy` selector map straight onto the run payload.
  - **Fork at a checkpoint** — calls the real time-travel endpoint `POST /threads/{id}/fork` with
    `{new_thread_id, checkpoint_id}`: the server copies the thread's checkpoint history up to (and
    including) the selected checkpoint into a new thread (`{thread_id}-fork-{step}`) on the same graph,
    and returns `201 {thread_id, checkpoints_copied}`.
  - **Replay & run from a checkpoint** — starts a background run whose payload carries
    `"checkpoint": {"checkpoint_id": …}`; the executor replays the thread from that checkpoint (its state
    and next-node set) instead of the latest, appending fresh history on top. Prefer replaying on a fork.
  - **Older-server fallback** — if a fork call 404s with a non-JSON body (an `agentgraph-server` older
    than v0.3 has no `/fork` route), the Studio falls back to its original client-side composition
    (new thread + `POST /threads/{new}/state`) and says so in the toast.
  - **Interrupt / resume helper** — when any run ends `interrupted`, the interrupt payload is shown with a
    resume input; the value is sent back as `{"command": {"resume": <value>}}` (parsed as JSON when
    possible, otherwise sent as a plain string), via *wait* or *stream*.
- **Status badges** — `pending` / `running` / `success` / `interrupted` / `error`, mapped from the wire
  values returned by `GET /runs/{run_id}`, `runs/wait`, and SSE `end` frames.

## How to open

### Option A — `serve.py` (same-origin static host)

```bash
# terminal 1: the demo server
cargo run --example server_demo          # http://127.0.0.1:8100

# terminal 2: the studio
python3 studio/serve.py                  # http://127.0.0.1:8000/
```

Open `http://127.0.0.1:8000/` and connect with base URL **`/api`** (the proxy forwards `/api/*` to
`127.0.0.1:8100`; override with `--target` / `--port`). The proxy also flushes SSE per chunk and sets
`X-Accel-Buffering: no`, so streams render live.

### Option B — `python3 -m http.server` or any static host

```bash
cd studio && python3 -m http.server 8000     # → http://localhost:8000/index.html
```

Then connect to `http://127.0.0.1:8100`. Since `agentgraph-server` v0.3 sends permissive CORS headers
(see below), plain cross-origin calls from any static host just work.

### Option C — double-click `index.html` (file://)

Works too: the page runs from `file://` (origin `null`) and the server's permissive CORS layer answers
those cross-origin calls as well.

## CORS

`agentgraph-server` v0.3+ layers `tower_http::cors::CorsLayer::permissive()` in `router()` as the
outermost middleware: every response carries `access-control-allow-origin: *`, and OPTIONS preflights are
answered before the API-key middleware runs. Any page — `file://`, `localhost:8000`, a LAN hostname — can
call the API directly. **Production deployments should restrict this** (the permissive layer is a dev
convenience): see the CORS note in [agentgraph-server/README.md](../agentgraph-server/README.md#http-api).

If Connect still fails with a *network* error, the usual causes are: the server isn't running, the base URL
is wrong (scheme/host/port), or you're talking to a pre-v0.3 server build. `studio/serve.py` (Option A)
remains a valid workaround in all three cases — the browser only ever talks to its own origin.

## Demo flow (against `examples/server_demo`)

The demo registers two graphs on `127.0.0.1:8100`: `pipeline` (channel `log`, two nodes `first → second`,
no network) and `react_agent` (channel `messages`, scripted model + echo tool, no network).

1. Start both processes (Option A above). Open `http://127.0.0.1:8000/`, set the base URL to `/api`,
   **Connect** — the header shows the server version and both graphs.
2. **Graphs → pipeline → New thread.** The thread appears in the local list; state is empty (no
   checkpoints yet).
3. Leave the payload as `{}` and click **Stream run**. Watch the feed: `metadata` → `updates` (step 1,
   node `first`) → `values` → `updates` (step 2, node `second`) → `values` → `end: success`. The state
   viewer now shows `log: ["first", "second"]`; history has two checkpoints.
4. Click the **first** (older) checkpoint in the timeline, then **Fork here → new thread** — the server
   copies the history up to that checkpoint into a new thread `…-fork-1`, which appears in the local list,
   already selected, with state head `log: ["first"]` and `checkpoints_copied: 1`.
5. Back on the original thread, select the older checkpoint and **Replay & run from here** — a background
   run starts with `"checkpoint": {"checkpoint_id": …}`; the executor replays from that boundary and
   appends `second` again; the badge flips `running → success` via live polling.
6. Create a thread on **react_agent**. The payload textarea pre-fills with a `messages` input —
   **Run & wait** returns the terminal JSON with the scripted agent's tool-call transcript in `output`.
7. **Interrupt/resume** (needs a graph that interrupts — the demo graphs don't; see
   [`docs/server-quickstart.md`](../docs/server-quickstart.md) for a graph with `ctx.interrupt()`): when a
   run ends interrupted, the interrupt payload card appears; type `{"approved": true}`, click
   **Resume (wait)**, and the run continues from the interrupted node.

## Limitations (by design or by server version)

- **Thread list is local-only.** The v0.3 server has no `GET /threads`; the Studio's thread list lives in
  `localStorage`, keyed by server base URL, and is not shared across browsers or machines. Server restarts
  drop the in-memory thread registry — **Attach** re-creates a thread with the same id to re-attach to its
  on-disk checkpoints.
- **Replay appends history.** `Replay & run` on the original thread grows new checkpoints on top of the old
  timeline (checkpoint history is append-only). To branch the timeline instead, **Fork** first and run on
  the fork. Rollback of a finished run (`DELETE /threads/{id}/runs/{run_id}`) is not exposed in the UI.
- **Older servers** (pre-v0.3): fork falls back to a client-side composition (new thread + state write,
  noted in the toast); a replay payload's `checkpoint` field is silently ignored by old servers, so runs
  execute from the latest state — upgrade the server for real replay.
- **SSE resume (`Last-Event-ID`)** is implemented server-side but not surfaced in the UI — reload the page
  and the live feed starts fresh (state/history re-fetch on select).
- **Static verification only.** The page was syntax-checked (see below) but not exercised in a real browser
  in this workspace; visual/behavioral bugs are possible. The API shapes it targets were read from
  `agentgraph-server/src/routes.rs` + `src/runs.rs`, not guessed.
- The server is **single-process** with an in-memory run registry: background-run polling (`GET
  /runs/{run_id}`) 404s for runs created before a server restart.

## Verification performed

- `node --check` on the extracted `<script>` block — syntax OK.
- `python3 -m py_compile studio/serve.py` — syntax OK.
- All endpoint paths, payload fields, response shapes, SSE frame kinds, and status strings cross-checked
  against `agentgraph-server/src/routes.rs`, `src/runs.rs`, `src/sse.rs`, and `examples/server_demo.rs`;
  fork/replay and the CORS preflight are covered by server integration tests (`tests/time_travel.rs`,
  `tests/cors.rs`).
- No browser is available in this environment, so no live end-to-end test was run — the honest next step is
  the Option-A demo flow above.
