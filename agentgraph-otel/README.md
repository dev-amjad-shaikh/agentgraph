# agentgraph-otel

OpenTelemetry export layer for the [`agentgraph`](../agentgraph) engine: one
call installs a global `tracing` subscriber that routes the executor's
structured spans to pretty stderr logs and — optionally — an OTLP/HTTP span
exporter feeding any OTel-compatible backend (collector, Jaeger, Tempo, ...).

## What gets traced

`agentgraph`'s executor is already instrumented; this crate only wires the
export. The span taxonomy emitted by `agentgraph` v0.3.0:

| Span / event            | Level | Fields                                   | Meaning                                            |
|-------------------------|-------|------------------------------------------|----------------------------------------------------|
| `agentgraph.run`        | INFO  | `thread_id`, `max_steps`                 | One per `Executor::run` call; root of the trace.   |
| `agentgraph.super_step` | DEBUG | `step`, `active_nodes`                   | One per Pregel/BSP super-step (plan → barrier → merge → route → checkpoint). |
| `agentgraph.node`       | INFO  | `node`, `step`                           | One per spawned node task (attached via `.instrument()`). |
| barrier-merge event     | DEBUG | channels written                         | Reducer merge at each super-step barrier.          |
| run-complete event      | INFO  | `steps`, `duration_ms`                   | Run finished.                                      |
| interrupt event         | INFO  | —                                        | Run interrupted (human-in-the-loop).               |
| error events            | WARN  | —                                        | Node/routing failures.                             |

Because spans nest (`run` → `super_step` → `node`), a single
`agentgraph.run` trace in Jaeger fans out into the full execution tree with
`thread_id`, `step`, and `node` attributes on every span.

## Setup

### Local only (no collector)

```rust
let _guard = agentgraph_otel::init_local("my-agent")?;
```

Pretty span logs go to stderr, filtered by `RUST_LOG` or the built-in
default `info,agentgraph=debug` (which surfaces the DEBUG `super_step`
spans without flooding other crates).

### With a collector (OTLP export)

```rust
let mut guard = agentgraph_otel::init(agentgraph_otel::OTelConfig {
    service_name: "my-agent".into(),
    otlp_endpoint: Some("http://localhost:4318/v1/traces".into()),
    log_filter: None, // RUST_LOG, else the default above
})?;

// ... run graphs ...

guard.shutdown(); // flush buffered spans before exit (idempotent; also on drop)
```

Notes:

- `init` may succeed **once per process** (the subscriber is global). A
  second call returns `OTelError::SubscriberAlreadyInstalled`.
- The exporter batches spans on a dedicated thread, so `init` works with or
  without a running Tokio runtime.
- Keep the `OTelGuard` alive until shutdown/drop or batched spans are lost.

### See the traces: docker compose

This crate ships a ready-made local stack (`docker-compose.yml` +
`otel-collector-config.yaml`): an OTel Collector receiving OTLP on
`4317`/`4318`, forwarding to Jaeger all-in-one:

```sh
docker compose up -d

# Run the demo against it (2-node pipeline + ReAct agent with a mock LLM):
OTEL_DEMO_ENDPOINT=http://localhost:4318/v1/traces cargo run --example otel_demo

# Open the Jaeger UI and select the `agentgraph-otel-demo` service:
open http://localhost:16686

# Tear down:
docker compose down
```

Without the endpoint variable, `cargo run --example otel_demo` runs fmt-only
so you can still see the span tree on stderr. The collector also logs span
summaries via its `debug` exporter (`docker compose logs -f otel-collector`).

## API

| Item                              | Purpose                                                        |
|-----------------------------------|----------------------------------------------------------------|
| `init(OTelConfig) -> Result<OTelGuard>` | Install the global subscriber: `Registry + EnvFilter + fmt (+ OTLP layer when `otlp_endpoint` is set)`. |
| `init_local(&str) -> Result<OTelGuard>` | Shorthand for fmt-only local logging.                      |
| `OTelConfig { service_name, otlp_endpoint, log_filter }` | Service name (OTel `service.name` resource), optional OTLP/HTTP endpoint, optional filter directive. |
| `OTelGuard::shutdown(&mut self)`  | Flush + shut down the tracer provider. Idempotent; also runs on drop. |
| `OTelError`                       | `SubscriberAlreadyInstalled` (second `init`) or `ExporterBuild` (bad endpoint/config). |
| `DEFAULT_FILTER`                  | `info,agentgraph=debug` — the fallback `EnvFilter`.           |

## Tests

```sh
cargo test
```

Covers once-per-process init (second init errors gracefully), `EnvFilter`
resolution (explicit → `RUST_LOG` → default), and idempotent guard shutdown.
No live collector is required.
