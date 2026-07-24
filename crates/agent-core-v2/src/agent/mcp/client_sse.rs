//! Legacy server-sent-events MCP client.
//!
//! Original: `agent/mcp/client-sse.ts`, `SSEClientTransport` input stream.

/// A complete server-sent event after line folding and blank-line dispatch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McpSseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpSseEndpointError {
    #[error("invalid MCP SSE server URL: {0}")]
    ServerUrl(#[from] url::ParseError),
    #[error("MCP SSE endpoint event has an empty message URL")]
    EmptyEndpoint,
}

/// Resolves the legacy `event: endpoint` data field using the source event
/// stream as the base URL, as done by the MCP SDK's SSE client transport.
pub fn resolve_sse_message_endpoint(
    server_url: &str,
    endpoint: &str,
) -> Result<url::Url, McpSseEndpointError> {
    if endpoint.is_empty() {
        return Err(McpSseEndpointError::EmptyEndpoint);
    }
    let server_url = url::Url::parse(server_url)?;
    Ok(server_url.join(endpoint)?)
}

/// Incremental SSE decoder used by the legacy MCP client. It accepts arbitrary
/// UTF-8 chunk boundaries and follows the SSE field/blank-line dispatch rules.
#[derive(Default)]
pub struct McpSseDecoder {
    pending: String,
    event: Option<String>,
    data: Vec<String>,
    id: Option<String>,
}

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
pub enum McpSseRequestError {
    #[error("MCP SSE response is not a JSON-RPC response with a numeric id")]
    InvalidResponse,
    #[error("MCP SSE request was closed before its response arrived")]
    Closed,
}

/// Correlates legacy SSE JSON-RPC response envelopes with the POST request
/// that originated them. The TypeScript SDK keeps this bookkeeping inside its
/// SSE transport; Rust makes it explicit so stream teardown can fail waiters.
#[derive(Default)]
pub struct McpSseResponseRouter {
    pending: std::collections::HashMap<u64, tokio::sync::oneshot::Sender<serde_json::Value>>,
}

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Map, Value};
use tokio::{
    sync::{Mutex, Notify},
    task::JoinHandle,
};

use crate::{
    _base::{
        utils::abort::{AbortSignal, abortable},
        version::get_core_version,
    },
    agent::mcp::{
        McpClient, McpClientError, McpServerSseConfig, McpToolDefinition, McpToolResult,
        UnexpectedCloseReason, build_mcp_remote_headers, to_mcp_tool_result,
    },
};

use super::client_stdio::UnexpectedCloseListener;

#[derive(Clone, Debug, Default)]
pub struct SseMcpClientOptions {
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub tool_call_timeout_ms: Option<u64>,
    pub oauth_access_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpSseClientError {
    #[error("MCP SSE client is closed")]
    Closed,
    #[error("MCP SSE client was closed during startup")]
    ClosedDuringStartup,
    #[error("MCP SSE client is not connected")]
    NotConnected,
    #[error("invalid MCP SSE header {name:?}: {value:?}")]
    InvalidHeader { name: String, value: String },
    #[error("MCP SSE {operation} failed: {message}")]
    Runtime {
        operation: &'static str,
        message: String,
    },
}

#[derive(Default)]
struct SseClientState {
    started: bool,
    closed: bool,
    ready: bool,
    endpoint: Option<url::Url>,
    stream_task: Option<JoinHandle<()>>,
    unexpected_close_listener: Option<UnexpectedCloseListener>,
    pending_unexpected_close: Option<UnexpectedCloseReason>,
}

/// Client for the MCP legacy SSE transport (`GET` event stream plus POST endpoint).
pub struct SseMcpClient {
    url: String,
    headers: reqwest::header::HeaderMap,
    http: reqwest::Client,
    client_name: String,
    client_version: String,
    tool_call_timeout: Option<Duration>,
    next_request_id: AtomicU64,
    router: Arc<Mutex<McpSseResponseRouter>>,
    state: Arc<Mutex<SseClientState>>,
    endpoint_changed: Arc<Notify>,
}

impl SseMcpClient {
    // Original: SseMcpClient.constructor().
    pub fn new(
        config: McpServerSseConfig,
        options: SseMcpClientOptions,
        env_lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, McpSseClientError> {
        let headers = build_mcp_remote_headers(&config, "sse", env_lookup)
            .map_err(|error| McpSseClientError::Runtime {
                operation: "header setup",
                message: error.to_string(),
            })?
            .unwrap_or_default();
        let mut header_map = reqwest::header::HeaderMap::new();
        for (name, value) in headers {
            let name = reqwest::header::HeaderName::try_from(name.as_str()).map_err(|_| {
                McpSseClientError::InvalidHeader {
                    name: name.clone(),
                    value: value.clone(),
                }
            })?;
            let value = reqwest::header::HeaderValue::try_from(value.as_str()).map_err(|_| {
                McpSseClientError::InvalidHeader {
                    name: name.to_string(),
                    value,
                }
            })?;
            header_map.insert(name, value);
        }
        if let Some(token) = options.oauth_access_token.as_deref() {
            let value = reqwest::header::HeaderValue::try_from(format!("Bearer {token}")).map_err(
                |_| McpSseClientError::InvalidHeader {
                    name: "Authorization".into(),
                    value: format!("Bearer {token}"),
                },
            )?;
            header_map.insert(reqwest::header::AUTHORIZATION, value);
        }
        Ok(Self {
            url: config.url,
            headers: header_map,
            http: reqwest::Client::new(),
            client_name: options.client_name.unwrap_or_else(|| "kimi-code".into()),
            client_version: options
                .client_version
                .unwrap_or_else(|| get_core_version().into()),
            tool_call_timeout: options.tool_call_timeout_ms.map(Duration::from_millis),
            next_request_id: AtomicU64::new(1),
            router: Arc::new(Mutex::new(McpSseResponseRouter::default())),
            state: Arc::new(Mutex::new(SseClientState::default())),
            endpoint_changed: Arc::new(Notify::new()),
        })
    }

    // Original: SseMcpClient.connect().
    pub async fn connect(&self) -> Result<(), McpSseClientError> {
        {
            let mut state = self.state.lock().await;
            if state.closed {
                return Err(McpSseClientError::Closed);
            }
            if state.started {
                return Ok(());
            }
            state.started = true;
        }
        self.start_stream().await?;
        self.wait_for_endpoint().await?;
        let initialize = self
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": self.client_name, "version": self.client_version},
                }),
            )
            .await?;
        if initialize.get("result").is_none() {
            return Err(McpSseClientError::Runtime {
                operation: "initialization",
                message: json_rpc_error_message(&initialize),
            });
        }
        self.send_notification("notifications/initialized", serde_json::json!({}))
            .await?;
        let mut state = self.state.lock().await;
        if state.closed {
            return Err(McpSseClientError::ClosedDuringStartup);
        }
        state.ready = true;
        Ok(())
    }

    async fn start_stream(&self) -> Result<(), McpSseClientError> {
        let response = self
            .http
            .get(&self.url)
            .headers(self.headers.clone())
            .send()
            .await
            .map_err(|error| McpSseClientError::Runtime {
                operation: "event stream startup",
                message: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpSseClientError::Runtime {
                operation: "event stream startup",
                message: error.to_string(),
            })?;
        let state = Arc::clone(&self.state);
        let router = Arc::clone(&self.router);
        let endpoint_changed = Arc::clone(&self.endpoint_changed);
        let url = self.url.clone();
        let stream_task = tokio::spawn(async move {
            let mut decoder = McpSseDecoder::default();
            let mut stream = response.bytes_stream();
            let mut terminal_error = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => {
                        let text = String::from_utf8_lossy(&chunk);
                        for event in decoder.push(&text) {
                            dispatch_sse_event(&state, &router, &endpoint_changed, &url, event)
                                .await;
                        }
                    }
                    Err(error) => {
                        terminal_error = Some(error.to_string());
                        break;
                    }
                }
            }
            if let Some(event) = decoder.finish() {
                dispatch_sse_event(&state, &router, &endpoint_changed, &url, event).await;
            }
            router.lock().await.close();
            let listener = {
                let mut state = state.lock().await;
                if state.closed || !state.ready {
                    return;
                }
                let reason = UnexpectedCloseReason {
                    error: terminal_error,
                    stderr: None,
                };
                if let Some(listener) = &state.unexpected_close_listener {
                    Some((Arc::clone(listener), reason))
                } else {
                    state.pending_unexpected_close = Some(reason);
                    None
                }
            };
            if let Some((listener, reason)) = listener {
                listener(reason);
            }
        });
        self.state.lock().await.stream_task = Some(stream_task);
        Ok(())
    }

    async fn wait_for_endpoint(&self) -> Result<url::Url, McpSseClientError> {
        loop {
            let notified = self.endpoint_changed.notified();
            {
                let state = self.state.lock().await;
                if let Some(endpoint) = &state.endpoint {
                    return Ok(endpoint.clone());
                }
                if state.closed {
                    return Err(McpSseClientError::Closed);
                }
            }
            notified.await;
        }
    }

    async fn send_notification(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(), McpSseClientError> {
        let endpoint = self.endpoint().await?;
        self.http
            .post(endpoint)
            .headers(self.headers.clone())
            .json(&serde_json::json!({"jsonrpc":"2.0", "method":method, "params":params}))
            .send()
            .await
            .map_err(|error| McpSseClientError::Runtime {
                operation: "notification",
                message: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpSseClientError::Runtime {
                operation: "notification",
                message: error.to_string(),
            })?;
        Ok(())
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value, McpSseClientError> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let receiver = self.router.lock().await.register(id);
        let endpoint = self.endpoint().await?;
        let response = self
            .http
            .post(endpoint)
            .headers(self.headers.clone())
            .json(&serde_json::json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .send()
            .await
            .map_err(|error| McpSseClientError::Runtime {
                operation: "request",
                message: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpSseClientError::Runtime {
                operation: "request",
                message: error.to_string(),
            })?;
        drop(response);
        let await_response = async {
            receiver.await.map_err(|_| McpSseClientError::Runtime {
                operation: "request",
                message: "event stream closed before response".into(),
            })
        };
        if let Some(timeout) = self.tool_call_timeout {
            tokio::time::timeout(timeout, await_response)
                .await
                .map_err(|_| McpSseClientError::Runtime {
                    operation: "request",
                    message: format!("timed out after {}ms", timeout.as_millis()),
                })?
        } else {
            await_response.await
        }
    }

    async fn endpoint(&self) -> Result<url::Url, McpSseClientError> {
        self.state
            .lock()
            .await
            .endpoint
            .clone()
            .ok_or(McpSseClientError::NotConnected)
    }

    // Original: SseMcpClient.close().
    pub async fn close(&self) -> Result<(), McpSseClientError> {
        let task = {
            let mut state = self.state.lock().await;
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.ready = false;
            state.started = false;
            state.endpoint = None;
            state.stream_task.take()
        };
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        self.router.lock().await.close();
        self.endpoint_changed.notify_waiters();
        Ok(())
    }

    pub async fn on_unexpected_close(&self, listener: UnexpectedCloseListener) {
        let pending = {
            let mut state = self.state.lock().await;
            state.unexpected_close_listener = Some(Arc::clone(&listener));
            state.pending_unexpected_close.take()
        };
        if let Some(reason) = pending {
            listener(reason);
        }
    }
}

async fn dispatch_sse_event(
    state: &Arc<Mutex<SseClientState>>,
    router: &Arc<Mutex<McpSseResponseRouter>>,
    endpoint_changed: &Arc<Notify>,
    server_url: &str,
    event: McpSseEvent,
) {
    if event.event.as_deref() == Some("endpoint") {
        if let Ok(endpoint) = resolve_sse_message_endpoint(server_url, &event.data) {
            state.lock().await.endpoint = Some(endpoint);
            endpoint_changed.notify_waiters();
        }
        return;
    }
    if let Ok(message) = serde_json::from_str::<Value>(&event.data) {
        let _ = router.lock().await.deliver(message);
    }
}

fn json_rpc_error_message(message: &Value) -> String {
    message
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("received an unexpected JSON-RPC response")
        .into()
}

#[async_trait]
impl McpClient for SseMcpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpClientError> {
        let response = self
            .send_request("tools/list", serde_json::json!({}))
            .await
            .map_err(box_sse_error)?;
        let tools = response
            .get("result")
            .and_then(|result| result.get("tools"))
            .cloned()
            .ok_or_else(|| {
                box_sse_error(McpSseClientError::Runtime {
                    operation: "tool listing",
                    message: json_rpc_error_message(&response),
                })
            })?;
        serde_json::from_value(tools).map_err(|error| {
            box_sse_error(McpSseClientError::Runtime {
                operation: "tool definition conversion",
                message: error.to_string(),
            })
        })
    }

    async fn call_tool(
        &self,
        name: &str,
        args: Map<String, Value>,
        signal: Option<AbortSignal>,
    ) -> Result<McpToolResult, McpClientError> {
        let call = self.send_request(
            "tools/call",
            serde_json::json!({"name": name, "arguments": args}),
        );
        let response = if let Some(signal) = signal {
            abortable(call, &signal)
                .await
                .map_err(|error| -> McpClientError { Box::new((*error).clone()) })?
                .map_err(box_sse_error)?
        } else {
            call.await.map_err(box_sse_error)?
        };
        let result = response.get("result").cloned().ok_or_else(|| {
            box_sse_error(McpSseClientError::Runtime {
                operation: "tool request",
                message: json_rpc_error_message(&response),
            })
        })?;
        Ok(to_mcp_tool_result(&result))
    }
}

fn box_sse_error(error: McpSseClientError) -> McpClientError {
    Box::new(error)
}

impl McpSseResponseRouter {
    pub fn register(&mut self, id: u64) -> tokio::sync::oneshot::Receiver<serde_json::Value> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        if let Some(previous) = self.pending.insert(id, sender) {
            drop(previous);
        }
        receiver
    }

    pub fn deliver(&mut self, message: serde_json::Value) -> Result<bool, McpSseRequestError> {
        let id = message
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .ok_or(McpSseRequestError::InvalidResponse)?;
        Ok(self
            .pending
            .remove(&id)
            .is_some_and(|sender| sender.send(message).is_ok()))
    }

    pub fn close(&mut self) {
        self.pending.clear();
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl McpSseDecoder {
    // Original adaptation: SSEClientTransport's event-source parser.
    pub fn push(&mut self, chunk: &str) -> Vec<McpSseEvent> {
        self.pending.push_str(chunk);
        let mut events = Vec::new();
        while let Some(newline) = self.pending.find('\n') {
            let mut line = self.pending[..newline].to_owned();
            self.pending.drain(..=newline);
            if line.ends_with('\r') {
                line.pop();
            }
            if line.is_empty() {
                if let Some(event) = self.dispatch() {
                    events.push(event);
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line.as_str(), ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.event = Some(value.into()),
                "data" => self.data.push(value.into()),
                "id" if !value.contains('\0') => self.id = Some(value.into()),
                _ => {}
            }
        }
        events
    }

    pub fn finish(&mut self) -> Option<McpSseEvent> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.apply_line(&line);
        }
        self.dispatch()
    }

    fn apply_line(&mut self, line: &str) {
        if line.is_empty() || line.starts_with(':') {
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.into()),
            "data" => self.data.push(value.into()),
            "id" if !value.contains('\0') => self.id = Some(value.into()),
            _ => {}
        }
    }

    fn dispatch(&mut self) -> Option<McpSseEvent> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        Some(McpSseEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
            id: self.id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn decodes_folded_events_across_arbitrary_chunks() {
        let mut decoder = McpSseDecoder::default();
        assert!(decoder.push("event: endpoint\ndata: /m").is_empty());
        assert_eq!(
            decoder.push("essages\nid: 7\n\ndata: {\\\"jsonrpc\\\":\\\"2.0\\\"}\n\n"),
            vec![
                McpSseEvent {
                    event: Some("endpoint".into()),
                    data: "/messages".into(),
                    id: Some("7".into()),
                },
                McpSseEvent {
                    event: None,
                    data: r#"{\"jsonrpc\":\"2.0\"}"#.into(),
                    id: Some("7".into()),
                },
            ]
        );
    }

    #[test]
    fn ignores_comments_and_null_ids_and_finishes_unterminated_data() {
        let mut decoder = McpSseDecoder::default();
        assert!(
            decoder
                .push(": ping\nid: good\nid: bad\0id\ndata: final")
                .is_empty()
        );
        assert_eq!(
            decoder.finish(),
            Some(McpSseEvent {
                event: None,
                data: "final".into(),
                id: Some("good".into()),
            })
        );
    }

    #[test]
    fn resolves_endpoint_events_against_the_sse_url() {
        assert_eq!(
            resolve_sse_message_endpoint("https://mcp.example/events", "/messages").unwrap(),
            url::Url::parse("https://mcp.example/messages").unwrap()
        );
        assert_eq!(
            resolve_sse_message_endpoint("https://mcp.example/base/events", "messages").unwrap(),
            url::Url::parse("https://mcp.example/base/messages").unwrap()
        );
        assert!(matches!(
            resolve_sse_message_endpoint("https://mcp.example/events", ""),
            Err(McpSseEndpointError::EmptyEndpoint)
        ));
    }

    #[tokio::test]
    async fn routes_numeric_json_rpc_responses_and_closes_pending_requests() {
        let mut router = McpSseResponseRouter::default();
        let first = router.register(1);
        let second = router.register(2);
        assert!(
            router
                .deliver(serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}))
                .unwrap()
        );
        assert_eq!(first.await.unwrap()["result"]["ok"], true);
        assert_eq!(router.pending_count(), 1);
        assert!(
            !router
                .deliver(serde_json::json!({"jsonrpc": "2.0", "id": 3, "result": null}))
                .unwrap()
        );
        assert!(matches!(
            router.deliver(serde_json::json!({"id": "two"})),
            Err(McpSseRequestError::InvalidResponse)
        ));
        router.close();
        assert!(second.await.is_err());
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert_ne!(read, 0, "client closed before a complete HTTP request");
            bytes.extend_from_slice(&buffer[..read]);
            let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..headers_end + 4]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .or_else(|| line.strip_prefix("Content-Length:"))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= headers_end + 4 + content_length {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn legacy_sse_client_initializes_lists_and_calls_tools() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut events, _) = listener.accept().await.unwrap();
            assert!(
                read_request(&mut events)
                    .await
                    .starts_with("GET /sse HTTP/1.1")
            );
            events.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\nevent: endpoint\ndata: /messages\n\n").await.unwrap();
            for _ in 0..4 {
                let (mut request, _) = listener.accept().await.unwrap();
                let raw = read_request(&mut request).await;
                let body = raw.split_once("\r\n\r\n").unwrap().1;
                let message: Value = serde_json::from_str(body).unwrap();
                request
                    .write_all(
                        b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                let Some(id) = message.get("id").cloned() else {
                    continue;
                };
                let result = match message["method"].as_str().unwrap() {
                    "initialize" => {
                        serde_json::json!({"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}})
                    }
                    "tools/list" => {
                        serde_json::json!({"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]})
                    }
                    "tools/call" => {
                        serde_json::json!({"content":[{"type":"text","text":"hello"}],"isError":false})
                    }
                    method => panic!("unexpected method {method}"),
                };
                let event = format!(
                    "data: {}\n\n",
                    serde_json::json!({"jsonrpc":"2.0","id":id,"result":result})
                );
                events.write_all(event.as_bytes()).await.unwrap();
            }
        });
        let config = McpServerSseConfig {
            url: format!("http://{address}/sse"),
            headers: None,
            bearer_token_env_var: None,
            common: crate::agent::mcp::McpServerCommonFields::default(),
        };
        let client = SseMcpClient::new(config, SseMcpClientOptions::default(), |_| None).unwrap();
        client.connect().await.unwrap();
        assert_eq!(client.list_tools().await.unwrap()[0].name, "echo");
        assert_eq!(
            client
                .call_tool("echo", Map::new(), None)
                .await
                .unwrap()
                .content[0]
                .text
                .as_deref(),
            Some("hello")
        );
        client.close().await.unwrap();
        server.await.unwrap();
    }
}
