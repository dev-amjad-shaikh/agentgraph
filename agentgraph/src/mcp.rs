//! MCP (Model Context Protocol) client support.
//!
//! This module lets `agentgraph` agents call tools hosted by **any MCP
//! server** — the ecosystem escape hatch. It provides:
//!
//! - JSON-RPC 2.0 framing types ([`JsonRpcRequest`], [`JsonRpcResponse`],
//!   [`JsonRpcNotification`], [`JsonRpcError`]) with `serde` support.
//! - A transport-generic [`McpClient`] over tokio `AsyncRead`/`AsyncWrite`,
//!   supporting both **newline-delimited JSON** (MCP stdio) and **LSP-style
//!   `Content-Length` headers** ([`Framing`]). Every request carries a
//!   timeout; a background reader task routes responses to their waiters.
//! - [`McpStdioClient::spawn`] to launch an MCP server as a child process
//!   with piped stdin/stdout.
//! - [`McpToolAdapter`], which wraps a single MCP tool as an `agentgraph`
//!   [`Tool`], and [`McpClient::into_tools`], which lists the server's tools
//!   and returns them as `Vec<Arc<dyn Tool>>` for direct registration in a
//!   [`crate::tool::ToolRegistry`].
//!
//! All failures map to [`AgentGraphError::Tool`] with an `mcp:` context
//! prefix.
//!
//! ```no_run
//! # async fn demo() -> agentgraph::error::Result<()> {
//! use agentgraph::mcp::McpStdioClient;
//!
//! let client = McpStdioClient::spawn("npx", &["-y", "@modelcontextprotocol/server-everything"])?;
//! client.initialize().await?;
//! let tools = client.into_tools().await?;
//! // register into a ToolRegistry and hand to a ReAct agent...
//! # let _ = tools;
//! client.shutdown().await?;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::io;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::{AgentGraphError, Result};
use crate::tool::Tool;

/// The MCP protocol revision this client requests during `initialize`.
///
/// `2024-11-05` is the most widely implemented revision; servers that do not
/// support it respond with a revision they do support, which this client
/// accepts.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Default per-request timeout.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Build an [`AgentGraphError::Tool`] with an `mcp:` context prefix.
fn tool_err(msg: impl Into<String>) -> AgentGraphError {
    AgentGraphError::Tool(format!("mcp: {}", msg.into()))
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 framing types
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Request identifier (this client uses monotonically increasing integers).
    pub id: u64,
    /// Method name, e.g. `"tools/call"`.
    pub method: String,
    /// Structured parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// A `"2.0"` request with parameters.
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            method: method.into(),
            params: Some(params),
        }
    }
}

/// A JSON-RPC 2.0 notification (no `id`, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Method name, e.g. `"notifications/initialized"`.
    pub method: String,
    /// Structured parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// A `"2.0"` notification with parameters.
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            method: method.into(),
            params: Some(params),
        }
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code (e.g. `-32602` invalid params, `-32603` internal error).
    pub code: i64,
    /// Human-readable message.
    pub message: String,
    /// Optional structured data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

/// A JSON-RPC 2.0 response. Exactly one of `result` / `error` is present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Echoed request identifier.
    pub id: Value,
    /// Success payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Failure payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

// ---------------------------------------------------------------------------
// Wire framing
// ---------------------------------------------------------------------------

/// How JSON-RPC messages are framed on the byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Framing {
    /// One JSON object per line — the MCP stdio transport.
    #[default]
    NewlineDelimited,
    /// LSP-style `Content-Length: N\r\n\r\n<body>` headers.
    ContentLength,
}

/// Write one framed JSON message.
async fn write_framed<W>(writer: &mut W, framing: Framing, value: &Value) -> io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    let body =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    match framing {
        Framing::NewlineDelimited => {
            writer.write_all(&body).await?;
            writer.write_all(b"\n").await?;
        }
        Framing::ContentLength => {
            let header = format!("Content-Length: {}\r\n\r\n", body.len());
            writer.write_all(header.as_bytes()).await?;
            writer.write_all(&body).await?;
        }
    }
    writer.flush().await
}

/// Read one framed JSON message. Returns `Ok(None)` on clean EOF.
async fn read_framed<R>(reader: &mut BufReader<R>, framing: Framing) -> io::Result<Option<Value>>
where
    R: AsyncRead + Unpin,
{
    match framing {
        Framing::NewlineDelimited => {
            let mut line = String::new();
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await?;
                if n == 0 {
                    return Ok(None); // EOF
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue; // tolerate blank lines
                }
                let value = serde_json::from_str(trimmed)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                return Ok(Some(value));
            }
        }
        Framing::ContentLength => {
            let mut content_length: Option<usize> = None;
            let mut line = String::new();
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await?;
                if n == 0 {
                    return Ok(None); // EOF before/inside headers
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    break; // end of headers
                }
                if let Some((name, val)) = trimmed.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        content_length = val.trim().parse().ok();
                    }
                }
            }
            let len = content_length.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
            })?;
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).await?;
            let value = serde_json::from_slice(&body)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Some(value))
        }
    }
}

// ---------------------------------------------------------------------------
// MCP metadata types
// ---------------------------------------------------------------------------

/// The parsed result of the MCP `initialize` handshake.
#[derive(Debug, Clone)]
pub struct InitializeResult {
    /// Protocol revision the server will use.
    pub protocol_version: String,
    /// Server implementation name (`serverInfo.name`).
    pub server_name: String,
    /// Server implementation version (`serverInfo.version`).
    pub server_version: String,
    /// Raw server capabilities object.
    pub capabilities: Value,
}

/// Metadata for one MCP tool, as returned by `tools/list`.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    /// Tool name.
    pub name: String,
    /// Human/model-facing description (may be empty).
    pub description: String,
    /// JSON Schema for the tool's arguments (`inputSchema`).
    pub input_schema: Value,
}

// ---------------------------------------------------------------------------
// McpClient
// ---------------------------------------------------------------------------

type BoxedWriter = Box<dyn AsyncWrite + Unpin + Send>;
type PendingMap = HashMap<u64, oneshot::Sender<Value>>;

struct ClientInner {
    framing: Framing,
    writer: Arc<Mutex<BoxedWriter>>,
    pending: Arc<Mutex<PendingMap>>,
    next_id: AtomicU64,
    closed: AtomicBool,
    initialized: AtomicBool,
    request_timeout: StdMutex<Duration>,
    reader_handle: StdMutex<Option<JoinHandle<()>>>,
    child: StdMutex<Option<Child>>,
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.reader_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.start_kill();
            }
        }
    }
}

/// Background task: reads framed messages from the server and routes them.
///
/// - Responses (`id` + `result`/`error`) are delivered to the matching
///   pending oneshot.
/// - Server-initiated requests (`method` + `id`) get a `-32601` reply, since
///   this client serves no methods.
/// - Notifications (`method`, no `id`) are ignored.
///
/// On EOF or a fatal read error the task drains `pending`, waking all waiters
/// with a "connection closed" error.
async fn reader_loop<R>(
    reader: R,
    framing: Framing,
    pending: Arc<Mutex<PendingMap>>,
    writer: Arc<Mutex<BoxedWriter>>,
) where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    loop {
        match read_framed(&mut reader, framing).await {
            Ok(Some(msg)) => {
                if msg.get("method").is_some() {
                    if let Some(id) = msg.get("id") {
                        let reply = json!({
                            "jsonrpc": "2.0",
                            "id": id.clone(),
                            "error": {"code": -32601, "message": "method not found"},
                        });
                        let mut w = writer.lock().await;
                        if write_framed(&mut **w, framing, &reply).await.is_err() {
                            break;
                        }
                    }
                    // else: notification — ignore.
                } else if let Some(id) = msg.get("id").and_then(Value::as_u64) {
                    let tx = pending.lock().await.remove(&id);
                    if let Some(tx) = tx {
                        let _ = tx.send(msg);
                    }
                }
            }
            Ok(None) => break, // clean EOF
            Err(_) => break,   // malformed frame or IO error — give up
        }
    }
    // Wake every waiter: dropping the senders makes the receivers fail.
    pending.lock().await.clear();
}

/// A transport-generic MCP client over a tokio byte stream.
///
/// Cheap to clone (all state is shared); clones see the same connection.
/// Use [`McpClient::connect`] for an arbitrary transport or
/// [`McpStdioClient::spawn`] for a child-process stdio server.
///
/// Lifecycle: `connect` → [`initialize`](McpClient::initialize) →
/// [`list_tools`](McpClient::list_tools) / [`call_tool`](McpClient::call_tool)
/// / [`into_tools`](McpClient::into_tools) → [`shutdown`](McpClient::shutdown).
#[derive(Clone)]
pub struct McpClient {
    inner: Arc<ClientInner>,
}

impl McpClient {
    /// Connect over an arbitrary transport using newline-delimited framing
    /// (the MCP stdio convention).
    pub fn connect<R, W>(reader: R, writer: W) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::connect_with_framing(reader, writer, Framing::NewlineDelimited)
    }

    /// Connect over an arbitrary transport with explicit framing.
    pub fn connect_with_framing<R, W>(reader: R, writer: W, framing: Framing) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::connect_inner(reader, writer, framing, None)
    }

    fn connect_inner<R, W>(reader: R, writer: W, framing: Framing, child: Option<Child>) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(HashMap::new()));
        let writer: Arc<Mutex<BoxedWriter>> = Arc::new(Mutex::new(Box::new(writer)));
        let handle = tokio::spawn(reader_loop(
            reader,
            framing,
            Arc::clone(&pending),
            Arc::clone(&writer),
        ));
        Self {
            inner: Arc::new(ClientInner {
                framing,
                writer,
                pending,
                next_id: AtomicU64::new(1),
                closed: AtomicBool::new(false),
                initialized: AtomicBool::new(false),
                request_timeout: StdMutex::new(DEFAULT_REQUEST_TIMEOUT),
                reader_handle: StdMutex::new(Some(handle)),
                child: StdMutex::new(child),
            }),
        }
    }

    /// The wire framing in use.
    pub fn framing(&self) -> Framing {
        self.inner.framing
    }

    /// `true` once [`initialize`](McpClient::initialize) has completed.
    pub fn is_initialized(&self) -> bool {
        self.inner.initialized.load(Ordering::Relaxed)
    }

    /// Override the per-request timeout (default: 30 s).
    pub fn set_request_timeout(&self, timeout: Duration) {
        if let Ok(mut guard) = self.inner.request_timeout.lock() {
            *guard = timeout;
        }
    }

    fn request_timeout(&self) -> Duration {
        self.inner
            .request_timeout
            .lock()
            .map(|d| *d)
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT)
    }

    fn ensure_open(&self) -> Result<()> {
        if self.inner.closed.load(Ordering::Relaxed) {
            return Err(tool_err("client is shut down"));
        }
        Ok(())
    }

    fn ensure_initialized(&self) -> Result<()> {
        if !self.is_initialized() {
            return Err(tool_err(
                "client is not initialized; call `initialize()` first",
            ));
        }
        Ok(())
    }

    /// Send a request and await its response, with timeout and JSON-RPC
    /// error mapping.
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.ensure_open()?;
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params);
        let encoded = serde_json::to_value(&request)
            .map_err(|e| tool_err(format!("failed to encode `{method}` request: {e}")))?;

        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

        {
            let mut w = self.inner.writer.lock().await;
            if let Err(e) = write_framed(&mut **w, self.inner.framing, &encoded).await {
                self.inner.pending.lock().await.remove(&id);
                return Err(tool_err(format!("failed to send `{method}` request: {e}")));
            }
        }

        let raw = match timeout(self.request_timeout(), rx).await {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => {
                return Err(tool_err(format!(
                    "connection closed while awaiting `{method}` response"
                )));
            }
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(tool_err(format!(
                    "`{method}` request timed out after {:?}",
                    self.request_timeout()
                )));
            }
        };

        let response: JsonRpcResponse = serde_json::from_value(raw)
            .map_err(|e| tool_err(format!("malformed response to `{method}`: {e}")))?;
        if let Some(error) = response.error {
            return Err(tool_err(format!(
                "`{method}` failed (code {}): {}",
                error.code, error.message
            )));
        }
        response.result.ok_or_else(|| {
            tool_err(format!(
                "`{method}` response carried neither result nor error"
            ))
        })
    }

    /// Send a notification (no response expected).
    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.ensure_open()?;
        let notification = JsonRpcNotification::new(method, params);
        let encoded = serde_json::to_value(&notification)
            .map_err(|e| tool_err(format!("failed to encode `{method}` notification: {e}")))?;
        let mut w = self.inner.writer.lock().await;
        write_framed(&mut **w, self.inner.framing, &encoded)
            .await
            .map_err(|e| tool_err(format!("failed to send `{method}` notification: {e}")))
    }

    /// Perform the MCP `initialize` handshake: negotiate the protocol
    /// revision, advertise `clientInfo`/capabilities, then send the
    /// `notifications/initialized` notification.
    pub async fn initialize(&self) -> Result<InitializeResult> {
        let params = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "agentgraph",
                "version": env!("CARGO_PKG_VERSION"),
            },
        });
        let result = self.request("initialize", params).await?;

        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let server_info = result.get("serverInfo");
        let server_name = server_info
            .and_then(|i| i.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let server_version = server_info
            .and_then(|i| i.get("version"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);

        self.notify("notifications/initialized", json!({})).await?;
        self.inner.initialized.store(true, Ordering::Relaxed);

        Ok(InitializeResult {
            protocol_version,
            server_name,
            server_version,
            capabilities,
        })
    }

    /// List the server's tools (`tools/list`).
    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        self.ensure_initialized()?;
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| tool_err("`tools/list` result missing `tools` array"))?;
        tools
            .iter()
            .map(|t| {
                let name = t
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| tool_err("`tools/list` entry missing `name`"))?;
                Ok(McpToolInfo {
                    name: name.to_owned(),
                    description: t
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                })
            })
            .collect()
    }

    /// Call a tool (`tools/call`).
    ///
    /// On success returns the concatenated `text` content items as a
    /// [`Value::String`] (the common case), or the raw result object if the
    /// server returned no text content. A result with `isError: true` maps to
    /// [`AgentGraphError::Tool`].
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        self.ensure_initialized()?;
        let result = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;

        let text = extract_text_content(&result);
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if is_error {
            let detail = text.unwrap_or_else(|| result.to_string());
            return Err(tool_err(format!("tool `{name}` reported error: {detail}")));
        }
        match text {
            Some(t) => Ok(Value::String(t)),
            None => Ok(result),
        }
    }

    /// List the server's tools and wrap each as an `agentgraph` [`Tool`],
    /// ready for [`crate::tool::ToolRegistry::register_shared`].
    pub async fn into_tools(&self) -> Result<Vec<Arc<dyn Tool>>> {
        let infos = self.list_tools().await?;
        Ok(infos
            .into_iter()
            .map(|info| Arc::new(McpToolAdapter::new(self.clone(), info)) as Arc<dyn Tool>)
            .collect())
    }

    /// Cleanly shut the client down: stop the reader task, fail pending
    /// requests, and kill the child process (for stdio clients). Idempotent.
    pub async fn shutdown(&self) -> Result<()> {
        self.inner.closed.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = self.inner.reader_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
        if let Ok(mut guard) = self.inner.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.start_kill();
            }
        }
        self.inner.pending.lock().await.clear();
        Ok(())
    }
}

/// Extract concatenated `text` content items from a `tools/call` result.
fn extract_text_content(result: &Value) -> Option<String> {
    let content = result.get("content")?.as_array()?;
    let texts: Vec<&str> = content
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect();
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Stdio transport
// ---------------------------------------------------------------------------

/// Factory for MCP clients backed by a child-process stdio transport.
pub struct McpStdioClient;

impl McpStdioClient {
    /// Spawn `command args...` as a child process with piped stdin/stdout
    /// (stderr is discarded) and return an [`McpClient`] connected to it
    /// using newline-delimited framing, per the MCP stdio transport.
    ///
    /// The child is killed on [`McpClient::shutdown`] and when the last
    /// client handle drops (`kill_on_drop`).
    pub fn spawn<S: AsRef<str>>(command: S, args: &[S]) -> Result<McpClient> {
        let mut cmd = Command::new(command.as_ref());
        cmd.args(args.iter().map(AsRef::as_ref))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| tool_err(format!("failed to spawn `{}`: {e}", command.as_ref())))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| tool_err("child stdout was not piped"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| tool_err("child stdin was not piped"))?;
        Ok(McpClient::connect_inner(
            stdout,
            stdin,
            Framing::NewlineDelimited,
            Some(child),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tool adapter
// ---------------------------------------------------------------------------

/// Wraps one MCP tool as an `agentgraph` [`Tool`].
///
/// `name` / `description` / `parameters_schema` come from the server's
/// `tools/list` metadata; [`Tool::call`] issues `tools/call` and extracts the
/// text content.
pub struct McpToolAdapter {
    client: McpClient,
    info: McpToolInfo,
}

impl McpToolAdapter {
    /// An adapter for `info` that dispatches through `client`.
    pub fn new(client: McpClient, info: McpToolInfo) -> Self {
        Self { client, info }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.info.name
    }

    fn description(&self) -> &str {
        &self.info.description
    }

    fn parameters_schema(&self) -> Value {
        self.info.input_schema.clone()
    }

    async fn call(&self, args: Value) -> Result<Value> {
        self.client.call_tool(&self.info.name, args).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolRegistry;
    use tokio::io::{duplex, DuplexStream};

    /// A scripted mock MCP server speaking the full handshake.
    async fn run_mock_server(stream: DuplexStream, framing: Framing) {
        let (read, mut write) = tokio::io::split(stream);
        let mut reader = BufReader::new(read);
        while let Ok(Some(msg)) = read_framed(&mut reader, framing).await {
            let id = msg.get("id").cloned();
            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
            let response = match method {
                "notifications/initialized" => None,
                "initialize" => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "mock-mcp", "version": "0.0.1"},
                    }
                })),
                "tools/list" => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [
                            {
                                "name": "echo",
                                "description": "Echoes text back.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {"text": {"type": "string"}},
                                    "required": ["text"]
                                }
                            },
                            {
                                "name": "fail_rpc",
                                "description": "Fails at the JSON-RPC layer.",
                                "inputSchema": {"type": "object"}
                            },
                            {
                                "name": "error_tool",
                                "description": "Reports a tool-level error.",
                                "inputSchema": {"type": "object"}
                            },
                            {
                                "name": "slow",
                                "description": "Responds slowly.",
                                "inputSchema": {"type": "object"}
                            }
                        ]
                    }
                })),
                "tools/call" => {
                    let name = msg
                        .pointer("/params/name")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    match name {
                        "echo" => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {"content": [{"type": "text", "text": "hello from echo"}]}
                        })),
                        "error_tool" => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": "invalid widget id"}],
                                "isError": true
                            }
                        })),
                        "fail_rpc" => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32602, "message": "invalid params: missing `widget_id`"}
                        })),
                        "slow" => {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            Some(json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {"content": [{"type": "text", "text": "too late"}]}
                            }))
                        }
                        _ => Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32601, "message": "unknown tool"}
                        })),
                    }
                }
                _ => id.map(|i| {
                    json!({
                        "jsonrpc": "2.0",
                        "id": i,
                        "error": {"code": -32601, "message": "method not found"}
                    })
                }),
            };
            if let Some(resp) = response {
                write_framed(&mut write, framing, &resp)
                    .await
                    .expect("mock server write");
            }
        }
    }

    /// A client connected to a scripted mock server over an in-memory
    /// full-duplex transport.
    fn client_and_mock(framing: Framing) -> (McpClient, JoinHandle<()>) {
        let (client_stream, server_stream) = duplex(64 * 1024);
        let handle = tokio::spawn(run_mock_server(server_stream, framing));
        let (read, write) = tokio::io::split(client_stream);
        (
            McpClient::connect_with_framing(read, write, framing),
            handle,
        )
    }

    async fn initialized_client(framing: Framing) -> (McpClient, JoinHandle<()>) {
        let (client, handle) = client_and_mock(framing);
        client.initialize().await.expect("initialize");
        (client, handle)
    }

    #[tokio::test]
    async fn initialize_handshake_returns_server_info() {
        let (client, _mock) = client_and_mock(Framing::NewlineDelimited);
        assert!(!client.is_initialized());
        let info = client.initialize().await.expect("initialize");
        assert_eq!(info.protocol_version, MCP_PROTOCOL_VERSION);
        assert_eq!(info.server_name, "mock-mcp");
        assert_eq!(info.server_version, "0.0.1");
        assert!(info.capabilities.get("tools").is_some());
        assert!(client.is_initialized());
        client.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn requests_before_initialize_are_rejected() {
        let (client, _mock) = client_and_mock(Framing::NewlineDelimited);
        let err = client.list_tools().await.unwrap_err();
        assert!(matches!(err, AgentGraphError::Tool(_)));
        assert!(err.to_string().contains("initialize"));
    }

    #[tokio::test]
    async fn tools_list_parses_metadata() {
        let (client, _mock) = initialized_client(Framing::NewlineDelimited).await;
        let tools = client.list_tools().await.expect("tools/list");
        assert_eq!(tools.len(), 4);
        let echo = tools.iter().find(|t| t.name == "echo").expect("echo tool");
        assert_eq!(echo.description, "Echoes text back.");
        assert_eq!(echo.input_schema["type"], json!("object"));
        assert!(echo.input_schema["properties"]["text"].is_object());
        assert_eq!(echo.input_schema["required"], json!(["text"]));
    }

    #[tokio::test]
    async fn tools_call_extracts_text_content() {
        let (client, _mock) = initialized_client(Framing::NewlineDelimited).await;
        let value = client
            .call_tool("echo", json!({"text": "hi"}))
            .await
            .expect("tools/call");
        assert_eq!(value, json!("hello from echo"));
    }

    #[tokio::test]
    async fn json_rpc_and_tool_errors_map_to_tool_variant() {
        let (client, _mock) = initialized_client(Framing::NewlineDelimited).await;

        // JSON-RPC-level error.
        let err = client.call_tool("fail_rpc", json!({})).await.unwrap_err();
        match err {
            AgentGraphError::Tool(msg) => {
                assert!(msg.contains("-32602"), "got: {msg}");
                assert!(msg.contains("invalid params"), "got: {msg}");
            }
            other => panic!("expected Tool error, got: {other}"),
        }

        // Tool-level error (`isError: true`).
        let err = client.call_tool("error_tool", json!({})).await.unwrap_err();
        assert!(matches!(err, AgentGraphError::Tool(_)));
        assert!(err.to_string().contains("invalid widget id"));
    }

    #[tokio::test]
    async fn request_timeout_aborts_pending_call() {
        let (client, _mock) = client_and_mock(Framing::NewlineDelimited);
        client.set_request_timeout(Duration::from_millis(50));
        client.initialize().await.expect("initialize");
        let err = client.call_tool("slow", json!({})).await.unwrap_err();
        assert!(matches!(err, AgentGraphError::Tool(_)));
        assert!(err.to_string().contains("timed out"), "got: {err}");
    }

    #[tokio::test]
    async fn into_tools_produces_registry_ready_adapters() {
        let (client, _mock) = initialized_client(Framing::NewlineDelimited).await;
        let tools = client.into_tools().await.expect("into_tools");
        assert_eq!(tools.len(), 4);

        let mut registry = ToolRegistry::new();
        for tool in tools {
            registry.register_shared(tool);
        }
        assert!(registry.contains("echo"));
        assert!(registry.contains("fail_rpc"));

        let echo = registry.get("echo").expect("echo registered");
        assert_eq!(echo.name(), "echo");
        assert_eq!(echo.description(), "Echoes text back.");
        assert!(echo.parameters_schema()["properties"]["text"].is_object());

        let out = echo
            .call(json!({"text": "yo"}))
            .await
            .expect("adapter call");
        assert_eq!(out, json!("hello from echo"));

        // Registry schemas remain OpenAI-shaped with the MCP tool inside.
        let schemas = registry.schemas();
        assert!(schemas
            .iter()
            .any(|s| s["function"]["name"] == json!("echo")));
    }

    #[tokio::test]
    async fn content_length_framing_roundtrip() {
        let (client, _mock) = initialized_client(Framing::ContentLength).await;
        assert_eq!(client.framing(), Framing::ContentLength);
        let value = client
            .call_tool("echo", json!({"text": "framed"}))
            .await
            .expect("tools/call over Content-Length framing");
        assert_eq!(value, json!("hello from echo"));
        client.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn stdio_spawn_of_missing_command_errors() {
        let args: [&str; 0] = [];
        let result = McpStdioClient::spawn("agentgraph-no-such-mcp-server-zzz", &args);
        match result {
            Err(err) => {
                assert!(matches!(err, AgentGraphError::Tool(_)));
                assert!(err.to_string().contains("failed to spawn"));
            }
            Ok(_) => panic!("spawning a nonexistent command should fail"),
        }
    }
}
