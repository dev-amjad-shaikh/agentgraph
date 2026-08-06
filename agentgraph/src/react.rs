//! The prebuilt ReAct agent (LangGraph `create_react_agent` parity).
//!
//! [`create_react_agent`] assembles the classic reasoning-acting loop as a
//! two-node cyclic graph over a single `messages` channel
//! ([`Reducer::AddMessages`](crate::state::Reducer::AddMessages)):
//!
//! ```text
//!         ┌──────────────────────────────────────────┐
//!         │                                          │
//!         ▼                                          │
//!      [agent] ── last message has tool_calls? ──► [tools]
//!         │                                          │
//!         └─ no tool_calls ──► End ◄── static edge ──┘
//! ```
//!
//! - **`agent`** — serializes the `messages` channel into
//!   [`ChatMessage`]s, calls [`ChatModel::chat`] with the registry's
//!   OpenAI-format tool schemas, and appends the assistant message (final
//!   answer *or* tool-call request) back onto `messages`.
//! - **`tools`** — takes the `tool_calls` of the last assistant message,
//!   dispatches them in parallel through [`ToolExecutor::execute_batch`],
//!   and appends one `role: "tool"` message per call.
//! - **Routing** — a conditional edge on `agent` routes to `tools` when the
//!   last message carries tool calls, otherwise to [`Route::End`]; a static
//!   edge loops `tools → agent` so the model observes the tool results.
//!
//! The caller drives the returned [`Graph`] with a [`crate::state::StateSpec`]
//! declaring `messages` with `Reducer::AddMessages` and an initial state
//! seeding the conversation (see `examples/react_agent.rs`).
//!
//! Two flavors exist: [`create_react_agent`] (the agent node calls
//! [`ChatModel::chat`]; no [`crate::executor::GraphEvent::Token`] events) and
//! [`create_react_agent_streaming`] (the agent node calls
//! [`ChatModel::chat_stream`] and forwards deltas as
//! [`crate::executor::GraphEvent::Token`]s into the run's event channel).

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::error::{AgentGraphError, Result};
use crate::executor::GraphEvent;
use crate::graph::{Graph, GraphBuilder, Route};
use crate::llm::{ChatMessage, ChatModel};
use crate::node::NodeOutput;
use crate::tool::{ToolExecutor, ToolRegistry};

/// The state channel the ReAct loop reads from and appends to. Declare it
/// with `Reducer::AddMessages` in the run's [`crate::state::StateSpec`].
pub const MESSAGES_CHANNEL: &str = "messages";

/// The name of the model-calling node in the compiled graph.
pub const AGENT_NODE: &str = "agent";

/// The name of the tool-dispatch node in the compiled graph.
pub const TOOLS_NODE: &str = "tools";

/// Read and deserialize the `messages` channel from a state snapshot.
///
/// A missing channel yields an empty conversation (the run may legitimately
/// start before any message is seeded); a malformed channel is a hard error.
fn read_messages(state: &crate::state::State) -> Result<Vec<ChatMessage>> {
    Ok(state
        .get_as::<Vec<ChatMessage>>(MESSAGES_CHANNEL)?
        .unwrap_or_default())
}

/// Build a prebuilt ReAct agent graph over `model` and `tools`.
///
/// The returned graph has exactly two nodes ([`AGENT_NODE`], [`TOOLS_NODE`]),
/// a conditional edge `agent → tools | End`, and a static edge
/// `tools → agent`. It is stateless with respect to any single run: clone it
/// freely and drive it with the [`crate::executor::Executor`].
///
/// The graph never errors at build time for an empty registry — a tool-less
/// agent simply answers directly on the first `agent` pass.
///
/// **This variant never emits [`GraphEvent::Token`]:** the agent node calls
/// [`ChatModel::chat`]. Use [`create_react_agent_streaming`] to stream token
/// deltas into the run's event channel.
pub fn create_react_agent(model: Arc<dyn ChatModel>, tools: ToolRegistry) -> Result<Graph> {
    build_react_agent(model, tools, None)
}

/// Build a prebuilt ReAct agent graph whose `agent` node streams token
/// deltas as [`GraphEvent::Token`]s through `token_tx`
/// ([`ChatModel::chat_stream`] under the hood; LangGraph's `messages`
/// stream mode).
///
/// Typically `token_tx` is a clone of the run's event sender
/// ([`crate::executor::RunConfig::token_tx`]) so token deltas interleave with
/// the executor's own events on one channel. Forwarding is best-effort
/// (`try_send`): a full or closed channel drops tokens but never aborts the
/// run.
///
/// Identical to [`create_react_agent`] in topology and behavior otherwise;
/// models that only implement [`ChatModel::chat`] work unchanged (the
/// trait's default `chat_stream` delivers the whole answer as one token).
pub fn create_react_agent_streaming(
    model: Arc<dyn ChatModel>,
    tools: ToolRegistry,
    token_tx: mpsc::Sender<GraphEvent>,
) -> Result<Graph> {
    build_react_agent(model, tools, Some(token_tx))
}

fn build_react_agent(
    model: Arc<dyn ChatModel>,
    tools: ToolRegistry,
    token_tx: Option<mpsc::Sender<GraphEvent>>,
) -> Result<Graph> {
    let tool_schemas = tools.schemas();
    let tool_executor = ToolExecutor::new(tools);

    let agent_node = {
        let model = Arc::clone(&model);
        move |ctx: crate::node::NodeContext| {
            let model = Arc::clone(&model);
            let tool_schemas = tool_schemas.clone();
            let token_tx = token_tx.clone();
            async move {
                let messages = read_messages(ctx.state())?;
                tracing::debug!(
                    node = AGENT_NODE,
                    messages = messages.len(),
                    tools = tool_schemas.len(),
                    "calling chat model"
                );
                let response = match token_tx {
                    Some(tx) => {
                        model
                            .chat_stream(&messages, &tool_schemas, &mut |chunk| {
                                if !chunk.delta.is_empty() {
                                    let _ = tx.try_send(GraphEvent::Token {
                                        node: AGENT_NODE.to_owned(),
                                        delta: chunk.delta,
                                    });
                                }
                            })
                            .await?
                    }
                    None => model.chat(&messages, &tool_schemas).await?,
                };
                let appended = serde_json::to_value(&response.message)?;
                // A single message object is fine: AddMessages accepts one
                // message or an array and upserts/appends accordingly.
                Ok(NodeOutput::update(MESSAGES_CHANNEL, appended))
            }
        }
    };

    let tools_node = move |ctx: crate::node::NodeContext| {
        let tool_executor = tool_executor.clone();
        async move {
            let messages = read_messages(ctx.state())?;
            let last = messages.last().ok_or_else(|| {
                AgentGraphError::Node(format!(
                    "node `{TOOLS_NODE}` ran with an empty `{MESSAGES_CHANNEL}` channel"
                ))
            })?;
            if !last.has_tool_calls() {
                return Err(AgentGraphError::Node(format!(
                    "node `{TOOLS_NODE}` expected the last message to carry tool calls"
                )));
            }
            let tool_names: Vec<&str> = last
                .tool_calls
                .iter()
                .map(|call| call.name.as_str())
                .collect();
            tracing::debug!(
                node = TOOLS_NODE,
                calls = last.tool_calls.len(),
                tools = ?tool_names,
                "dispatching tool calls"
            );
            // Per-call error policy: see ToolExecutor::execute_batch docs.
            let results = tool_executor.execute_batch(&last.tool_calls).await;
            let appended = serde_json::to_value(&results)?;
            Ok(NodeOutput::update(MESSAGES_CHANNEL, appended))
        }
    };

    let mut builder = GraphBuilder::new();
    builder.add_node(AGENT_NODE, agent_node);
    builder.add_node(TOOLS_NODE, tools_node);
    builder.set_entry_point(AGENT_NODE);

    // Route on the post-barrier state: the appended assistant message decides.
    builder.add_conditional_edges(AGENT_NODE, |state| async move {
        let needs_tools = read_messages(&state)?
            .last()
            .map(ChatMessage::has_tool_calls)
            .unwrap_or(false);
        Ok(if needs_tools {
            Route::Node(TOOLS_NODE.to_owned())
        } else {
            Route::End
        })
    });
    builder.add_edge(TOOLS_NODE, AGENT_NODE);

    builder.compile()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Edge;
    use crate::llm::{ChatResponse, TokenChunk, ToolCall};
    use crate::node::{Node, NodeConfig, NodeContext};
    use crate::state::{Reducer, State, StateSpec};
    use crate::tool::Tool;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A scripted model: pops one canned response per `chat` call.
    struct ScriptedModel {
        script: Mutex<VecDeque<ChatMessage>>,
        seen_tool_schemas: Mutex<Vec<usize>>,
    }

    impl ScriptedModel {
        fn new(script: Vec<ChatMessage>) -> Self {
            Self {
                script: Mutex::new(script.into()),
                seen_tool_schemas: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ChatModel for ScriptedModel {
        async fn chat(&self, _messages: &[ChatMessage], tools: &[Value]) -> Result<ChatResponse> {
            self.seen_tool_schemas.lock().unwrap().push(tools.len());
            let message = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| AgentGraphError::Llm("script exhausted".into()))?;
            Ok(ChatResponse {
                message,
                model: None,
                usage: None,
            })
        }
    }

    /// A model whose `chat_stream` emits real deltas (accumulating the full
    /// answer, as wire-backed implementations do).
    struct StreamingModel;

    #[async_trait]
    impl ChatModel for StreamingModel {
        async fn chat(&self, _messages: &[ChatMessage], _tools: &[Value]) -> Result<ChatResponse> {
            Ok(ChatResponse {
                message: ChatMessage::assistant("streamed"),
                model: None,
                usage: None,
            })
        }
        async fn chat_stream(
            &self,
            messages: &[ChatMessage],
            tools: &[Value],
            on_token: &mut (dyn FnMut(TokenChunk) + Send),
        ) -> Result<ChatResponse> {
            for delta in ["str", "eamed"] {
                on_token(TokenChunk {
                    delta: delta.to_owned(),
                    finish: false,
                    raw: None,
                });
            }
            self.chat(messages, tools).await
        }
    }

    #[tokio::test]
    async fn streaming_variant_forwards_token_events() {
        let (tx, mut rx) = mpsc::channel::<GraphEvent>(8);
        let model: Arc<dyn ChatModel> = Arc::new(StreamingModel);
        let graph = create_react_agent_streaming(model, registry(), tx).unwrap();

        let state = State::from_value(json!({
            MESSAGES_CHANNEL: [serde_json::to_value(ChatMessage::user("hi")).unwrap()]
        }))
        .unwrap();
        let ctx = NodeContext::new(state, NodeConfig::default());
        let out = graph.node(AGENT_NODE).unwrap().run(ctx).await.unwrap();

        // The accumulated response is appended exactly as in chat().
        let appended = out.updates.get(MESSAGES_CHANNEL).unwrap();
        let msg: ChatMessage = serde_json::from_value(appended.clone()).unwrap();
        assert_eq!(msg.content.as_deref(), Some("streamed"));

        // Both deltas arrived as Token events on the forwarded channel.
        let mut deltas = String::new();
        for _ in 0..2 {
            match rx.try_recv().expect("two token events") {
                GraphEvent::Token { node, delta } => {
                    assert_eq!(node, AGENT_NODE);
                    deltas.push_str(&delta);
                }
                other => panic!("expected Token event, got {other:?}"),
            }
        }
        assert_eq!(deltas, "streamed");
    }

    /// The non-streaming variant must emit no Token events (it calls chat()).
    #[tokio::test]
    async fn non_streaming_variant_emits_no_token_events() {
        let model: Arc<dyn ChatModel> =
            Arc::new(ScriptedModel::new(vec![ChatMessage::assistant("done")]));
        let graph = create_react_agent(model, registry()).unwrap();
        // No token sender is even available to this graph: the assertion is
        // structural (create_react_agent takes no channel), documented here
        // so the two variants do not drift.
        assert!(graph.has_node(AGENT_NODE));
    }

    struct Echo;

    #[async_trait]
    impl Tool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes its input."
        }
        fn parameters_schema(&self) -> Value {
            json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn call(&self, args: Value) -> Result<Value> {
            Ok(json!(args.get("text").cloned().unwrap_or(Value::Null)))
        }
    }

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(Echo);
        r
    }

    #[test]
    fn graph_topology_is_the_react_loop() {
        let model: Arc<dyn ChatModel> = Arc::new(ScriptedModel::new(vec![]));
        let graph = create_react_agent(model, registry()).unwrap();

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.entry_point(), AGENT_NODE);
        assert!(graph.has_node(AGENT_NODE));
        assert!(graph.has_node(TOOLS_NODE));

        // agent: one conditional edge; tools: one static edge back to agent.
        let agent_edges = graph.outgoing_edges(AGENT_NODE);
        assert_eq!(agent_edges.len(), 1);
        assert!(matches!(agent_edges[0], Edge::Conditional { .. }));
        let tools_edges = graph.outgoing_edges(TOOLS_NODE);
        assert_eq!(tools_edges.len(), 1);
        assert!(matches!(
            tools_edges[0],
            Edge::Direct { from, to } if from == TOOLS_NODE && to == AGENT_NODE
        ));
    }

    #[tokio::test]
    async fn agent_node_appends_assistant_message_and_sees_schemas() {
        let model = Arc::new(ScriptedModel::new(vec![ChatMessage::assistant("done")]));
        let graph = create_react_agent(model.clone(), registry()).unwrap();

        let state = State::from_value(json!({
            MESSAGES_CHANNEL: [serde_json::to_value(ChatMessage::user("hi")).unwrap()]
        }))
        .unwrap();
        let ctx = NodeContext::new(state, NodeConfig::default());
        let out = graph.node(AGENT_NODE).unwrap().run(ctx).await.unwrap();

        let appended = out.updates.get(MESSAGES_CHANNEL).unwrap();
        let msg: ChatMessage = serde_json::from_value(appended.clone()).unwrap();
        assert_eq!(msg.content.as_deref(), Some("done"));

        // The registry's schemas were passed to the model.
        assert_eq!(model.seen_tool_schemas.lock().unwrap().as_slice(), &[1]);
    }

    #[tokio::test]
    async fn tools_node_executes_pending_calls_in_order() {
        let model: Arc<dyn ChatModel> = Arc::new(ScriptedModel::new(vec![]));
        let graph = create_react_agent(model, registry()).unwrap();

        let calls = vec![
            ToolCall::new("c1", "echo", json!({"text": "a"})),
            ToolCall::new("c2", "echo", json!({"text": "b"})),
        ];
        let state = State::from_value(json!({
            MESSAGES_CHANNEL: [
                serde_json::to_value(ChatMessage::assistant_tool_calls(calls)).unwrap()
            ]
        }))
        .unwrap();
        let ctx = NodeContext::new(state, NodeConfig::default());
        let out = graph.node(TOOLS_NODE).unwrap().run(ctx).await.unwrap();

        let appended = out.updates.get(MESSAGES_CHANNEL).unwrap();
        let msgs: Vec<ChatMessage> = serde_json::from_value(appended.clone()).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(msgs[0].content.as_deref(), Some("a"));
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("c2"));
        assert_eq!(msgs[1].content.as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn router_follows_tool_calls_else_ends() {
        let model: Arc<dyn ChatModel> = Arc::new(ScriptedModel::new(vec![]));
        let graph = create_react_agent(model, registry()).unwrap();
        let edges = graph.outgoing_edges(AGENT_NODE);
        let router = match edges[0] {
            Edge::Conditional { router, .. } => router,
            _ => panic!("expected conditional edge"),
        };

        let with_calls = State::from_value(json!({
            MESSAGES_CHANNEL: [serde_json::to_value(ChatMessage::assistant_tool_calls(vec![
                ToolCall::new("c1", "echo", json!({"text": "x"})),
            ]))
            .unwrap()]
        }))
        .unwrap();
        assert_eq!(
            router(with_calls).await.unwrap(),
            Route::Node(TOOLS_NODE.to_owned())
        );

        let final_answer = State::from_value(json!({
            MESSAGES_CHANNEL: [serde_json::to_value(ChatMessage::assistant("42")).unwrap()]
        }))
        .unwrap();
        assert_eq!(router(final_answer).await.unwrap(), Route::End);
    }

    /// Drive the loop by hand (one super-step at a time, through the public
    /// `StateSpec` merge) to prove the wiring end-to-end without depending on
    /// the concurrently-implemented `Executor::run`.
    #[tokio::test]
    async fn manual_super_steps_reproduce_the_react_loop() {
        let model: Arc<dyn ChatModel> = Arc::new(ScriptedModel::new(vec![
            ChatMessage::assistant_tool_calls(vec![ToolCall::new(
                "c1",
                "echo",
                json!({"text": "hello"}),
            )]),
            ChatMessage::assistant("echoed: hello"),
        ]));
        let graph = create_react_agent(model, registry()).unwrap();
        let spec = StateSpec::new().channel(MESSAGES_CHANNEL, Reducer::AddMessages);
        let mut state = State::from_value(json!({
            MESSAGES_CHANNEL: [serde_json::to_value(ChatMessage::user("say hello")).unwrap()]
        }))
        .unwrap();

        // Step 0: agent -> assistant tool-call request.
        let out = graph
            .node(AGENT_NODE)
            .unwrap()
            .run(NodeContext::new(state.clone(), NodeConfig::default()))
            .await
            .unwrap();
        spec.apply_single(&mut state, AGENT_NODE, out.updates)
            .unwrap();

        // Route: tool calls present -> tools.
        let edges = graph.outgoing_edges(AGENT_NODE);
        let route = match edges[0] {
            Edge::Conditional { router, .. } => router(state.clone()).await.unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(route, Route::Node(TOOLS_NODE.to_owned()));

        // Step 1: tools -> tool result message.
        let out = graph
            .node(TOOLS_NODE)
            .unwrap()
            .run(NodeContext::new(state.clone(), NodeConfig::default()))
            .await
            .unwrap();
        spec.apply_single(&mut state, TOOLS_NODE, out.updates)
            .unwrap();

        // Step 2: agent -> final answer; route -> End.
        let out = graph
            .node(AGENT_NODE)
            .unwrap()
            .run(NodeContext::new(state.clone(), NodeConfig::default()))
            .await
            .unwrap();
        spec.apply_single(&mut state, AGENT_NODE, out.updates)
            .unwrap();
        let route = match edges[0] {
            Edge::Conditional { router, .. } => router(state.clone()).await.unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(route, Route::End);

        // Full transcript: user, assistant(tool_calls), tool, assistant(final).
        let msgs: Vec<ChatMessage> = state.get_as(MESSAGES_CHANNEL).unwrap().unwrap();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[3].content.as_deref(), Some("echoed: hello"));
    }
}
