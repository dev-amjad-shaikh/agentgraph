//! # Rusty Core
//!
//! Rusty Core (the `rusty-agent-runtime` crate) is a LangGraph-style agentic
//! core engine in Rust. It models
//! agent workflows as **cyclic graphs over shared state**:
//!
//! - **State & channels** ([`state`]): every state key is a *channel* with a
//!   per-key [`state::Reducer`] defining merge semantics. Nodes return partial
//!   updates; the engine merges them via reducers. `LastValue`-style channels
//!   (the default) accept **at most one write per super-step**, otherwise an
//!   [`error::RustyError::InvalidUpdate`] is raised.
//! - **Nodes** ([`node`]): async functions (or [`node::Node`] trait impls)
//!   receiving a [`node::NodeContext`] (immutable state snapshot + config +
//!   interrupt/resume helpers) and returning a [`node::NodeOutput`] — partial
//!   state updates plus an optional [`node::Command`] for dynamic routing.
//! - **Graph** ([`graph`]): a thin builder ([`graph::GraphBuilder`]) with
//!   normal edges, conditional edges returning a [`graph::Route`] (including
//!   dynamic fan-out via [`graph::Send`]), and structural validation when
//!   you call [`graph::GraphBuilder::compile`].
//! - **Execution** ([`executor`]): a Pregel/BSP-inspired super-step loop —
//!   *plan → run active nodes in parallel over an immutable snapshot →
//!   barrier → merge writes via reducers → route → checkpoint* — emitting
//!   [`executor::GraphEvent`]s for streaming.
//! - **Persistence** ([`checkpoint`]): thread-scoped, versioned snapshots via
//!   the [`checkpoint::Checkpointer`] trait; includes an in-memory saver
//!   and a durable pure-`serde_json` file saver, plus a `postgres`-feature
//!   `PostgresCheckpointer` (see the `checkpoint_postgres` module).
//! - **LLM & tools** ([`llm`], [`tool`]): a [`llm::ChatModel`] abstraction
//!   with an OpenAI-compatible client, and a [`tool::ToolRegistry`] /
//!   [`tool::ToolExecutor`] for parallel tool-call dispatch — everything
//!   needed for the prebuilt ReAct pattern ([`react`]).
//! - **MCP** ([`mcp`]): call any MCP server's tools from [`tool::Tool`]
//!   impls over stdio transport; MCP tool servers register into the
//!   registry/executor exactly like native tools.
//! - **Remote nodes** ([`remote`]): a [`remote::RemoteNode`] executes node
//!   work on a remote worker over HTTP behind the same [`node::Node`] trait;
//!   HITL interrupts cross the wire.
//! - **WASM nodes** (`wasm_node`, feature `wasm`): sandboxed WebAssembly
//!   modules run as graph nodes via Wasmtime.
//!
//! ## Quick sketch
//!
//! ```no_run
//! use rusty_agent_runtime::prelude::*;
//!
//! # async fn demo() -> Result<()> {
//! let spec = StateSpec::new().channel("messages", Reducer::AddMessages);
//!
//! let mut builder = GraphBuilder::new();
//! builder.add_node("agent", |ctx: NodeContext| async move {
//!     let _state = ctx.state();
//!     Ok(NodeOutput::update("messages", serde_json::json!({"role": "assistant", "content": "hi"})))
//! });
//! builder.set_entry_point("agent");
//! let graph = builder.compile()?;
//!
//! let outcome = Executor::new()
//!     .run(&graph, &spec, State::new(), RunConfig::new("thread-1"))
//!     .await?;
//! # let _ = outcome;
//! # Ok(())
//! # }
//! ```

pub mod checkpoint;
#[cfg(feature = "postgres")]
pub mod checkpoint_postgres;
pub mod error;
pub mod executor;
pub mod graph;
pub mod llm;
pub mod mcp;
pub mod node;
pub mod react;
pub mod remote;
pub mod state;
pub mod tool;
#[cfg(feature = "wasm")]
pub mod wasm_node;

/// Convenience re-exports of the main public API.
pub mod prelude {
    pub use crate::checkpoint::{
        Checkpoint, Checkpointer, InMemoryCheckpointer, JsonFileCheckpointer,
    };
    #[cfg(feature = "postgres")]
    pub use crate::checkpoint_postgres::PostgresCheckpointer;
    pub use crate::error::{Result, RustyError};
    pub use crate::executor::{ExecutionOutcome, Executor, GraphEvent, RunConfig};
    pub use crate::graph::{ConditionalRouter, Edge, Graph, GraphBuilder, Route, Send};
    pub use crate::llm::{
        ChatMessage, ChatModel, ChatResponse, OpenAiCompatibleClient, Role, ToolCall, Usage,
    };
    pub use crate::node::{Command, Node, NodeConfig, NodeContext, NodeOutput};
    pub use crate::react::{create_react_agent, create_react_agent_streaming};
    pub use crate::state::{Reducer, State, StateSpec};
    pub use crate::tool::{Tool, ToolExecutor, ToolRegistry};
}
