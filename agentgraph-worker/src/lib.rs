//! # agentgraph-worker
//!
//! The worker-side SDK for `agentgraph` remote node execution: *one `Node`
//! trait, remote impls behind the same trait*. A worker is just an HTTP
//! service that hosts [`Node`] handlers by name; a graph node registered as
//! [`agentgraph::remote::RemoteNode`] calls into it transparently.
//!
//! ## Endpoints
//!
//! - `POST /execute` — accepts a JSON [`NodeTask`], dispatches to the handler
//!   registered under [`NodeTask::node`], and replies with a JSON
//!   [`NodeTaskResponse`]:
//!   - `Ok(output)` → `{ "output": ... }`
//!   - `Err(interrupt)` → `{ "interrupt": <value> }` (HITL across the wire)
//!   - `Err(e)` → `{ "error": "<message>" }`
//! - `GET /ok` — liveness + capability probe: protocol version and the
//!   registered handler names.
//!
//! ## Registering handlers
//!
//! [`WorkerRegistry::register`] accepts **anything that implements
//! [`Node`]** — which, thanks to the blanket impl in the core crate, includes
//! ordinary async closures `Fn(NodeContext) -> impl Future<Output =
//! Result<NodeOutput>>`, named `Node` impls, and `Arc<dyn Node>`.
//!
//! ```no_run
//! use agentgraph::prelude::*;
//! use agentgraph_worker::{serve, WorkerRegistry};
//!
//! # async fn demo() -> std::io::Result<()> {
//! let mut registry = WorkerRegistry::new();
//! registry.register("greeter", |ctx: NodeContext| async move {
//!     let name = ctx
//!         .state()
//!         .get("name")
//!         .and_then(|v| v.as_str())
//!         .unwrap_or("world")
//!         .to_string();
//!     Ok(NodeOutput::update("greeting", serde_json::json!(format!("hello, {name}!"))))
//! });
//!
//! serve(registry, "127.0.0.1:8200").await
//! # }
//! ```
//!
//! On the graph side, point a `RemoteNode` at the same handler name:
//!
//! ```ignore
//! builder.add_node("greeter", RemoteNode::new("greeter", "http://127.0.0.1:8200"));
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use agentgraph::node::{Node, NodeContext};
use agentgraph::remote::{NodeTask, NodeTaskResponse, PROTOCOL_VERSION};
use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

/// The registry of named node handlers a worker serves.
///
/// Cheap to clone (handlers are `Arc`'d); build one up front, then hand it to
/// [`router`] or [`serve`].
#[derive(Clone, Default)]
pub struct WorkerRegistry {
    handlers: HashMap<String, Arc<dyn Node>>,
}

impl WorkerRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler under `name`.
    ///
    /// Accepts any [`Node`] implementation — including plain async closures
    /// via the core blanket impl, so the ergonomics match
    /// `GraphBuilder::add_node` exactly.
    ///
    /// Registering the same name twice replaces the previous handler.
    pub fn register<N>(&mut self, name: impl Into<String>, node: N) -> &mut Self
    where
        N: Node + 'static,
    {
        self.handlers.insert(name.into(), Arc::new(node));
        self
    }

    /// Builder-style variant of [`WorkerRegistry::register`].
    pub fn with<N>(mut self, name: impl Into<String>, node: N) -> Self
    where
        N: Node + 'static,
    {
        self.register(name, node);
        self
    }

    /// `true` if a handler is registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Number of registered handlers.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// `true` if no handlers are registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// All registered handler names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.handlers.keys().map(String::as_str)
    }

    /// Look up a handler by name.
    pub fn handler(&self, name: &str) -> Option<Arc<dyn Node>> {
        self.handlers.get(name).cloned()
    }
}

/// The shared state handed to axum handlers.
type SharedRegistry = Arc<WorkerRegistry>;

/// Liveness response for `GET /ok`.
#[derive(Debug, Serialize)]
struct OkResponse {
    status: &'static str,
    protocol_version: u32,
    nodes: Vec<String>,
}

/// `GET /ok`: liveness + capability probe.
async fn ok_handler(AxumState(registry): AxumState<SharedRegistry>) -> Json<OkResponse> {
    let mut nodes: Vec<String> = registry.names().map(str::to_owned).collect();
    nodes.sort();
    Json(OkResponse {
        status: "ok",
        protocol_version: PROTOCOL_VERSION,
        nodes,
    })
}

/// `POST /execute`: dispatch a [`NodeTask`] to its handler and shape the
/// outcome as a [`NodeTaskResponse`].
///
/// Status codes:
///
/// - `200 OK` for all handler-level outcomes (success, handler error,
///   interrupt, unknown handler) — outcome lives in the response body, so
///   `RemoteNode` never mistakes a worker-side application error for a
///   transport failure.
/// - `400 Bad Request` when the protocol version is unsupported (a
///   client/worker mismatch the client should not retry blindly).
async fn execute_handler(
    AxumState(registry): AxumState<SharedRegistry>,
    Json(task): Json<NodeTask>,
) -> (StatusCode, Json<NodeTaskResponse>) {
    let request_id = Uuid::new_v4();
    let span = tracing::info_span!(
        "execute",
        %request_id,
        node = %task.node,
        thread_id = %task.config.thread_id,
        step = task.config.step,
        protocol_version = task.protocol_version,
    );
    let _enter = span.enter();

    if task.protocol_version != PROTOCOL_VERSION {
        tracing::warn!("unsupported protocol version");
        return (
            StatusCode::BAD_REQUEST,
            Json(NodeTaskResponse::error(format!(
                "unsupported protocol_version {} (this worker speaks {})",
                task.protocol_version, PROTOCOL_VERSION
            ))),
        );
    }

    let Some(handler) = registry.handler(&task.node) else {
        tracing::warn!("no handler registered for node");
        return (
            StatusCode::OK,
            Json(NodeTaskResponse::error(format!(
                "no handler registered for node `{}` on this worker (registered: {:?})",
                task.node,
                registry.names().collect::<Vec<_>>()
            ))),
        );
    };

    let resuming = task.config.resume.is_some();
    let ctx = NodeContext::new(task.state, task.config);
    match handler.run(ctx).await {
        Ok(output) => {
            tracing::info!(resuming, "node executed");
            (StatusCode::OK, Json(NodeTaskResponse::ok(output)))
        }
        Err(e) if e.is_interrupt() => {
            let value = e.interrupt_value().cloned().unwrap_or(Value::Null);
            tracing::info!(payload = %value, "node interrupted");
            (StatusCode::OK, Json(NodeTaskResponse::interrupt(value)))
        }
        Err(e) => {
            tracing::warn!(error = %e, "node failed");
            (StatusCode::OK, Json(NodeTaskResponse::error(e.to_string())))
        }
    }
}

/// Build the axum [`Router`] for a registry (`POST /execute` + `GET /ok`).
///
/// Exposed separately from [`serve`] so tests and embedders can bind their
/// own listener (e.g. an ephemeral port) and drive the app with
/// `axum::serve`.
pub fn router(registry: WorkerRegistry) -> Router {
    Router::new()
        .route("/execute", post(execute_handler))
        .route("/ok", get(ok_handler))
        .with_state(Arc::new(registry))
}

/// Serve a registry on `addr` until the process is stopped.
///
/// ```no_run
/// # use agentgraph_worker::{serve, WorkerRegistry};
/// # async fn demo() -> std::io::Result<()> {
/// serve(WorkerRegistry::new(), "127.0.0.1:8200").await
/// # }
/// ```
pub async fn serve(registry: WorkerRegistry, addr: impl AsRef<str>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr.as_ref()).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(
        addr = %local_addr,
        nodes = ?registry.names().collect::<Vec<_>>(),
        "agentgraph worker listening"
    );
    axum::serve(listener, router(registry)).await
}

/// Convenience JSON body for quick manual probes (`curl` examples in docs).
pub fn probe_body() -> Value {
    json!({
        "protocol_version": PROTOCOL_VERSION,
        "node": "<handler-name>",
        "state": {},
        "config": { "thread_id": "t-1", "step": 0, "resume": null, "extra": {} }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentgraph::node::NodeOutput;
    use agentgraph::state::State;
    use serde_json::json;

    #[test]
    fn registry_register_and_lookup() {
        let mut registry = WorkerRegistry::new();
        assert!(registry.is_empty());

        registry.register("a", |_ctx: NodeContext| async { Ok(NodeOutput::empty()) });
        registry.register("b", |_ctx: NodeContext| async { Ok(NodeOutput::empty()) });

        assert_eq!(registry.len(), 2);
        assert!(registry.contains("a"));
        assert!(registry.contains("b"));
        assert!(!registry.contains("c"));
        assert!(registry.handler("a").is_some());

        let mut names: Vec<&str> = registry.names().collect();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn registry_builder_style_and_replace() {
        let registry = WorkerRegistry::new()
            .with("x", |_ctx: NodeContext| async {
                Ok(NodeOutput::update("v", json!(1)))
            })
            .with("x", |_ctx: NodeContext| async {
                Ok(NodeOutput::update("v", json!(2)))
            });
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn probe_body_is_a_valid_node_task_shape() {
        let body = probe_body();
        let task: std::result::Result<NodeTask, _> = serde_json::from_value(body);
        let task = task.unwrap();
        assert_eq!(task.protocol_version, PROTOCOL_VERSION);
        assert_eq!(task.state, State::new());
    }
}
