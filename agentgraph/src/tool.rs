//! Tool abstraction, registry, and parallel tool-call dispatch.
//!
//! A [`Tool`] is an async callable with a JSON-schema-described parameter
//! surface. [`ToolRegistry`] holds the tools available to an agent and emits
//! OpenAI-format tool schemas for the chat API. [`ToolExecutor`] dispatches
//! a batch of [`crate::llm::ToolCall`]s **in parallel** (the `ToolNode`
//! pattern of the prebuilt ReAct agent) and returns one `role: "tool"`
//! message per call, preserving call order.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::FutureExt;
use serde_json::{json, Value};

use crate::error::{AgentGraphError, Result};
use crate::llm::{ChatMessage, ToolCall};

/// An invocable tool.
///
/// Implement directly for stateful tools, or wrap async closures with a
/// small adapter struct. `parameters_schema` should be a JSON Schema object
/// (`{"type": "object", "properties": {...}, "required": [...]}`).
#[async_trait]
pub trait Tool: Send + Sync {
    /// The tool name — must match what the model emits in `tool_calls`.
    fn name(&self) -> &str;

    /// Human/model-facing description used in the tool schema.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's arguments.
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with model-supplied arguments.
    async fn call(&self, args: Value) -> Result<Value>;
}

/// A registry of tools, shared cheaply via `Arc<dyn Tool>`.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ToolRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Re-registering the same name replaces the tool.
    pub fn register<T: Tool + 'static>(&mut self, tool: T) -> &mut Self {
        self.tools.insert(tool.name().to_owned(), Arc::new(tool));
        self
    }

    /// Register a pre-shared tool.
    pub fn register_shared(&mut self, tool: Arc<dyn Tool>) -> &mut Self {
        self.tools.insert(tool.name().to_owned(), tool);
        self
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// `true` if a tool with this name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// All registered tool names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// `true` if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// OpenAI-format tool schemas for the chat API, one per registered tool:
    /// `{"type": "function", "function": {"name", "description", "parameters"}}`.
    /// Pass directly as the `tools` argument of
    /// [`crate::llm::ChatModel::chat`].
    pub fn schemas(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }
}

/// Dispatches tool calls against a registry, in parallel.
///
/// Typical use in a ReAct `tools` node: take the assistant message's
/// `tool_calls`, `execute_batch` them, and append the resulting tool
/// messages to the `messages` channel via the `AddMessages` reducer.
#[derive(Debug, Clone, Default)]
pub struct ToolExecutor {
    registry: ToolRegistry,
}

impl ToolExecutor {
    /// An executor over `registry`.
    pub fn new(registry: ToolRegistry) -> Self {
        Self { registry }
    }

    /// The underlying registry.
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Execute a batch of tool calls concurrently.
    ///
    /// Returns one [`ChatMessage::tool_result`] per call, **in the same
    /// order as `calls`** (order stability matters for conversation
    /// reconstruction). Individual failures do not abort the batch: a failed
    /// call yields a tool message whose content is the error description
    /// (prefixed with `ERROR:`), so the model can observe and recover from
    /// tool failures — matching `ToolNode`'s default `handle_tool_errors`
    /// behavior. A *panicking* tool is contained the same way: the unwind is
    /// caught and reported as an `ERROR:` tool message instead of taking
    /// down the batch (and the executor task driving it).
    pub async fn execute_batch(&self, calls: &[ToolCall]) -> Vec<ChatMessage> {
        let futures = calls.iter().map(|call| {
            let registry = self.registry.clone();
            async move {
                let result = std::panic::AssertUnwindSafe(async {
                    let tool = registry.get(&call.name).ok_or_else(|| {
                        AgentGraphError::Tool(format!("unknown tool `{}`", call.name))
                    })?;
                    let value = tool.call(call.arguments.clone()).await?;
                    Ok::<String, AgentGraphError>(match value {
                        Value::String(s) => s,
                        other => other.to_string(),
                    })
                })
                .catch_unwind()
                .await;
                match result {
                    Ok(Ok(content)) => ChatMessage::tool_result(&call.id, content),
                    Ok(Err(e)) => ChatMessage::tool_result(&call.id, format!("ERROR: {e}")),
                    Err(payload) => ChatMessage::tool_result(
                        &call.id,
                        format!(
                            "ERROR: tool `{}` panicked: {}",
                            call.name,
                            // `&*`: `&payload` would unsize-coerce the *Box*
                            // itself into `&dyn Any`, hiding the real payload.
                            panic_message(&*payload)
                        ),
                    ),
                }
            }
        });
        futures::future::join_all(futures).await
    }
}

/// Best-effort extraction of a panic payload for error reporting.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string payload>".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    struct Fail;

    #[async_trait]
    impl Tool for Fail {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails."
        }
        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, _args: Value) -> Result<Value> {
            Err(AgentGraphError::Tool("boom".into()))
        }
    }

    #[test]
    fn registry_schemas_are_openai_shaped() {
        let mut registry = ToolRegistry::new();
        registry.register(Echo);
        let schemas = registry.schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0]["type"], json!("function"));
        assert_eq!(schemas[0]["function"]["name"], json!("echo"));
        assert!(schemas[0]["function"]["parameters"]["properties"].is_object());
    }

    #[tokio::test]
    async fn batch_preserves_order_and_isolates_failures() {
        let mut registry = ToolRegistry::new();
        registry.register(Echo);
        registry.register(Fail);
        let executor = ToolExecutor::new(registry);

        let calls = vec![
            ToolCall::new("c1", "echo", json!({"text": "hello"})),
            ToolCall::new("c2", "fail", json!({})),
            ToolCall::new("c3", "missing", json!({})),
        ];
        let results = executor.execute_batch(&calls).await;

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(results[0].content.as_deref(), Some("hello"));
        assert_eq!(results[1].tool_call_id.as_deref(), Some("c2"));
        assert!(results[1].content.as_deref().unwrap().starts_with("ERROR:"));
        assert_eq!(results[2].tool_call_id.as_deref(), Some("c3"));
        assert!(results[2]
            .content
            .as_deref()
            .unwrap()
            .contains("unknown tool"));
    }

    struct Panic;

    #[async_trait]
    impl Tool for Panic {
        fn name(&self) -> &str {
            "panic"
        }
        fn description(&self) -> &str {
            "Always panics."
        }
        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, _args: Value) -> Result<Value> {
            panic!("kaboom");
        }
    }

    #[tokio::test]
    async fn panicking_tool_is_contained_as_error_message() {
        let mut registry = ToolRegistry::new();
        registry.register(Echo);
        registry.register(Panic);
        let executor = ToolExecutor::new(registry);

        let calls = vec![
            ToolCall::new("c1", "panic", json!({})),
            ToolCall::new("c2", "echo", json!({"text": "still alive"})),
        ];
        let results = executor.execute_batch(&calls).await;

        // The panic joins the same ERROR: channel as ordinary failures, and
        // the rest of the batch completes normally.
        assert_eq!(results.len(), 2);
        let msg = results[0].content.as_deref().unwrap();
        assert!(msg.starts_with("ERROR:"), "got: {msg}");
        assert!(msg.contains("panicked"), "got: {msg}");
        assert!(msg.contains("kaboom"), "got: {msg}");
        assert_eq!(results[1].content.as_deref(), Some("still alive"));
    }
}
