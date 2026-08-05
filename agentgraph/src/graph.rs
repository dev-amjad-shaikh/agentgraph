//! Graph definition and compile-time validation.
//!
//! The public graph API is deliberately a thin builder ([`GraphBuilder`]);
//! `compile()` freezes the graph into an immutable, cheaply-clonable
//! [`Graph`] with `Arc`-wrapped internals and validates structure **before
//! any node (or LLM call) ever runs**:
//!
//! - an entry point must be set and must reference a known node;
//! - every edge endpoint must reference a known node;
//! - at least one node must be registered.
//!
//! Edge types:
//!
//! - **Normal edges** ([`GraphBuilder::add_edge`]): `from → to`. Multiple
//!   outgoing normal edges from one node activate **all** destinations in
//!   parallel in the next super-step.
//! - **Conditional edges** ([`GraphBuilder::add_conditional_edges`]): an
//!   async routing function reads the post-barrier state and returns a
//!   [`Route`] — one node, dynamic fan-out via [`Send`], or the end of the
//!   run.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AgentGraphError, Result};
use crate::node::Node;
use crate::state::State;

/// Sentinel node name for the terminal route. Not a real node; returned by
/// routers as [`Route::End`] and accepted by the executor.
pub const END: &str = "__end__";

/// The routing decision of a conditional edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Activate exactly one node next.
    Node(String),
    /// Dynamic fan-out (LangGraph `Send` API): activate one node invocation
    /// per item, each with its own scoped input state. The canonical
    /// map-reduce pattern: items are generated at runtime, each mapped
    /// through a node, results fan back in through multi-write reducers.
    Send(Vec<Send>),
    /// Terminate the run.
    End,
}

/// A single dynamic fan-out instruction: run `node` once with `state` as
/// its scoped input state.
///
/// Semantics for the executor: the scoped state is merged into the shared
/// state before the node runs (so the node sees its item), and the node's
/// updates merge back through the normal channel reducers — fan-in requires
/// multi-write reducers on the destination channels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Send {
    /// Target node name (must resolve to a known node at execution time).
    pub node: String,
    /// Scoped input state for this invocation (partial update applied to the
    /// shared state snapshot before the node runs).
    pub state: Value,
}

impl Send {
    /// Convenience constructor.
    pub fn new(node: impl Into<String>, state: Value) -> Self {
        Self {
            node: node.into(),
            state,
        }
    }
}

/// Async routing function for a conditional edge: reads the post-barrier
/// state and decides where the run goes next.
///
/// Produced by [`GraphBuilder::add_conditional_edges`] from any
/// `Fn(State) -> impl Future<Output = Result<Route>>`.
// NOTE: `Send` below refers to std's marker trait; the local `Send` struct
// shadows it in this module, hence the fully-qualified path.
pub type ConditionalRouter = Arc<
    dyn Fn(State) -> Pin<Box<dyn Future<Output = Result<Route>> + std::marker::Send>>
        + std::marker::Send
        + Sync,
>;

/// One edge in the graph.
#[derive(Clone)]
pub enum Edge {
    /// Static edge: after `from` completes, activate `to` in the next
    /// super-step. Multiple `Direct` edges from the same node activate all
    /// targets in parallel.
    Direct { from: String, to: String },
    /// Dynamic edge: after `from` completes, evaluate `router` against the
    /// post-barrier state to decide the next activation.
    Conditional {
        from: String,
        router: ConditionalRouter,
    },
}

impl Edge {
    /// The source node of this edge.
    pub fn from(&self) -> &str {
        match self {
            Edge::Direct { from, .. } | Edge::Conditional { from, .. } => from,
        }
    }
}

impl fmt::Debug for Edge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Edge::Direct { from, to } => write!(f, "Edge::Direct({from} -> {to})"),
            Edge::Conditional { from, .. } => write!(f, "Edge::Conditional({from} -> ?)"),
        }
    }
}

/// Internal, immutable graph topology. Wrapped in `Arc` by [`Graph`].
struct GraphInner {
    nodes: HashMap<String, Arc<dyn Node>>,
    edges: Vec<Edge>,
    entry_point: String,
}

/// A compiled, immutable, thread-safe graph. Cheap to clone (Arc internals).
///
/// Obtain via [`GraphBuilder::compile`]. The executor drives a `Graph`;
/// graphs are frozen so topology can never drift mid-run.
pub struct Graph {
    inner: Arc<GraphInner>,
}

impl Clone for Graph {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl fmt::Debug for Graph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Graph")
            .field("nodes", &self.inner.nodes.keys().collect::<Vec<_>>())
            .field("edges", &self.inner.edges)
            .field("entry_point", &self.inner.entry_point)
            .finish()
    }
}

impl Graph {
    /// The entry-point node name.
    pub fn entry_point(&self) -> &str {
        &self.inner.entry_point
    }

    /// Look up a node by name.
    pub fn node(&self, name: &str) -> Option<Arc<dyn Node>> {
        self.inner.nodes.get(name).cloned()
    }

    /// `true` if a node with this name is registered.
    pub fn has_node(&self, name: &str) -> bool {
        self.inner.nodes.contains_key(name)
    }

    /// All registered node names.
    pub fn node_names(&self) -> impl Iterator<Item = &str> {
        self.inner.nodes.keys().map(String::as_str)
    }

    /// All edges originating at `from` (static and conditional).
    pub fn outgoing_edges(&self, from: &str) -> Vec<&Edge> {
        self.inner
            .edges
            .iter()
            .filter(|e| e.from() == from)
            .collect()
    }

    /// All edges in the graph.
    pub fn edges(&self) -> &[Edge] {
        &self.inner.edges
    }

    /// Number of registered nodes.
    pub fn node_count(&self) -> usize {
        self.inner.nodes.len()
    }
}

/// Builder for a [`Graph`]. Register nodes and edges, set the entry point,
/// then [`GraphBuilder::compile`] to validate and freeze.
#[derive(Default)]
pub struct GraphBuilder {
    nodes: HashMap<String, Arc<dyn Node>>,
    edges: Vec<Edge>,
    entry_point: Option<String>,
}

impl GraphBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a node under `name`.
    ///
    /// Accepts any [`Node`] impl — including async closures
    /// (`Fn(NodeContext) -> impl Future<Output = Result<NodeOutput>>`) via
    /// the blanket impl. Re-registering the same name replaces the node.
    pub fn add_node<N>(&mut self, name: impl Into<String>, node: N) -> &mut Self
    where
        N: Node + 'static,
    {
        self.nodes.insert(name.into(), Arc::new(node));
        self
    }

    /// Add a static edge `from → to`. Multiple outgoing static edges from
    /// the same node activate all destinations in parallel in the next
    /// super-step.
    ///
    /// **Do not** mix static edges and dynamic routing (`Command::goto` /
    /// conditional edges) from the same node — both paths will execute.
    pub fn add_edge(&mut self, from: impl Into<String>, to: impl Into<String>) -> &mut Self {
        self.edges.push(Edge::Direct {
            from: from.into(),
            to: to.into(),
        });
        self
    }

    /// Add a conditional edge: after `from` completes, run `router` against
    /// the post-barrier state and follow the returned [`Route`].
    ///
    /// ```ignore
    /// builder.add_conditional_edges("agent", |state| async move {
    ///     let needs_tools = state.get("messages")
    ///         .and_then(|m| m.as_array())
    ///         .and_then(|a| a.last())
    ///         .map(|m| m.get("tool_calls").is_some())
    ///         .unwrap_or(false);
    ///     Ok(if needs_tools { Route::Node("tools".into()) } else { Route::End })
    /// });
    /// ```
    pub fn add_conditional_edges<F, Fut>(&mut self, from: impl Into<String>, router: F) -> &mut Self
    where
        F: Fn(State) -> Fut + std::marker::Send + Sync + 'static,
        Fut: Future<Output = Result<Route>> + std::marker::Send + 'static,
    {
        let router: ConditionalRouter = Arc::new(move |state| Box::pin(router(state)));
        self.edges.push(Edge::Conditional {
            from: from.into(),
            router,
        });
        self
    }

    /// Set the entry-point node (the LangGraph `START` edge).
    pub fn set_entry_point(&mut self, name: impl Into<String>) -> &mut Self {
        self.entry_point = Some(name.into());
        self
    }

    /// Validate and freeze the graph.
    ///
    /// Validation (all failures are [`AgentGraphError::Graph`]):
    ///
    /// - at least one node is registered;
    /// - the entry point is set and references a known node;
    /// - every edge endpoint (`from`, and `to` for static edges) references
    ///   a known node.
    ///
    /// Conditional router targets and [`Send`] node names are validated at
    /// execution time (they are data-dependent by design).
    pub fn compile(self) -> Result<Graph> {
        if self.nodes.is_empty() {
            return Err(AgentGraphError::Graph(
                "cannot compile an empty graph: register at least one node".into(),
            ));
        }

        let entry_point = self.entry_point.ok_or_else(|| {
            AgentGraphError::Graph("no entry point set: call set_entry_point()".into())
        })?;
        if !self.nodes.contains_key(&entry_point) {
            return Err(AgentGraphError::Graph(format!(
                "entry point `{entry_point}` does not reference a known node"
            )));
        }

        for edge in &self.edges {
            let from = edge.from();
            if !self.nodes.contains_key(from) {
                return Err(AgentGraphError::Graph(format!(
                    "edge source `{from}` does not reference a known node"
                )));
            }
            if let Edge::Direct { to, .. } = edge {
                if !self.nodes.contains_key(to) {
                    return Err(AgentGraphError::Graph(format!(
                        "edge target `{to}` (from `{from}`) does not reference a known node"
                    )));
                }
            }
        }

        Ok(Graph {
            inner: Arc::new(GraphInner {
                nodes: self.nodes,
                edges: self.edges,
                entry_point,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodeOutput;
    use serde_json::json;

    async fn ok_node(_ctx: crate::node::NodeContext) -> Result<NodeOutput> {
        Ok(NodeOutput::empty())
    }

    #[test]
    fn compile_rejects_empty_graph() {
        let err = GraphBuilder::new().compile().unwrap_err();
        assert!(matches!(err, AgentGraphError::Graph(_)));
    }

    #[test]
    fn compile_rejects_missing_entry_point() {
        let mut b = GraphBuilder::new();
        b.add_node("a", ok_node);
        let err = b.compile().unwrap_err();
        assert!(matches!(err, AgentGraphError::Graph(_)));
    }

    #[test]
    fn compile_rejects_unknown_entry_point() {
        let mut b = GraphBuilder::new();
        b.add_node("a", ok_node);
        b.set_entry_point("nope");
        assert!(b.compile().is_err());
    }

    #[test]
    fn compile_rejects_dangling_edges() {
        let mut b = GraphBuilder::new();
        b.add_node("a", ok_node);
        b.set_entry_point("a");
        b.add_edge("a", "ghost");
        assert!(b.compile().is_err());

        let mut b = GraphBuilder::new();
        b.add_node("a", ok_node);
        b.set_entry_point("a");
        b.add_conditional_edges("ghost", |_s| async { Ok(Route::End) });
        assert!(b.compile().is_err());
    }

    #[test]
    fn compile_accepts_valid_graph() {
        let mut b = GraphBuilder::new();
        b.add_node("agent", ok_node);
        b.add_node("tools", ok_node);
        b.set_entry_point("agent");
        b.add_edge("tools", "agent");
        b.add_conditional_edges("agent", |state| async move {
            Ok(if state.contains("done") {
                Route::End
            } else {
                Route::Node("tools".into())
            })
        });
        let graph = b.compile().unwrap();
        assert_eq!(graph.entry_point(), "agent");
        assert_eq!(graph.node_count(), 2);
        assert!(graph.has_node("tools"));
        assert_eq!(graph.outgoing_edges("agent").len(), 1);
        assert_eq!(graph.outgoing_edges("tools").len(), 1);

        // Clones share the same frozen internals.
        let g2 = graph.clone();
        assert!(g2.node("agent").is_some());

        // Router is callable.
        let edges = graph.outgoing_edges("agent");
        let route = match edges[0] {
            Edge::Conditional { router, .. } => {
                let r = router(State::from_value(json!({"done": true})).unwrap());
                futures::executor::block_on(r)
            }
            _ => panic!("expected conditional edge"),
        };
        assert_eq!(route.unwrap(), Route::End);
    }
}
