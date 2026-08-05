//! LLM abstraction and an OpenAI-compatible chat client.
//!
//! [`ChatModel`] is the minimal async chat-completion interface used by
//! agent nodes (the prebuilt ReAct agent only needs `chat`). Messages use
//! the OpenAI wire conventions: roles, assistant `tool_calls`, and tool
//! results carried by `role: "tool"` messages with `tool_call_id`.

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};

use crate::error::{AgentGraphError, Result};

/// Chat message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System prompt / instructions.
    System,
    /// End-user input.
    User,
    /// Model output (may carry tool calls).
    Assistant,
    /// Tool execution result (must carry `tool_call_id`).
    Tool,
}

/// A single chat message.
///
/// Serialization follows the OpenAI chat-completions schema:
/// `content` may be null on assistant tool-call messages; `tool_calls` and
/// `tool_call_id` are omitted when absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Who produced this message.
    pub role: Role,

    /// Text content. `None` is legal for assistant messages that only carry
    /// tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Tool calls requested by the assistant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,

    /// Required on `role: tool` messages: the tool call this answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Optional participant name (multi-agent disambiguation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    /// A system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    /// A user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    /// An assistant message with text content.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    /// An assistant message requesting tool calls (content may be empty).
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: None,
            tool_calls,
            tool_call_id: None,
            name: None,
        }
    }

    /// A tool-result message answering `tool_call_id`.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }

    /// `true` if this is an assistant message requesting tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// A tool call requested by the model.
///
/// Wire format is the OpenAI function-calling shape:
/// `{"id": "...", "type": "function", "function": {"name": "...", "arguments": "<json string>"}}`.
/// The `arguments` field is exposed as a parsed [`Value`]; serialization
/// re-encodes it to the string form the API expects.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// Provider-assigned call id (echoed back in the tool-result message).
    pub id: String,
    /// Tool name (must match a registered [`crate::tool::Tool::name`]).
    pub name: String,
    /// Parsed arguments.
    pub arguments: Value,
}

impl ToolCall {
    /// Convenience constructor.
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

impl Serialize for ToolCall {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        json!({
            "id": self.id,
            "type": "function",
            "function": {
                "name": self.name,
                "arguments": self.arguments.to_string(),
            }
        })
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolCall {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct FnArgs {
            name: String,
            arguments: Value,
        }
        #[derive(Deserialize)]
        struct Wire {
            id: String,
            function: FnArgs,
        }
        let wire = Wire::deserialize(deserializer)?;
        let arguments = match wire.function.arguments {
            // Standard: arguments arrive as a JSON-encoded string.
            Value::String(s) => serde_json::from_str(&s).map_err(serde::de::Error::custom)?,
            // Lenient: some providers send a raw object.
            other => other,
        };
        Ok(ToolCall {
            id: wire.id,
            name: wire.function.name,
            arguments,
        })
    }
}

/// Token usage accounting from the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the prompt.
    #[serde(default)]
    pub prompt_tokens: u64,
    /// Tokens in the completion.
    #[serde(default)]
    pub completion_tokens: u64,
    /// Total tokens billed.
    #[serde(default)]
    pub total_tokens: u64,
}

/// One chat-completion response (single choice; multi-choice responses are
/// not modeled).
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// The assistant message (text and/or tool calls).
    pub message: ChatMessage,
    /// The model that produced the response, when reported.
    pub model: Option<String>,
    /// Token usage, when reported.
    pub usage: Option<Usage>,
}

/// One incremental piece of a streamed completion (the LangGraph `messages`
/// stream-mode analog at the model level).
///
/// `on_token` callbacks of [`ChatModel::chat_stream`] receive a sequence of
/// `TokenChunk`s: zero or more with `finish: false` carrying text deltas,
/// terminated by exactly one with `finish: true` (whose `delta` is empty for
/// truly-streaming implementations).
#[derive(Debug, Clone)]
pub struct TokenChunk {
    /// The incremental text produced since the previous chunk. May be empty
    /// (e.g. on the terminal chunk, or on chunks that only carry tool-call
    /// deltas).
    pub delta: String,

    /// `true` on the final chunk of the stream.
    pub finish: bool,

    /// The raw provider chunk (the decoded SSE `data:` JSON), when the
    /// implementation streams from a wire protocol. `None` for synthetic
    /// chunks (default fallback, mocks).
    pub raw: Option<Value>,
}

/// The chat-model interface used by agent nodes.
///
/// `tools` are OpenAI-format tool schemas (`{"type": "function", "function":
/// {...}}`); pass an empty slice for a plain completion. See
/// [`crate::tool::ToolRegistry::schemas`].
///
/// # Streaming tokens into the executor's event channel
///
/// [`ChatModel::chat_stream`] is pull-based: the `on_token` callback fires
/// once per token delta. To surface those deltas as
/// [`crate::executor::GraphEvent::Token`]s (the LangGraph `messages` stream
/// mode), clone the run's event sender into the node closure and forward
/// each chunk — the executor's `event_tx` channel is the shared sink:
///
/// ```ignore
/// use agentgraph::executor::{GraphEvent, RunConfig};
/// use agentgraph::llm::{ChatModel, ChatMessage, TokenChunk};
///
/// let (tx, mut rx) = tokio::sync::mpsc::channel::<GraphEvent>(64);
/// let node_tx = tx.clone();                  // captured by the node closure
/// let config = RunConfig::new("t-1").with_event_tx(tx);
/// // Convenience handles for wiring the clone into nodes:
/// //   RunConfig::token_tx()  -> Option<mpsc::Sender<GraphEvent>>
/// //   Executor::with_token_tx(tx) / Executor::token_tx()
///
/// // ...inside the node:
/// // let response = model
/// //     .chat_stream(&messages, &tools, &mut |chunk: TokenChunk| {
/// //         if !chunk.delta.is_empty() {
/// //             let _ = node_tx.try_send(GraphEvent::Token {
/// //                 node: "agent".into(),
/// //                 delta: chunk.delta,
/// //             });
/// //         }
/// //     })
/// //     .await?;
/// ```
///
/// Forwarding uses `try_send` (best-effort), matching the executor's own
/// emission policy: a full or closed channel drops tokens but never aborts
/// the run.
#[async_trait]
pub trait ChatModel: Send + Sync {
    /// Produce the next assistant message for the conversation.
    async fn chat(&self, messages: &[ChatMessage], tools: &[Value]) -> Result<ChatResponse>;

    /// Produce the next assistant message, streaming token deltas through
    /// `on_token` as they arrive.
    ///
    /// The default implementation falls back to [`ChatModel::chat`] and
    /// delivers the whole assistant text as a single [`TokenChunk`] with
    /// `finish: true`, so existing implementors remain source-compatible.
    /// Implementations with a streaming wire protocol (e.g.
    /// [`OpenAiCompatibleClient`]) override this to deliver real deltas.
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        on_token: &mut (dyn FnMut(TokenChunk) + Send),
    ) -> Result<ChatResponse> {
        let response = self.chat(messages, tools).await?;
        on_token(TokenChunk {
            delta: response.message.content.clone().unwrap_or_default(),
            finish: true,
            raw: None,
        });
        Ok(response)
    }
}

/// A client for any OpenAI-compatible `/chat/completions` endpoint (OpenAI,
/// Azure-OpenAI-compatible gateways, vLLM, Ollama, LM Studio, ...).
///
/// Uses `reqwest` with rustls; no default TLS features.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleClient {
    base_url: String,
    api_key: Option<String>,
    model: String,
    client: reqwest::Client,
}

impl OpenAiCompatibleClient {
    /// A client for `base_url` (e.g. `https://api.openai.com/v1`) serving
    /// `model`. Trailing slashes on `base_url` are trimmed.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Read the API key from an environment variable.
    pub fn from_env(
        base_url: impl Into<String>,
        api_key_env: &str,
        model: impl Into<String>,
    ) -> Self {
        Self::new(base_url, std::env::var(api_key_env).ok(), model)
    }

    /// Override the underlying `reqwest::Client` (timeouts, proxies, ...).
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    /// The model this client requests.
    pub fn model(&self) -> &str {
        &self.model
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

/// Wire shape of one completion choice.
#[derive(Deserialize)]
struct WireChoice {
    message: ChatMessage,
}

/// Wire shape of the completion response body.
#[derive(Deserialize)]
struct WireResponse {
    choices: Vec<WireChoice>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

/// Wire shape of one streaming chunk (`stream: true`).
#[derive(Deserialize)]
struct WireStreamChunk {
    #[serde(default)]
    choices: Vec<WireStreamChoice>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

/// Wire shape of one streaming choice.
///
/// Note: the wire's `finish_reason` field is deliberately not modeled here.
/// With `stream_options.include_usage`, OpenAI-compatible servers send the
/// terminal usage chunk *after* the chunk whose choice carries
/// `finish_reason: "stop"`, so terminating on `finish_reason` would drop
/// usage accounting. Stream termination is instead driven by the `[DONE]`
/// sentinel (with end-of-body as the fallback for providers that omit it).
#[derive(Deserialize)]
struct WireStreamChoice {
    delta: WireStreamDelta,
}

/// Wire shape of the incremental delta inside a streaming chunk.
#[derive(Deserialize)]
struct WireStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCallDelta>>,
}

/// Wire shape of an incremental tool-call delta (indexed slots; `id`,
/// `name`, and `arguments` arrive piecewise and are concatenated per index).
#[derive(Deserialize)]
struct WireToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<WireFunctionDelta>,
}

/// Wire shape of the function fragment inside a tool-call delta.
#[derive(Deserialize)]
struct WireFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Accumulates streaming deltas into a final [`ChatResponse`].
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    tool_calls: Vec<ToolCallAccumulator>,
    model: Option<String>,
    usage: Option<Usage>,
}

/// Per-index accumulation of one streamed tool call.
#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

impl StreamAccumulator {
    fn into_response(self) -> Result<ChatResponse> {
        let mut tool_calls = Vec::with_capacity(self.tool_calls.len());
        for (index, acc) in self.tool_calls.into_iter().enumerate() {
            let arguments = if acc.arguments.trim().is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(&acc.arguments).map_err(|e| {
                    AgentGraphError::Llm(format!(
                        "malformed tool-call arguments in stream (index {index}): {e}"
                    ))
                })?
            };
            tool_calls.push(ToolCall::new(acc.id, acc.name, arguments));
        }
        let content = if self.content.is_empty() {
            None
        } else {
            Some(self.content)
        };
        Ok(ChatResponse {
            message: ChatMessage {
                role: Role::Assistant,
                content,
                tool_calls,
                tool_call_id: None,
                name: None,
            },
            model: self.model,
            usage: self.usage,
        })
    }
}

/// A minimal hand-rolled Server-Sent-Events decoder.
///
/// SSE is a line protocol over an arbitrarily chunked byte stream: events
/// are separated by blank lines, each event's `data:` lines (possibly
/// several) join with `\n` into one payload, `:`-prefixed lines are
/// comments/heartbeats, and other fields (`event:`, `id:`, `retry:`) are
/// ignored. The decoder buffers partial lines across `feed` calls, so a
/// single `data:` line split across TCP chunks still decodes correctly.
#[derive(Default)]
struct SseDecoder {
    /// Bytes received but not yet terminated by `\n`.
    buf: String,
    /// `data:` lines of the event currently being assembled.
    data_lines: Vec<String>,
}

impl SseDecoder {
    fn new() -> Self {
        Self::default()
    }

    /// Feed a text fragment; returns the payloads of all events completed by
    /// it (usually zero or one).
    fn feed(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=pos).collect();
            if let Some(payload) = self.process_line(line.trim_end_matches(['\n', '\r'])) {
                events.push(payload);
            }
        }
        events
    }

    /// Flush any unterminated trailing line and any event that ended without
    /// its blank-line terminator (end of stream).
    fn finish(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            if let Some(payload) = self.process_line(line.trim_end_matches('\r')) {
                events.push(payload);
            }
        }
        if !self.data_lines.is_empty() {
            events.push(self.data_lines.join("\n"));
            self.data_lines.clear();
        }
        events
    }

    /// Process one complete line; returns the event payload when a blank
    /// line terminates an event that carried `data:`.
    fn process_line(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            if self.data_lines.is_empty() {
                return None;
            }
            let payload = self.data_lines.join("\n");
            self.data_lines.clear();
            return Some(payload);
        }
        if line.starts_with(':') {
            return None; // comment / heartbeat
        }
        if let Some(data) = line.strip_prefix("data:") {
            // Per spec, a single leading space after the colon is stripped.
            self.data_lines
                .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
        }
        None
    }
}

/// Apply one decoded SSE `data:` payload to the accumulator, invoking
/// `on_token` for text deltas. Returns `Ok(true)` on the terminal `[DONE]`
/// sentinel.
fn handle_sse_payload(
    payload: &str,
    acc: &mut StreamAccumulator,
    on_token: &mut (dyn FnMut(TokenChunk) + Send),
) -> Result<bool> {
    let trimmed = payload.trim();
    if trimmed == "[DONE]" {
        return Ok(true);
    }

    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| AgentGraphError::Llm(format!("malformed stream chunk: {e}")))?;
    let chunk: WireStreamChunk = serde_json::from_value(value.clone())
        .map_err(|e| AgentGraphError::Llm(format!("malformed stream chunk: {e}")))?;

    if chunk.model.is_some() {
        acc.model = chunk.model;
    }
    if chunk.usage.is_some() {
        acc.usage = chunk.usage;
    }

    if let Some(choice) = chunk.choices.into_iter().next() {
        let delta = choice.delta;
        if let Some(content) = delta.content {
            if !content.is_empty() {
                acc.content.push_str(&content);
                on_token(TokenChunk {
                    delta: content,
                    finish: false,
                    raw: Some(value),
                });
            }
        }
        if let Some(calls) = delta.tool_calls {
            for call in calls {
                if acc.tool_calls.len() <= call.index {
                    acc.tool_calls
                        .resize_with(call.index + 1, ToolCallAccumulator::default);
                }
                let slot = &mut acc.tool_calls[call.index];
                if let Some(id) = call.id {
                    slot.id.push_str(&id);
                }
                if let Some(function) = call.function {
                    if let Some(name) = function.name {
                        slot.name.push_str(&name);
                    }
                    if let Some(arguments) = function.arguments {
                        slot.arguments.push_str(&arguments);
                    }
                }
            }
        }
    }
    Ok(false)
}

#[async_trait]
impl ChatModel for OpenAiCompatibleClient {
    async fn chat(&self, messages: &[ChatMessage], tools: &[Value]) -> Result<ChatResponse> {
        let mut body = json!({
            "model": self.model,
            "messages": messages,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }

        let mut request = self.client.post(self.endpoint()).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }

        let response = request.send().await.map_err(|e| {
            AgentGraphError::Llm(format!("request to {} failed: {e}", self.base_url))
        })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AgentGraphError::Llm(format!(
                "chat completions returned {status}: {}",
                text.chars().take(512).collect::<String>()
            )));
        }

        let wire: WireResponse = response.json().await.map_err(|e| {
            AgentGraphError::Llm(format!("malformed chat completions response: {e}"))
        })?;

        let choice =
            wire.choices.into_iter().next().ok_or_else(|| {
                AgentGraphError::Llm("chat completions returned zero choices".into())
            })?;

        Ok(ChatResponse {
            message: choice.message,
            model: wire.model,
            usage: wire.usage,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        on_token: &mut (dyn FnMut(TokenChunk) + Send),
    ) -> Result<ChatResponse> {
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            // Ask for a final usage chunk (supported by OpenAI, vLLM, ...);
            // providers that ignore it simply omit `usage`.
            "stream_options": {"include_usage": true},
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }

        let mut request = self.client.post(self.endpoint()).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }

        let mut response = request.send().await.map_err(|e| {
            AgentGraphError::Llm(format!("request to {} failed: {e}", self.base_url))
        })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AgentGraphError::Llm(format!(
                "chat completions returned {status}: {}",
                text.chars().take(512).collect::<String>()
            )));
        }

        // Read the body as raw bytes and decode SSE manually (`chunk()` is
        // used because the `stream` feature of reqwest is not enabled; the
        // SseDecoder is byte-chunk agnostic either way).
        let mut decoder = SseDecoder::new();
        let mut acc = StreamAccumulator::default();
        let mut done = false;

        while !done {
            let bytes = match response.chunk().await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break, // end of body
                Err(e) => {
                    return Err(AgentGraphError::Llm(format!(
                        "stream read from {} failed: {e}",
                        self.base_url
                    )))
                }
            };
            for payload in decoder.feed(&String::from_utf8_lossy(&bytes)) {
                if handle_sse_payload(&payload, &mut acc, on_token)? {
                    done = true;
                    break;
                }
            }
        }
        if !done {
            // Stream ended without `[DONE]`: flush whatever the decoder holds.
            for payload in decoder.finish() {
                if handle_sse_payload(&payload, &mut acc, on_token)? {
                    break;
                }
            }
        }

        on_token(TokenChunk {
            delta: String::new(),
            finish: true,
            raw: None,
        });
        acc.into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_wire_roundtrip() {
        let call = ToolCall::new("call_1", "search", json!({"q": "rust"}));
        let serialized = serde_json::to_value(&call).unwrap();
        assert_eq!(
            serialized,
            json!({
                "id": "call_1",
                "type": "function",
                "function": {"name": "search", "arguments": "{\"q\":\"rust\"}"}
            })
        );
        let back: ToolCall = serde_json::from_value(serialized).unwrap();
        assert_eq!(back, call);
    }

    #[test]
    fn message_builders() {
        let m = ChatMessage::tool_result("call_1", "42");
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.tool_call_id.as_deref(), Some("call_1"));

        let m = ChatMessage::assistant_tool_calls(vec![ToolCall::new("c", "t", json!({}))]);
        assert!(m.has_tool_calls());
        assert_eq!(m.content, None);

        // Roles serialize to OpenAI lowercase strings.
        assert_eq!(
            serde_json::to_value(Role::Assistant).unwrap(),
            json!("assistant")
        );
    }

    #[test]
    fn sse_decoder_handles_multi_chunk_delivery() {
        let mut decoder = SseDecoder::new();
        // One event split across two arbitrary byte chunks (split inside a
        // `data:` line), followed by a comment and a blank-line terminator;
        // then a CRLF-framed event.
        assert!(decoder.feed("data: {\"hel").is_empty());
        assert_eq!(
            decoder.feed("lo\"}\n: heartbeat\n\nda"),
            vec!["{\"hello\"}".to_string()]
        );
        assert_eq!(decoder.feed("ta: world\r\n\r\n"), vec!["world".to_string()]);
    }

    #[test]
    fn sse_decoder_joins_multi_line_data_and_flushes_trailing_event() {
        let mut decoder = SseDecoder::new();
        // Multiple `data:` lines in one event join with `\n` per the SSE spec.
        assert_eq!(
            decoder.feed("data: a\ndata: b\n\n"),
            vec!["a\nb".to_string()]
        );
        // A final event with no blank-line terminator is flushed by finish().
        assert!(decoder.feed("data: tail").is_empty());
        assert_eq!(decoder.finish(), vec!["tail".to_string()]);
        // A blank line with no pending `data:` is not an event.
        assert!(SseDecoder::new().process_line("").is_none());
    }

    #[test]
    fn sse_done_sentinel_terminates_and_content_deltas_accumulate() {
        let mut acc = StreamAccumulator::default();
        let mut deltas: Vec<String> = Vec::new();
        let mut on_token = |chunk: TokenChunk| {
            assert!(!chunk.finish);
            assert!(chunk.raw.is_some(), "wire chunks carry the raw JSON");
            deltas.push(chunk.delta);
        };

        let done = handle_sse_payload(
            r#"{"choices":[{"delta":{"content":"Hel"},"finish_reason":null}]}"#,
            &mut acc,
            &mut on_token,
        )
        .unwrap();
        assert!(!done);
        let done = handle_sse_payload(
            r#"{"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}],
                "model":"gpt-x","usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#,
            &mut acc,
            &mut on_token,
        )
        .unwrap();
        assert!(!done);
        // The [DONE] sentinel terminates the stream and is not parsed as JSON.
        assert!(handle_sse_payload("[DONE]", &mut acc, &mut on_token).unwrap());

        assert_eq!(deltas, ["Hel", "lo"]);
        let response = acc.into_response().unwrap();
        assert_eq!(response.message.content.as_deref(), Some("Hello"));
        assert_eq!(response.model.as_deref(), Some("gpt-x"));
        assert_eq!(response.usage.unwrap().total_tokens, 3);
    }

    #[test]
    fn sse_stream_accumulates_tool_call_deltas() {
        let mut acc = StreamAccumulator::default();
        let mut on_token = |_chunk: TokenChunk| {};
        // id/name/arguments arrive piecewise across chunks at the same index.
        for payload in [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"search","arguments":"{\"q\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"rust\"}"}}]}}]}"#,
        ] {
            assert!(!handle_sse_payload(payload, &mut acc, &mut on_token).unwrap());
        }
        let response = acc.into_response().unwrap();
        assert_eq!(
            response.message.tool_calls,
            vec![ToolCall::new("call_1", "search", json!({"q": "rust"}))]
        );
        assert_eq!(response.message.content, None);
    }

    /// A model that only implements `chat` (the pre-streaming API surface).
    struct NonStreamingMock;

    #[async_trait]
    impl ChatModel for NonStreamingMock {
        async fn chat(&self, _messages: &[ChatMessage], _tools: &[Value]) -> Result<ChatResponse> {
            Ok(ChatResponse {
                message: ChatMessage::assistant("full answer"),
                model: Some("mock".into()),
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn chat_stream_default_falls_back_to_single_chunk() {
        let model = NonStreamingMock;
        let mut chunks: Vec<TokenChunk> = Vec::new();
        let response = model
            .chat_stream(&[], &[], &mut |chunk| chunks.push(chunk))
            .await
            .unwrap();

        // Exactly one terminal chunk carrying the whole message.
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].delta, "full answer");
        assert!(chunks[0].finish);
        assert!(chunks[0].raw.is_none());
        assert_eq!(
            response.message.content.as_deref(),
            Some("full answer"),
            "the fallback returns the chat() response unchanged"
        );
    }
}
