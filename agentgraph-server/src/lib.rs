//! # agentgraph-server
//!
//! The network face of [`agentgraph`]: an axum-based HTTP + SSE server
//! implementing a pragmatic Agent-Protocol subset (see
//! `docs/agentgraph-server-design.md`). The server ships as a **library** —
//! users build their graphs, register them in a [`GraphRegistry`], and call
//! [`serve`] (or [`router`] to embed the routes in a larger application):
//!
//! ```no_run
//! use agentgraph::prelude::*;
//! use agentgraph_server::{serve, GraphRegistry, ServerConfig};
//!
//! # async fn demo(graph: Graph, spec: StateSpec) -> std::io::Result<()> {
//! let mut registry = GraphRegistry::new();
//! registry.register("my_agent", graph, spec);
//!
//! let config = ServerConfig::new(
//!     "127.0.0.1:8080".parse().unwrap(),
//!     "./data/checkpoints",
//! );
//! serve(registry, config).await
//! # }
//! ```
//!
//! ## Endpoint inventory (v0.2)
//!
//! | Endpoint | Purpose |
//! |---|---|
//! | `GET /ok` | liveness |
//! | `GET /info` | service version + registered graphs and their channels |
//! | `POST /threads` | create a thread bound to a registered graph |
//! | `GET /threads/{id}/state` | latest checkpoint as `{values, next, checkpoint}` |
//! | `POST /threads/{id}/state` | write a new checkpoint (`update_state` analog) |
//! | `POST /threads/{id}/history` | checkpoint list, newest first, `limit`/`before` |
//! | `POST /threads/{id}/runs` | background run: `202 + run_id` |
//! | `POST /threads/{id}/runs/wait` | blocking run: terminal result as JSON |
//! | `POST /threads/{id}/runs/stream` | run with SSE streaming (`updates`/`values`/`messages`/`metadata`/`error`/`end`) |
//! | `DELETE /threads/{id}/runs/{run_id}` | rollback: delete a finished run's checkpoints |
//! | `GET /runs/{run_id}` | run status polling (plus `output`/`error`/`interrupt` once terminal) |
//! | `POST /assistants` | create a named graph alias with config metadata |
//! | `GET /assistants` / `GET /assistants/{id}` | list / fetch assistants |
//! | `POST /crons` | schedule recurring runs (interval secs or 5-field cron expr) |
//! | `GET /crons` / `DELETE /crons/{id}` | list / delete crons |
//! | `PUT /store/{ns}/{key}` | upsert a JSON value in a namespace (`201` create, `200` replace) |
//! | `GET /store/{ns}/{key}` / `DELETE /store/{ns}/{key}` | fetch / delete one item |
//! | `GET /store/{ns}` | list a namespace's items |
//!
//! Runs support `command.resume` (HITL), `config.recursion_limit`, the
//! `reject` / `enqueue` multitask strategies (one active run per thread),
//! and `assistant_id` (resolved to its bound graph, with the assistant's
//! `config.recursion_limit` as a default).

mod assistants;
mod auth;
mod crons;
mod error;
mod routes;
mod runs;
mod sse;
mod store;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use agentgraph::graph::Graph;
use agentgraph::state::StateSpec;
use axum::Router;

pub use error::ApiError;
pub use runs::{RunManager, RunStatus};

/// One registered graph: the compiled topology plus the state schema the
/// executor needs to drive it.
#[derive(Debug, Clone)]
struct GraphEntry {
    graph: Graph,
    spec: StateSpec,
}

/// The set of graphs this server hosts — the Rust analog of the `graphs` map
/// in LangGraph's `langgraph.json`. Registration is compile-checked in user
/// code; a `GraphRegistry` is heterogeneous (each entry carries its own
/// [`StateSpec`]), which is safe because `State` is a JSON map at runtime.
#[derive(Debug, Default, Clone)]
pub struct GraphRegistry {
    entries: HashMap<String, GraphEntry>,
}

impl GraphRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a compiled graph under `name`, together with the state spec
    /// the executor should merge its node updates through. Re-registering a
    /// name replaces the previous entry.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        graph: Graph,
        spec: StateSpec,
    ) -> &mut Self {
        self.entries.insert(name.into(), GraphEntry { graph, spec });
        self
    }

    /// `true` if a graph is registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// All registered graph names, sorted for stable output.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    /// The declared channel names of a registered graph's spec, sorted.
    pub fn channel_names(&self, name: &str) -> Vec<String> {
        let mut channels: Vec<String> = self
            .entries
            .get(name)
            .map(|entry| entry.spec.channel_names().map(str::to_owned).collect())
            .unwrap_or_default();
        channels.sort_unstable();
        channels
    }

    /// A cheap clone of the `(Graph, StateSpec)` pair for `name`.
    pub(crate) fn get(&self, name: &str) -> Option<(Graph, StateSpec)> {
        self.entries
            .get(name)
            .map(|entry| (entry.graph.clone(), entry.spec.clone()))
    }
}

/// Server configuration.
///
/// Checkpointing is rooted at `store_path` via
/// [`agentgraph::checkpoint::JsonFileCheckpointer`]; auth is a single static
/// API key checked against the `X-Api-Key` header when set.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to bind when using [`serve`] (default `0.0.0.0:8080`).
    pub bind_addr: SocketAddr,

    /// Root directory for checkpoint files
    /// (`{store_path}/{thread_id}/{checkpoint_id}.json`).
    pub store_path: PathBuf,

    /// Per-thread in-flight run cap used as the **enqueue queue depth**
    /// (default 1). There is always at most one *active* run per thread.
    pub max_concurrent_runs_per_thread: usize,

    /// Static API key required via the `X-Api-Key` header. `None` (the
    /// default) is dev mode: no authentication.
    pub api_key: Option<String>,

    /// Per-run SSE event-log capacity (frames retained for replay, default
    /// 1000).
    pub event_log_capacity: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 8080)),
            store_path: PathBuf::from("./data/checkpoints"),
            max_concurrent_runs_per_thread: 1,
            api_key: None,
            event_log_capacity: 1000,
        }
    }
}

impl ServerConfig {
    /// A config with the given bind address and checkpoint store root;
    /// everything else at its default.
    pub fn new(bind_addr: SocketAddr, store_path: impl Into<PathBuf>) -> Self {
        Self {
            bind_addr,
            store_path: store_path.into(),
            ..Self::default()
        }
    }

    /// Builder-style: require an API key on every request.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Builder-style: set the per-thread enqueue queue depth cap.
    pub fn with_max_concurrent_runs_per_thread(mut self, cap: usize) -> Self {
        self.max_concurrent_runs_per_thread = cap;
        self
    }

    /// Builder-style: set the per-run SSE event-log capacity.
    pub fn with_event_log_capacity(mut self, capacity: usize) -> Self {
        self.event_log_capacity = capacity;
        self
    }
}

/// Build the axum [`Router`] for a registry and config. Use this to embed the
/// agentgraph routes into a larger application, or to drive the API in tests
/// via `tower::ServiceExt::oneshot`.
pub fn router(registry: GraphRegistry, config: ServerConfig) -> Router {
    routes::router(registry, config)
}

/// Build the router and bind it to `config.bind_addr`. Blocks until the
/// server shuts down.
pub async fn serve(registry: GraphRegistry, config: ServerConfig) -> std::io::Result<()> {
    let addr = config.bind_addr;
    let app = router(registry, config);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "agentgraph-server listening");
    axum::serve(listener, app).await
}
