//! Streamable-HTTP MCP client.
//!
//! Original: `agent/mcp/client-http.ts`, `HttpMcpClient`.

use std::{collections::HashMap, error::Error, sync::Arc, time::Duration};

use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use rmcp::{
    Peer, RoleClient,
    model::{CallToolRequest, CallToolRequestParams, ClientInfo, ClientRequest, ServerResult},
    serve_client,
    service::{PeerRequestOptions, RunningService},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Map, Value};
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
    _base::{
        utils::abort::{AbortSignal, abortable},
        version::get_core_version,
    },
    agent::mcp::{
        McpClient, McpClientError, McpServerHttpConfig, McpToolDefinition, McpToolResult,
        UnexpectedCloseReason, build_mcp_remote_headers, to_mcp_tool_result,
    },
};

use super::client_stdio::UnexpectedCloseListener;

type RunningHttpClient = RunningService<RoleClient, ClientInfo>;

#[derive(Clone, Debug, Default)]
pub struct HttpMcpClientOptions {
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub tool_call_timeout_ms: Option<u64>,
    pub oauth_access_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpHttpClientError {
    #[error("MCP HTTP client is closed")]
    Closed,
    #[error("MCP HTTP client was closed during startup")]
    ClosedDuringStartup,
    #[error("MCP HTTP client is not connected")]
    NotConnected,
    #[error("invalid MCP HTTP header {name:?}: {value:?}")]
    InvalidHeader { name: String, value: String },
    #[error("MCP HTTP {operation} failed: {message}")]
    Runtime {
        operation: &'static str,
        message: String,
        #[source]
        source: Option<Box<dyn Error + Send + Sync>>,
    },
}

#[derive(Default)]
struct HttpState {
    started: bool,
    closed: bool,
    ready: bool,
    peer: Option<Peer<RoleClient>>,
    unexpected_close_listener: Option<UnexpectedCloseListener>,
    pending_unexpected_close: Option<UnexpectedCloseReason>,
    close_monitor: Option<JoinHandle<()>>,
}

/// Remote HTTP client backed by RMCP's streamable HTTP transport.
pub struct HttpMcpClient {
    url: String,
    headers: HashMap<HeaderName, HeaderValue>,
    client_name: String,
    client_version: String,
    tool_call_timeout: Option<Duration>,
    state: Arc<Mutex<HttpState>>,
}

impl HttpMcpClient {
    // Original: HttpMcpClient.constructor(). OAuth is deliberately supplied by
    // the future OAuth transport bridge; static headers and bearer env values
    // retain the source precedence here.
    pub fn new(
        config: McpServerHttpConfig,
        options: HttpMcpClientOptions,
        env_lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, McpHttpClientError> {
        let headers = build_mcp_remote_headers(&config, "http", env_lookup)
            .map_err(|error| McpHttpClientError::Runtime {
                operation: "header setup",
                message: error.to_string(),
                source: Some(Box::new(error)),
            })?
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| {
                let parsed_name = HeaderName::try_from(name.as_str()).map_err(|_| {
                    McpHttpClientError::InvalidHeader {
                        name: name.clone(),
                        value: value.clone(),
                    }
                })?;
                let parsed_value = HeaderValue::try_from(value.as_str())
                    .map_err(|_| McpHttpClientError::InvalidHeader { name, value })?;
                Ok((parsed_name, parsed_value))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let mut headers = headers;
        if let Some(token) = options.oauth_access_token.as_deref() {
            headers.insert(
                HeaderName::from_static("authorization"),
                HeaderValue::try_from(format!("Bearer {token}")).map_err(|_| {
                    McpHttpClientError::InvalidHeader {
                        name: "Authorization".into(),
                        value: format!("Bearer {token}"),
                    }
                })?,
            );
        }
        Ok(Self {
            url: config.url,
            headers,
            client_name: options.client_name.unwrap_or_else(|| "kimi-code".into()),
            client_version: options
                .client_version
                .unwrap_or_else(|| get_core_version().into()),
            tool_call_timeout: options.tool_call_timeout_ms.map(Duration::from_millis),
            state: Arc::new(Mutex::new(HttpState::default())),
        })
    }

    // Original: HttpMcpClient.connect().
    pub async fn connect(&self) -> Result<(), McpHttpClientError> {
        {
            let mut state = self.state.lock().await;
            if state.closed {
                return Err(McpHttpClientError::Closed);
            }
            if state.started {
                return Ok(());
            }
            state.started = true;
        }
        let result = self.start_client().await;
        let mut state = self.state.lock().await;
        if let Err(error) = result {
            state.started = false;
            return Err(error);
        }
        if state.closed {
            if let Some(monitor) = state.close_monitor.take() {
                monitor.abort();
                let _ = monitor.await;
            }
            return Err(McpHttpClientError::ClosedDuringStartup);
        }
        state.ready = true;
        Ok(())
    }

    async fn start_client(&self) -> Result<(), McpHttpClientError> {
        let config = StreamableHttpClientTransportConfig::with_uri(self.url.clone())
            .custom_headers(self.headers.clone());
        let transport = StreamableHttpClientTransport::from_config(config);
        let mut info = ClientInfo::default();
        info.client_info =
            rmcp::model::Implementation::new(&self.client_name, &self.client_version);
        let running =
            serve_client(info, transport)
                .await
                .map_err(|error| McpHttpClientError::Runtime {
                    operation: "connection startup",
                    message: error.to_string(),
                    source: Some(Box::new(error)),
                })?;
        let peer = running.peer().clone();
        let monitor = self.spawn_close_monitor(running);
        let mut state = self.state.lock().await;
        state.peer = Some(peer);
        state.close_monitor = Some(monitor);
        Ok(())
    }

    // Original: HttpMcpClient.close(). The close monitor owns the running
    // service; aborting it drops the service, and rmcp's own drop guard
    // cancels the serve loop, which drains in-flight responses and closes the
    // transport asynchronously.
    pub async fn close(&self) -> Result<(), McpHttpClientError> {
        let monitor = {
            let mut state = self.state.lock().await;
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.ready = false;
            state.started = false;
            state.peer = None;
            state.close_monitor.take()
        };
        if let Some(monitor) = monitor {
            monitor.abort();
            let _ = monitor.await;
        }
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

    async fn peer(&self) -> Result<Peer<RoleClient>, McpHttpClientError> {
        self.state
            .lock()
            .await
            .peer
            .clone()
            .ok_or(McpHttpClientError::NotConnected)
    }

    /// Waits for the service loop to terminate instead of polling
    /// `peer.is_transport_closed()`: the loop ends exactly when the transport
    /// closes (or the service is cancelled or errors), so this is a true
    /// event-driven wait with no timers.
    fn spawn_close_monitor(&self, running: RunningHttpClient) -> JoinHandle<()> {
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let _ = running.waiting().await;
            let listener = {
                let mut state = state.lock().await;
                if state.closed || !state.ready {
                    return;
                }
                let reason = UnexpectedCloseReason {
                    error: None,
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
        })
    }

    async fn call_tool_with_options(
        &self,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<McpToolResult, McpHttpClientError> {
        let peer = self.peer().await?;
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(
            CallToolRequestParams::new(name.to_owned()).with_arguments(args),
        ));
        let options = self
            .tool_call_timeout
            .map(PeerRequestOptions::with_timeout)
            .unwrap_or_else(PeerRequestOptions::no_options);
        let result = peer
            .send_request_with_option(request, options)
            .await
            .map_err(|error| McpHttpClientError::Runtime {
                operation: "tool request",
                message: error.to_string(),
                source: Some(Box::new(error)),
            })?
            .await_response()
            .await
            .map_err(|error| McpHttpClientError::Runtime {
                operation: "tool request",
                message: error.to_string(),
                source: Some(Box::new(error)),
            })?;
        let ServerResult::CallToolResult(result) = result else {
            return Err(McpHttpClientError::Runtime {
                operation: "tool request",
                message: "received an unexpected MCP response".into(),
                source: None,
            });
        };
        Ok(to_mcp_tool_result(&serde_json::to_value(result).map_err(
            |error| McpHttpClientError::Runtime {
                operation: "tool result conversion",
                message: error.to_string(),
                source: Some(Box::new(error)),
            },
        )?))
    }
}

#[async_trait]
impl McpClient for HttpMcpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpClientError> {
        let peer = self.peer().await.map_err(box_http_error)?;
        let result = peer.list_tools(None).await.map_err(|error| {
            box_http_error(McpHttpClientError::Runtime {
                operation: "tool listing",
                message: error.to_string(),
                source: Some(Box::new(error)),
            })
        })?;
        result
            .tools
            .into_iter()
            .map(|tool| {
                let mut value = serde_json::to_value(tool).map_err(|error| {
                    box_http_error(McpHttpClientError::Runtime {
                        operation: "tool definition conversion",
                        message: error.to_string(),
                        source: Some(Box::new(error)),
                    })
                })?;
                if value.get("description").is_none() {
                    value["description"] = Value::String(String::new());
                }
                serde_json::from_value(value).map_err(|error| {
                    box_http_error(McpHttpClientError::Runtime {
                        operation: "tool definition conversion",
                        message: error.to_string(),
                        source: Some(Box::new(error)),
                    })
                })
            })
            .collect()
    }

    async fn call_tool(
        &self,
        name: &str,
        args: Map<String, Value>,
        signal: Option<AbortSignal>,
    ) -> Result<McpToolResult, McpClientError> {
        let call = self.call_tool_with_options(name, args);
        let result = if let Some(signal) = signal {
            abortable(call, &signal)
                .await
                .map_err(|error| -> McpClientError { Box::new((*error).clone()) })?
        } else {
            call.await
        };
        result.map_err(box_http_error)
    }
}

fn box_http_error(error: McpHttpClientError) -> McpClientError {
    Box::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::mcp::McpServerCommonFields;

    fn config() -> McpServerHttpConfig {
        McpServerHttpConfig {
            url: "https://example.com/mcp".into(),
            headers: Some(HashMap::from([
                ("Authorization".into(), "old".into()),
                ("X-Trace".into(), "yes".into()),
            ])),
            bearer_token_env_var: Some("MCP_TOKEN".into()),
            common: McpServerCommonFields::default(),
        }
    }

    #[test]
    fn constructor_uses_remote_header_rules_and_rejects_invalid_headers() {
        let client = HttpMcpClient::new(config(), HttpMcpClientOptions::default(), |name| {
            (name == "MCP_TOKEN").then(|| "secret".into())
        })
        .unwrap();
        assert_eq!(
            client
                .headers
                .get(&HeaderName::from_static("authorization")),
            Some(&HeaderValue::from_static("Bearer secret"))
        );
        assert_eq!(
            client.headers.get(&HeaderName::from_static("x-trace")),
            Some(&HeaderValue::from_static("yes"))
        );

        let mut invalid = config();
        invalid.headers = Some(HashMap::from([("bad header".into(), "value".into())]));
        invalid.bearer_token_env_var = None;
        assert!(matches!(
            HttpMcpClient::new(invalid, HttpMcpClientOptions::default(), |_| None),
            Err(McpHttpClientError::InvalidHeader { .. })
        ));
    }

    #[tokio::test]
    async fn close_is_idempotent_and_prevents_startup() {
        let client = HttpMcpClient::new(config(), HttpMcpClientOptions::default(), |_| {
            Some("token".into())
        })
        .unwrap();
        client.close().await.unwrap();
        client.close().await.unwrap();
        assert!(matches!(
            client.connect().await,
            Err(McpHttpClientError::Closed)
        ));
    }
}
