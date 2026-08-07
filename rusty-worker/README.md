# Rusty Worker

Worker-side SDK for [`rusty-agent-runtime`](../rusty-core) remote node
execution: *one `Node` trait, remote impls behind the same trait*. A worker is
an HTTP service that hosts `Node` handlers by name; a graph node registered as
`RemoteNode` calls into it transparently.

## Endpoints

- `POST /execute` — accepts a JSON `NodeTask`, dispatches to the handler
  registered under `NodeTask::node`, and replies with a JSON
  `NodeTaskResponse`:
  - `Ok(output)` → `{ "output": ... }`
  - `Err(interrupt)` → `{ "interrupt": <value> }` (HITL across the wire)
  - `Err(e)` → `{ "error": "<message>" }`
- `GET /ok` — liveness + capability probe: protocol version and the
  registered handler names (sorted).

Status codes:

- `200 OK` for all handler-level outcomes (success, handler error, interrupt,
  unknown handler, handler panic) — the outcome lives in the body, so
  `RemoteNode` never mistakes a worker-side application error for a
  transport failure. Handler panics are caught and returned as an error body:
  a dropped connection would read as a transport failure client-side and be
  retried, silently replaying node logic.
- `400 Bad Request` when the protocol version is unsupported — a
  client/worker mismatch the client treats as fatal (never retried).

## Registering handlers

`WorkerRegistry::register` accepts **anything that implements `Node`** —
which, thanks to the blanket impl in the core crate, includes ordinary async
closures `Fn(NodeContext) -> impl Future<Output = Result<NodeOutput>>`,
named `Node` impls, and `Arc<dyn Node>`. The ergonomics match
`GraphBuilder::add_node` exactly; registering the same name twice replaces
the previous handler.

```rust,no_run
use rusty_agent_runtime::prelude::*;
use rusty_worker::{serve, WorkerRegistry};

# async fn demo() -> std::io::Result<()> {
let mut registry = WorkerRegistry::new();
registry.register("greeter", |ctx: NodeContext| async move {
    let name = ctx
        .state()
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("world")
        .to_string();
    Ok(NodeOutput::update("greeting", serde_json::json!(format!("hello, {name}!"))))
});

serve(registry, "127.0.0.1:8200").await
# }
```

On the graph side, point a `RemoteNode` at the same handler name:

```rust,ignore
builder.add_node("greeter", RemoteNode::new("greeter", "http://127.0.0.1:8200"));
```

## Error semantics across the wire

Handler errors are flattened to a message string in `NodeTaskResponse::error`
and arrive client-side as `RustyError::Node`, which the executor treats
as a **hard failure** — the retryable classes (`Llm`, `Tool`) do not survive
the wire. A remote node whose transient failures should be retried must rely
on transport-level retry (connection/timeout/5xx on the client) or surface
retryable outcomes through its own protocol on top of the `extra` config
channel.

## API

| Item                                        | Purpose                                             |
|---------------------------------------------|-----------------------------------------------------|
| `WorkerRegistry`                            | Named `Node` handlers; `new` / `register` / `with` / `contains` / `len` / `names` / `handler`. |
| `router(registry) -> Router`                | axum router (`POST /execute` + `GET /ok`) for embedding or tests with an ephemeral listener. |
| `serve(registry, addr) -> io::Result<()>`   | Bind and serve until the process stops.             |
| `probe_body() -> Value`                     | A valid `NodeTask` JSON body with the current `PROTOCOL_VERSION`, for manual `curl` probes. |

## Demo

```sh
cargo run --example worker_demo
```

Serves a `greeter` handler and an interrupting `approval_gate` HITL handler
on `127.0.0.1:8200`, and prints the matching `RemoteNode` wiring and `curl`
probe commands.

## Tests

```sh
cargo test
```

Unit tests cover the registry (register/replace/builder, probe shape); the
e2e suite runs real graphs mixing local and remote nodes through the actual
`Executor` — including an interrupt → resume round trip across the wire —
plus the HTTP-layer contract (protocol-version 400, unknown handler, handler
error, and handler panic all as 200 + one-payload error bodies).
