//! Demo server: a two-node pipeline graph plus a ReAct agent (scripted
//! `ChatModel` — no network), served on `127.0.0.1:8100`.
//!
//! Run with: `cargo run --example server_demo`

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use rusty_agent_runtime::prelude::*;
use rusty_server::{serve, GraphRegistry, ServerConfig};
use async_trait::async_trait;
use serde_json::{json, Value};

/// A scripted model: pops one canned response per call; once the script is
/// exhausted it always answers "done" (so repeated runs keep working).
struct ScriptedModel {
    script: Mutex<VecDeque<ChatMessage>>,
}

#[async_trait]
impl ChatModel for ScriptedModel {
    async fn chat(&self, _messages: &[ChatMessage], _tools: &[Value]) -> Result<ChatResponse> {
        let message = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| ChatMessage::assistant("done"));
        Ok(ChatResponse {
            message,
            model: Some("scripted".to_string()),
            usage: None,
        })
    }
}

/// Trivial echo tool for the ReAct agent.
struct Echo;

#[async_trait]
impl Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echoes its `text` argument back."
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {"text": {"type": "string"}}})
    }
    async fn call(&self, args: Value) -> Result<Value> {
        Ok(args.get("text").cloned().unwrap_or(Value::Null))
    }
}

/// `first -> second`, appending to a `log` channel.
fn build_pipeline_graph() -> Result<(Graph, StateSpec)> {
    let spec = StateSpec::new().channel("log", Reducer::Append);
    let mut builder = GraphBuilder::new();
    builder.add_node("first", |_ctx: NodeContext| async {
        Ok(NodeOutput::update("log", json!("first")))
    });
    builder.add_node("second", |_ctx: NodeContext| async {
        Ok(NodeOutput::update("log", json!("second")))
    });
    builder.set_entry_point("first");
    builder.add_edge("first", "second");
    Ok((builder.compile()?, spec))
}

/// ReAct agent over a scripted model: one tool call, then a final answer.
fn build_react_graph() -> Result<(Graph, StateSpec)> {
    let mut tools = ToolRegistry::new();
    tools.register(Echo);
    let model: Arc<dyn ChatModel> = Arc::new(ScriptedModel {
        script: Mutex::new(VecDeque::from(vec![
            ChatMessage::assistant_tool_calls(vec![ToolCall::new(
                "call_1",
                "echo",
                json!({"text": "pong"}),
            )]),
            ChatMessage::assistant("The echo tool said: pong."),
        ])),
    });
    let graph = create_react_agent(model, tools)?;
    let spec = StateSpec::new().channel("messages", Reducer::AddMessages);
    Ok((graph, spec))
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let (pipeline, pipeline_spec) = build_pipeline_graph()?;
    let (react, react_spec) = build_react_graph()?;

    let mut registry = GraphRegistry::new();
    registry.register("pipeline", pipeline, pipeline_spec);
    registry.register("react_agent", react, react_spec);

    let config = ServerConfig::new(
        "127.0.0.1:8100".parse().unwrap(),
        "./data/server-demo-checkpoints",
    );

    println!("\nrusty-server demo on http://127.0.0.1:8100\n");
    println!("  # liveness + registered graphs");
    println!("  curl localhost:8100/ok");
    println!("  curl localhost:8100/info | jq\n");
    println!("  # create a thread (pipeline graph)");
    println!("  THREAD=$(curl -s -X POST localhost:8100/threads \\");
    println!("    -H 'content-type: application/json' \\");
    println!("    -d '{{\"graph\": \"pipeline\"}}' | jq -r .thread_id)\n");
    println!("  # blocking run");
    println!("  curl -s -X POST localhost:8100/threads/$THREAD/runs/wait \\");
    println!("    -H 'content-type: application/json' -d '{{}}' | jq\n");
    println!("  # streaming run (SSE)");
    println!("  curl -N -X POST localhost:8100/threads/$THREAD/runs/stream \\");
    println!("    -H 'content-type: application/json' -d '{{}}'\n");
    println!("  # state + history");
    println!("  curl -s localhost:8100/threads/$THREAD/state | jq");
    println!("  curl -s -X POST localhost:8100/threads/$THREAD/history \\");
    println!("    -H 'content-type: application/json' -d '{{}}' | jq\n");
    println!("  # ReAct agent (scripted model; no network)");
    println!("  REACT=$(curl -s -X POST localhost:8100/threads \\");
    println!("    -H 'content-type: application/json' \\");
    println!("    -d '{{\"graph\": \"react_agent\"}}' | jq -r .thread_id)");
    println!("  curl -s -X POST localhost:8100/threads/$REACT/runs/wait \\");
    println!("    -H 'content-type: application/json' \\");
    println!("    -d '{{\"input\": {{\"messages\": [{{\"role\": \"user\", \"content\": \"say pong\"}}]}}}}' | jq\n");

    serve(registry, config).await?;
    Ok(())
}
