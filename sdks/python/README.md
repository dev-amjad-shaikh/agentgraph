# Rusty SDK — Python client

**Zero-dependency, stdlib-only Python SDK for [`rusty-server`](../../rusty-server).** Threads, runs (background / blocking / SSE-streaming), checkpoint history, time travel (fork + replay), assistants, crons, and the cross-thread KV store — over plain HTTP + SSE with nothing but `urllib.request` and `json`. Python 3.8+, no `pip install` of anything else, ever.

## Philosophy

This SDK is the **"interop over HTTP"** story: the Rust server owns orchestration, checkpoints, and streaming; any language that can speak HTTP and parse SSE can drive it. Python is the language most likely to already be on the machine, so this client deliberately uses **only the standard library** — no `requests`, no `httpx`, no `sseclient`. Drop the `rusty_client/` package into any project (or any `python3 -c` one-liner) and it works. The trade-off is explicit: you get a hand-rolled SSE parser and blocking I/O instead of a fancy async stack — which is exactly what you want for scripts, CI, notebooks, and LangChain-adjacent glue code.

## Install

```bash
pip install rusty-agent-runtime
```

From a local checkout (editable path install):

```bash
pip install /path/to/repo/sdks/python
```

Or just copy the package — it has no build step and no dependencies:

```bash
cp -r sdks/python/rusty_client /your/project/
```

## Quickstart

Start the demo server (scripted model — no network, no API keys):

```bash
cargo run -p rusty-server --example server_demo
# rusty-server demo on http://127.0.0.1:8100  (graphs: pipeline, react_agent)
```

Then, mirroring the curl quickstart from the server README:

```python
from rusty_client import RustyClient

client = RustyClient("http://127.0.0.1:8100")   # api_key="..." when auth is on

# Liveness + what's registered
client.ok()      # True
client.info()    # {"service": "rusty-server", "graphs": [...], ...}

# Create a thread bound to a registered graph
thread = client.create_thread("pipeline")
tid = thread["thread_id"]

# Blocking run
result = client.run_wait(tid)
# {"status": "success", "output": {"log": ["first", "second"]}, ...}

# Streaming run (SSE) — frames arrive as the graph executes
for frame in client.run_stream(tid, stream_mode=["updates", "values"]):
    print(frame.event, frame.id, frame.data)
# metadata -:0:1 {"run_id": ..., "graph": "pipeline", ...}
# updates  -:0:2 {"step": 0, "updates": {"log": ["first"]}}
# values   <cp>:0:3 {"log": ["first"]}
# ...
# end      <cp>:1:6 {"status": "success"}

# Background run + polling
run = client.run(tid)
status = client.run_status(run["run_id"])   # terminal runs carry output/error

# Thread state + checkpoint history
client.get_state(tid)                       # {"values", "next", "checkpoint"}
client.history(tid, limit=10)               # newest first

# Time travel: fork at an earlier checkpoint, replay on the fork
mid = next(h for h in client.history(tid) if h["next"] == ["second"])
cp_id = mid["checkpoint"]["checkpoint_id"]
fork = client.fork(tid, checkpoint_id=cp_id)
client.run_wait(fork["thread_id"], checkpoint_id=cp_id)

# Human-in-the-loop: resume an interrupted run
client.run_wait(tid, command={"resume": {"approved": True}})

# Assistants, crons, KV store
assistant = client.create_assistant("support-bot", graph="react_agent",
                                    config={"recursion_limit": 25})
client.run_wait(tid, assistant_id=assistant["assistant_id"])

cron = client.create_cron(graph="react_agent", interval_secs=60,
                          input={"messages": [{"role": "user", "content": "hourly summary"}]})
client.list_crons()
client.delete_cron(cron["cron_id"])

client.kv_put("memories", "user-1", {"preference": "dark-mode"})
client.kv_list("memories")
client.kv_delete("memories", "user-1")
```

With auth configured on the server (`ServerConfig::with_api_key`), pass `RustyClient(url, api_key="...")` — it is sent as the `X-Api-Key` header on every request.

## API reference

| Method | HTTP | Returns |
|---|---|---|
| `ok()` | `GET /ok` | `bool` |
| `info()` | `GET /info` | service metadata + registered graphs |
| `create_thread(graph, thread_id=None, metadata=None)` | `POST /threads` | thread record |
| `get_state(thread_id)` | `GET /threads/{id}/state` | `{values, next, checkpoint}` |
| `update_state(thread_id, values, as_node=None, next_nodes=None)` | `POST /threads/{id}/state` | new checkpoint |
| `history(thread_id, limit=None, before=None)` | `POST /threads/{id}/history` | checkpoints, newest first |
| `fork(thread_id, checkpoint_id=None, new_thread_id=None)` | `POST /threads/{id}/fork` | `{thread_id, checkpoints_copied}` |
| `run(thread_id, input=None, command=None, checkpoint_id=None, multitask_strategy=None, config=None, metadata=None, assistant_id=None)` | `POST /threads/{id}/runs` | `202` `{run_id, …}` (background) |
| `run_wait(thread_id, …same opts…, timeout=None)` | `POST /threads/{id}/runs/wait` | terminal dict `{status, output‖interrupt, …}` |
| `run_stream(thread_id, …same opts…, stream_mode=None, last_event_id=None, timeout=None)` | `POST /threads/{id}/runs/stream` | **generator** of `SSEEvent(event, data, id)` |
| `run_status(run_id)` | `GET /runs/{id}` | `{run_id, status, …}` (+ `output`/`error` when terminal) |
| `delete_run(thread_id, run_id)` | `DELETE /threads/{id}/runs/{run_id}` | rollback a finished run |
| `create_assistant(name, graph, config=None, metadata=None, assistant_id=None)` | `POST /assistants` | assistant record |
| `list_assistants()` / `get_assistant(assistant_id)` | `GET /assistants[/{id}]` | assistant(s) |
| `create_cron(graph, interval_secs=None, cron_expr=None, input=None, metadata=None, on_run_completed=None)` | `POST /crons` | cron record (exactly one schedule kind) |
| `list_crons()` / `delete_cron(cron_id)` | `GET`/`DELETE /crons[/{id}]` | cron(s) |
| `kv_put(ns, key, value)` / `kv_get(ns, key)` / `kv_delete(ns, key)` / `kv_list(ns)` | `PUT`/`GET`/`DELETE /store/{ns}[/{key}]` | KV item(s) |

### Streaming details

- `run_stream` returns a generator of `SSEEvent` dataclasses: `event` (e.g. `metadata`, `updates`, `values`, `messages`, `error`, `end`), `data` (JSON-decoded when possible), and `id` (`{checkpoint_id}:{step}:{seq}`).
- `stream_mode` filters frame families (`"updates"`, `"values"`, `"messages"`); `metadata`/`error`/`end` are always emitted.
- Pass `last_event_id=frame.id` to resume a dropped connection — the server replays only frames after that id (sent as the `Last-Event-ID` header).

### Errors

Every non-2xx response raises `RustyError` with `.status` (HTTP code, `None` for transport failures) and `.body` (raw response text):

```python
from rusty_client import RustyError

try:
    client.create_thread("no_such_graph")
except RustyError as exc:
    print(exc.status, exc.body)   # 404 / 400, server's error JSON
```

## Tests

The suite is a true end-to-end test: it builds (if needed) and launches the real `server_demo` binary as a subprocess, waits for `/ok`, exercises every endpoint family against it, and kills the process afterwards.

```bash
python -m pytest sdks/python/tests -q
```

Note: `server_demo` registers no interrupting graph, so the interrupt/resume round trip is a documented skip in the suite; the client's resume path is `run_wait(tid, command={"resume": value})`.

## License

Dual-licensed under MIT OR Apache-2.0, same as the rest of the repo.
