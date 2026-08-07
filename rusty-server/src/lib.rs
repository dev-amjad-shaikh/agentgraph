//! # rusty-server
//!
//! The network face of [`rusty_agent_runtime`]: an axum-based HTTP + SSE server
//! implementing a pragmatic Agent-Protocol subset (see
//! `docs/rusty-server-design.md`). The server ships as a **library** —
//! users build their graphs, register them in a [`GraphRegistry`], and call
//! [`serve`] (or [`router`] to embed the routes in a larger application):
//!
//! ```no_run
//! use rusty_agent_runtime::prelude::*;
//! use rusty_server::{serve, GraphRegistry, ServerConfig};
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
//! ## Endpoint inventory (v0.5)
//!
//! | Endpoint | Purpose |
//! |---|---|
//! | `GET /ok` | liveness |
//! | `GET /info` | service version + registered graphs and their channels |
//! | `POST /threads` | create a thread bound to a registered graph |
//! | `POST /threads/{id}/fork` | time travel: copy the thread's checkpoint history (full or up to `checkpoint_id`) into a new thread |
//! | `GET /threads/{id}/state` | latest checkpoint as `{values, next, checkpoint}` |
//! | `POST /threads/{id}/state` | write a new checkpoint (`update_state` analog; `as_node` is accepted for LangGraph compatibility but not recorded) |
//! | `POST /threads/{id}/history` | checkpoint list, newest first, `limit`/`before` |
//! | `POST /threads/{id}/runs` | background run: `202 + run_id` |
//! | `POST /threads/{id}/runs/wait` | blocking run: terminal result as JSON |
//! | `POST /threads/{id}/runs/stream` | run with SSE streaming (`updates`/`values`/`messages`/`metadata`/`error`/`end`); a fresh run starts a new frame sequence, so `Last-Event-ID` is ignored here |
//! | `GET /runs/{id}/stream` | attach to an existing run's SSE stream: replay honoring `Last-Event-ID`, then live frames |
//! | `GET /runs/{id}/events` | Flight Recorder: the run's journaled `RunEvent`s as `{run_id, events, complete}` (snapshot flushed per checkpoint boundary and at run completion; persisted under `{store_path}/journals/` or the `server_journals` table; fetchable by run id even after the live run record is evicted or the process restarts) |
//! | `GET /runs/{id}/fixture` | Flight Recorder: download the run as a portable `ReplayFixture` bundle (journal + graph topology hash + final checkpoint) for CI replay |
//! | `POST /runs/replay` | Flight Recorder: re-drive a journaled run against its registered graph and verify the replayed evidence → `{run_id, verified, expected_events, actual_events, first_divergence}` (`422` when the graph is not registered in this process or the journal carries recorded effect calls) |
//! | `GET /runs/diff?base=&branch=` | Flight Recorder: structural diff of two runs' journals (core's `BranchDiff` shape: `first_divergent_seq`, `added`/`removed` events, per-step channel diffs, token/cost totals) |
//! | `DELETE /threads/{id}/runs/{run_id}` | rollback: delete a finished run's checkpoints (JSON-file checkpointer only; `409` on Postgres) |
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
//! `assistant_id` (resolved to its bound graph, with the assistant's
//! `config.recursion_limit` as a default), and `checkpoint.checkpoint_id`
//! (time-travel replay from that checkpoint instead of the latest).

mod assistants;
mod auth;
mod crons;
mod error;
mod journals;
mod replay;
mod routes;
mod runs;
mod server_store;
mod sse;
mod store;
mod threads;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use rusty_agent_runtime::graph::Graph;
use rusty_agent_runtime::state::StateSpec;

pub use error::ApiError;
pub use runs::RunStatus;

/// Names the JSON-file layout already owns at the store root
/// (`assistants/`, `crons/`, `journals/`, `threads/`, `store/`, plus the
/// `latest` pointer file inside each thread's checkpoint dir). Client-chosen
/// ids and tenant ids claiming one of these would write checkpoints into
/// platform directories (or platform records into checkpoint dirs), so both
/// `validate_client_id` and [`ServerConfig::with_tenant_key`] reject them.
pub(crate) const RESERVED_NAMES: &[&str] = &[
    "assistants",
    "crons",
    "journals",
    "store",
    "threads",
    "latest",
];

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

    pub(crate) fn get(&self, name: &str) -> Option<(Graph, StateSpec)> {
        self.entries
            .get(name)
            .map(|entry| (entry.graph.clone(), entry.spec.clone()))
    }
}

/// Server configuration.
///
/// Checkpointing is rooted at `store_path` via
/// [`rusty_agent_runtime::checkpoint::JsonFileCheckpointer`]. Auth maps static API
/// keys (checked against the `X-Api-Key` header) to tenants: the legacy
/// single [`ServerConfig::with_api_key`] maps its key to the `default`
/// tenant, while [`ServerConfig::with_tenant_key`] adds `(tenant, key)`
/// pairs for multi-tenant deployments. With no keys configured the server
/// runs in open (dev) mode — no header required, everything lives in the
/// `default` tenant. Every tenant-scoped resource (threads + checkpoints,
/// assistants, crons, KV namespaces) is isolated per tenant; cross-tenant
/// access answers `404`. With the `postgres` feature,
/// `ServerConfig::with_postgres` switches **both** the run checkpointer
/// (core's `PostgresCheckpointer`) and the server store (assistants /
/// crons / threads / KV) to Postgres.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to bind when using [`serve`] (default `0.0.0.0:8080`).
    pub bind_addr: SocketAddr,

    /// Root directory for checkpoint files
    /// (`{store_path}/{thread_id}/{checkpoint_id}.json`). Also roots the
    /// JSON-file assistants/crons/threads/KV persistence. Unused for
    /// checkpointing when `database_url` is set (still used as the
    /// `store_path` reported by `GET /info`).
    pub store_path: PathBuf,

    /// Postgres connection URL. When set (requires the `postgres` feature —
    /// see `ServerConfig::with_postgres`), checkpoints live in core's
    /// `rusty_checkpoints` table and the platform surface in the
    /// `server_assistants` / `server_crons` / `server_threads` /
    /// `server_kv` / `server_journals` tables, all auto-migrated on
    /// connect. Connections are established lazily on first use.
    pub database_url: Option<String>,

    /// Per-thread in-flight run cap used as the **enqueue queue depth**
    /// (default 1). There is always at most one *active* run per thread.
    pub max_concurrent_runs_per_thread: usize,

    /// Static API key required via the `X-Api-Key` header, mapped to the
    /// `default` tenant (legacy single-key mode). `None` (the default) with
    /// an empty [`ServerConfig::api_keys`] is dev mode: no authentication.
    pub api_key: Option<String>,

    /// Additional `(api_key, tenant)` pairs for multi-tenant deployments
    /// (see [`ServerConfig::with_tenant_key`]). Each key maps to exactly one
    /// tenant; every tenant's threads, assistants, crons, and KV namespaces
    /// are isolated from all others (cross-tenant access answers `404`).
    pub api_keys: Vec<(String, String)>,

    /// Per-run SSE event-log capacity (frames retained for replay, default
    /// 1000).
    pub event_log_capacity: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 8080)),
            store_path: PathBuf::from("./data/checkpoints"),
            database_url: None,
            max_concurrent_runs_per_thread: 1,
            api_key: None,
            api_keys: Vec::new(),
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

    /// Builder-style: require an API key on every request. The key maps to
    /// the `default` tenant (legacy single-tenant mode).
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Builder-style: map an API key to a tenant (multi-tenant mode). Every
    /// request presenting `key` via `X-Api-Key` runs as `tenant`, fully
    /// isolated from all other tenants. Tenant ids must match
    /// `[A-Za-z0-9._-]` (1–64 chars) and must not be a reserved layout name
    /// (`assistants`, `crons`, `store`, `threads`, `latest`) — they become a
    /// path segment in the JSON-file layout and a `{tenant}/` id prefix
    /// everywhere else.
    ///
    /// # Panics
    ///
    /// Panics on an empty key or an invalid tenant id (configuration is a
    /// programmer error, caught at startup).
    pub fn with_tenant_key(mut self, tenant: impl Into<String>, key: impl Into<String>) -> Self {
        let tenant = tenant.into();
        let key = key.into();
        let valid = !tenant.is_empty()
            && tenant.len() <= 64
            && tenant
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            && !RESERVED_NAMES.contains(&tenant.as_str());
        assert!(
            valid,
            "invalid tenant id `{tenant}` (allowed: [A-Za-z0-9._-], 1..=64 chars, not a reserved name)"
        );
        assert!(
            !key.is_empty(),
            "API key for tenant `{tenant}` must not be empty"
        );
        self.api_keys.push((key, tenant));
        self
    }

    /// `true` when at least one API key is configured (legacy `api_key` or
    /// any tenant key), i.e. requests must authenticate.
    pub fn auth_enabled(&self) -> bool {
        self.api_key.is_some() || !self.api_keys.is_empty()
    }

    /// The tenant a presented API key maps to, or `None` for unknown keys.
    /// Tenant keys are checked first (last registration wins on duplicate
    /// keys); the legacy `api_key` maps to the `default` tenant.
    pub fn tenant_for_key(&self, key: &str) -> Option<&str> {
        if let Some((_, tenant)) = self.api_keys.iter().rev().find(|(k, _)| k == key) {
            return Some(tenant.as_str());
        }
        if self.api_key.as_deref() == Some(key) {
            return Some("default");
        }
        None
    }

    /// Builder-style: persist everything in Postgres at `url` (e.g.
    /// `postgres://user:pass@localhost/rusty`). Switches the run
    /// checkpointer to [`rusty_agent_runtime::checkpoint_postgres::PostgresCheckpointer`]
    /// **and** the assistants/crons/threads/KV/journals server store to the
    /// `server_*` tables. Schemas auto-migrate on (lazy) connect.
    #[cfg(feature = "postgres")]
    pub fn with_postgres(mut self, url: impl Into<String>) -> Self {
        self.database_url = Some(url.into());
        self
    }

    /// Builder-style: set the per-thread enqueue queue depth cap. Values
    /// below 1 are clamped to 1 (a zero-deep queue would reject every
    /// `enqueue` run).
    pub fn with_max_concurrent_runs_per_thread(mut self, cap: usize) -> Self {
        self.max_concurrent_runs_per_thread = cap;
        self
    }

    /// Builder-style: set the per-run SSE event-log capacity. Values below
    /// 16 are clamped to 16 (replay needs room for at least the
    /// metadata/updates/end frames of a minimal run).
    pub fn with_event_log_capacity(mut self, capacity: usize) -> Self {
        self.event_log_capacity = capacity;
        self
    }
}

/// Build the axum [`Router`] for a registry and config. Use this to embed the
/// rusty-server routes into a larger application, or to drive the API in tests
/// via `tower::ServiceExt::oneshot`.
pub fn router(registry: GraphRegistry, config: ServerConfig) -> Router {
    routes::router(registry, config)
}

/// Build the router and bind it to `config.bind_addr`. Blocks until the
/// server shuts down.
pub async fn serve(registry: GraphRegistry, config: ServerConfig) -> std::io::Result<()> {
    let addr = config.bind_addr;
    // Open (dev) mode on a non-loopback address exposes the full API — run
    // creation, KV writes, checkpoint deletion — to the network. That's a
    // legitimate dev choice, but it must never be a quiet one.
    if !config.auth_enabled() && !addr.ip().is_loopback() {
        tracing::warn!(
            %addr,
            "serving WITHOUT authentication on a non-loopback address; \
             configure `with_api_key`/`with_tenant_key` or bind 127.0.0.1"
        );
    }
    let app = router(registry, config);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "rusty-server listening");
    axum::serve(listener, app).await
}
