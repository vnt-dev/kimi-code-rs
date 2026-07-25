//! Stdio MCP client process setup helpers.
//!
//! Original: `agent/mcp/client-stdio.ts`:
//! `BoundedTail`, `resolveStdioCwd()`, and `mergeStdioEnv()`.

use std::{
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use rmcp::{
    Peer, RoleClient,
    model::{CallToolRequest, CallToolRequestParams, ClientInfo, ClientRequest, ServerResult},
    serve_client,
    service::{PeerRequestOptions, RunningService},
    transport::TokioChildProcess,
};
use serde_json::{Map, Value};
use tokio::{
    io::{AsyncReadExt, BufReader},
    sync::Mutex,
    task::JoinHandle,
};

use crate::{
    _base::{
        utils::{
            abort::{AbortSignal, abortable},
            proxy::{Env, proxy_env_for_child, reconcile_child_no_proxy},
        },
        version::get_core_version,
    },
    agent::mcp::{
        McpClient, McpClientError, McpServerStdioConfig, McpToolDefinition, McpToolResult,
        UnexpectedCloseReason, to_mcp_tool_result,
    },
};

pub const STDERR_BUFFER_CAPACITY: usize = 4 * 1024;

pub type UnexpectedCloseListener = Arc<dyn Fn(UnexpectedCloseReason) + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub struct StdioMcpClientOptions {
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub tool_call_timeout_ms: Option<u64>,
    pub default_cwd: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpStdioClientError {
    #[error("MCP stdio executor '{executor}' is not yet implemented")]
    UnsupportedExecutor { executor: String },

    #[error("MCP stdio client is closed")]
    Closed,

    #[error("MCP stdio client was closed during startup")]
    ClosedDuringStartup,

    #[error("MCP stdio client is not connected")]
    NotConnected,

    #[error("MCP stdio {operation} failed: {message}")]
    Runtime {
        operation: &'static str,
        message: String,
    },
}

type RunningStdioClient = RunningService<RoleClient, ClientInfo>;

#[derive(Default)]
struct StdioMcpClientState {
    started: bool,
    closed: bool,
    ready: bool,
    running: Option<RunningStdioClient>,
    unexpected_close_listener: Option<UnexpectedCloseListener>,
    pending_unexpected_close: Option<UnexpectedCloseReason>,
    stderr_task: Option<JoinHandle<()>>,
    close_monitor: Option<JoinHandle<()>>,
}

/// MCP client backed by a local child process and its standard streams.
///
/// Original: `agent/mcp/client-stdio.ts`, `StdioMcpClient`.
pub struct StdioMcpClient {
    command: String,
    args: Vec<String>,
    env: Env,
    cwd: Option<PathBuf>,
    client_name: String,
    client_version: String,
    tool_call_timeout: Option<Duration>,
    stderr: Arc<Mutex<BoundedTail>>,
    state: Arc<Mutex<StdioMcpClientState>>,
}

/// A bounded stderr tail used when a child MCP server closes unexpectedly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedTail {
    capacity: usize,
    buffer: String,
}

impl BoundedTail {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: String::new(),
        }
    }

    // Original: BoundedTail.push().
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        let character_count = self.buffer.chars().count();
        if character_count > self.capacity {
            self.buffer = self
                .buffer
                .chars()
                .skip(character_count - self.capacity)
                .collect();
        }
    }

    // Original: BoundedTail.snapshot().
    pub fn snapshot(&self) -> String {
        self.buffer.clone()
    }
}

impl StdioMcpClient {
    pub fn new(
        config: McpServerStdioConfig,
        options: StdioMcpClientOptions,
    ) -> Result<Self, McpStdioClientError> {
        if let Some(executor) = config.executor
            && executor != crate::agent::mcp::McpExecutor::Local
        {
            return Err(McpStdioClientError::UnsupportedExecutor {
                executor: format!("{executor:?}").to_ascii_lowercase(),
            });
        }
        let cwd = resolve_stdio_cwd(config.cwd.as_deref(), options.default_cwd.as_deref());
        Ok(Self {
            command: config.command,
            args: config.args.unwrap_or_default(),
            env: merge_stdio_env(config.env.as_ref()),
            cwd,
            client_name: options.client_name.unwrap_or_else(|| "kimi-code".into()),
            client_version: options
                .client_version
                .unwrap_or_else(|| get_core_version().into()),
            tool_call_timeout: options.tool_call_timeout_ms.map(Duration::from_millis),
            stderr: Arc::new(Mutex::new(BoundedTail::new(STDERR_BUFFER_CAPACITY))),
            state: Arc::new(Mutex::new(StdioMcpClientState::default())),
        })
    }

    // Original: StdioMcpClient.connect().
    pub async fn connect(&self) -> Result<(), McpStdioClientError> {
        {
            let mut state = self.state.lock().await;
            if state.closed {
                return Err(McpStdioClientError::Closed);
            }
            if state.started {
                return Ok(());
            }
            state.started = true;
        }

        let connect_result = self.start_client().await;
        let mut state = self.state.lock().await;
        if let Err(error) = connect_result {
            state.started = false;
            return Err(error);
        }
        if state.closed {
            let mut running = state.running.take();
            drop(state);
            if let Some(running) = running.as_mut() {
                let _ = running.close().await;
            }
            return Err(McpStdioClientError::ClosedDuringStartup);
        }
        state.ready = true;
        Ok(())
    }

    async fn start_client(&self) -> Result<(), McpStdioClientError> {
        let mut command = tokio::process::Command::new(&self.command);
        command.args(&self.args).env_clear().envs(&self.env);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        let (transport, stderr) = TokioChildProcess::builder(command)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| McpStdioClientError::Runtime {
                operation: "process startup",
                message: error.to_string(),
            })?;
        let stderr_task = stderr.map(|stderr| self.spawn_stderr_reader(stderr));
        let mut client_info = ClientInfo::default();
        client_info.client_info =
            rmcp::model::Implementation::new(&self.client_name, &self.client_version);
        let running = match serve_client(client_info, transport).await {
            Ok(running) => running,
            Err(error) => {
                if let Some(stderr_task) = stderr_task {
                    let _ = stderr_task.await;
                }
                return Err(McpStdioClientError::Runtime {
                    operation: "connection startup",
                    message: error.to_string(),
                });
            }
        };
        let close_monitor = self.spawn_close_monitor(running.peer().clone());
        let mut state = self.state.lock().await;
        state.running = Some(running);
        state.stderr_task = stderr_task;
        state.close_monitor = Some(close_monitor);
        Ok(())
    }

    // Original: StdioMcpClient.close().
    pub async fn close(&self) -> Result<(), McpStdioClientError> {
        let (mut running, stderr_task, close_monitor) = {
            let mut state = self.state.lock().await;
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.ready = false;
            state.started = false;
            (
                state.running.take(),
                state.stderr_task.take(),
                state.close_monitor.take(),
            )
        };
        if let Some(close_monitor) = close_monitor {
            close_monitor.abort();
            let _ = close_monitor.await;
        }
        if let Some(running) = running.as_mut() {
            running
                .close()
                .await
                .map_err(|error| McpStdioClientError::Runtime {
                    operation: "shutdown",
                    message: error.to_string(),
                })?;
        }
        if let Some(stderr_task) = stderr_task {
            let _ = stderr_task.await;
        }
        Ok(())
    }

    // Original: StdioMcpClient.onUnexpectedClose().
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

    // Original: StdioMcpClient.stderrSnapshot().
    pub async fn stderr_snapshot(&self) -> String {
        self.stderr.lock().await.snapshot()
    }

    async fn peer(&self) -> Result<Peer<RoleClient>, McpStdioClientError> {
        self.state
            .lock()
            .await
            .running
            .as_ref()
            .map(|running| running.peer().clone())
            .ok_or(McpStdioClientError::NotConnected)
    }

    fn spawn_stderr_reader(&self, stderr: tokio::process::ChildStderr) -> JoinHandle<()> {
        let stderr_tail = Arc::clone(&self.stderr);
        tokio::spawn(async move {
            let mut stderr = BufReader::new(stderr);
            let mut buffer = [0_u8; 1024];
            loop {
                let Ok(read) = stderr.read(&mut buffer).await else {
                    return;
                };
                if read == 0 {
                    return;
                }
                stderr_tail
                    .lock()
                    .await
                    .push(&String::from_utf8_lossy(&buffer[..read]));
            }
        })
    }

    fn spawn_close_monitor(&self, peer: Peer<RoleClient>) -> JoinHandle<()> {
        let state = Arc::clone(&self.state);
        let stderr = Arc::clone(&self.stderr);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if !peer.is_transport_closed() {
                    continue;
                }
                let stderr = stderr.lock().await.snapshot();
                let listener = {
                    let mut state = state.lock().await;
                    if state.closed || !state.ready {
                        return;
                    }
                    let reason = UnexpectedCloseReason {
                        error: None,
                        stderr: (!stderr.is_empty()).then_some(stderr),
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
                return;
            }
        })
    }

    async fn call_tool_with_options(
        &self,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<McpToolResult, McpStdioClientError> {
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
            .map_err(|error| McpStdioClientError::Runtime {
                operation: "tool request",
                message: error.to_string(),
            })?
            .await_response()
            .await
            .map_err(|error| McpStdioClientError::Runtime {
                operation: "tool request",
                message: error.to_string(),
            })?;
        let ServerResult::CallToolResult(result) = result else {
            return Err(McpStdioClientError::Runtime {
                operation: "tool request",
                message: "received an unexpected MCP response".into(),
            });
        };
        let value = serde_json::to_value(result).map_err(|error| McpStdioClientError::Runtime {
            operation: "tool result conversion",
            message: error.to_string(),
        })?;
        Ok(to_mcp_tool_result(&value))
    }
}

#[async_trait]
impl McpClient for StdioMcpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpClientError> {
        let peer = self.peer().await.map_err(box_stdio_error)?;
        let result = peer.list_tools(None).await.map_err(|error| {
            box_stdio_error(McpStdioClientError::Runtime {
                operation: "tool listing",
                message: error.to_string(),
            })
        })?;
        result
            .tools
            .into_iter()
            .map(|tool| {
                let mut value = serde_json::to_value(tool).map_err(|error| {
                    box_stdio_error(McpStdioClientError::Runtime {
                        operation: "tool definition conversion",
                        message: error.to_string(),
                    })
                })?;
                if value.get("description").is_none() {
                    value["description"] = Value::String(String::new());
                }
                serde_json::from_value(value).map_err(|error| {
                    box_stdio_error(McpStdioClientError::Runtime {
                        operation: "tool definition conversion",
                        message: error.to_string(),
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
        result.map_err(box_stdio_error)
    }
}

fn box_stdio_error(error: McpStdioClientError) -> McpClientError {
    Box::new(error)
}

// Original: resolveStdioCwd(). `PathBuf` makes the resolved value directly
// usable by Tokio's child-process command builder.
pub fn resolve_stdio_cwd(config_cwd: Option<&str>, default_cwd: Option<&Path>) -> Option<PathBuf> {
    let config_cwd = config_cwd?;
    let config_path = Path::new(config_cwd);
    if config_path.is_absolute() {
        return Some(config_path.into());
    }

    let Some(default_cwd) = default_cwd else {
        return Some(config_path.into());
    };
    let base = if default_cwd.is_absolute() {
        default_cwd.into()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(default_cwd)
    };
    Some(normalize_lexically(&base.join(config_path)))
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

// Original: mergeStdioEnv().
pub fn merge_stdio_env(config_env: Option<&Env>) -> Env {
    merge_stdio_env_with_parent(config_env, std::env::vars())
}

fn merge_stdio_env_with_parent(
    config_env: Option<&Env>,
    parent_env: impl IntoIterator<Item = (String, String)>,
) -> Env {
    let mut merged: Env = parent_env.into_iter().collect();
    if let Some(config_env) = config_env {
        merged.extend(config_env.clone());
    }
    merged.extend(proxy_env_for_child(&merged));
    reconcile_child_no_proxy(&mut merged, config_env);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(entries: &[(&str, &str)]) -> Env {
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect()
    }

    #[test]
    fn default_state_is_not_started_connected_or_closed() {
        let state = StdioMcpClientState::default();
        assert!(!state.started);
        assert!(!state.closed);
        assert!(!state.ready);
        assert!(state.running.is_none());
        assert!(state.unexpected_close_listener.is_none());
        assert!(state.pending_unexpected_close.is_none());
        assert!(state.stderr_task.is_none());
        assert!(state.close_monitor.is_none());
    }

    #[test]
    fn retains_only_the_configured_stderr_tail() {
        let mut tail = BoundedTail::new(4);
        tail.push("ab");
        tail.push("cdef");
        assert_eq!(tail.snapshot(), "cdef");
    }

    #[test]
    fn resolves_relative_stdio_cwd_against_default_cwd() {
        assert_eq!(
            resolve_stdio_cwd(
                Some("servers/../mcp"),
                Some(Path::new("/workspace/project"))
            ),
            Some(PathBuf::from("/workspace/project/mcp"))
        );
        assert_eq!(
            resolve_stdio_cwd(Some("/tmp/mcp"), Some(Path::new("/workspace/project"))),
            Some(PathBuf::from("/tmp/mcp"))
        );
        assert_eq!(
            resolve_stdio_cwd(None, Some(Path::new("/workspace/project"))),
            None
        );
    }

    #[test]
    fn merges_overrides_and_reconciles_proxy_environment() {
        let config = env(&[
            ("HTTP_PROXY", "http://configured:8080"),
            ("NO_PROXY", "example.com"),
        ]);
        let result = merge_stdio_env_with_parent(
            Some(&config),
            env(&[("HTTP_PROXY", "http://parent:8080"), ("PATH", "/bin")]),
        );
        assert_eq!(result.get("PATH").map(String::as_str), Some("/bin"));
        assert_eq!(
            result.get("HTTP_PROXY").map(String::as_str),
            Some("http://configured:8080")
        );
        assert_eq!(
            result.get("NO_PROXY").map(String::as_str),
            Some("example.com,localhost,127.0.0.1,::1,[::1]")
        );
        assert_eq!(result.get("no_proxy"), result.get("NO_PROXY"));
        assert_eq!(
            result.get("NODE_USE_ENV_PROXY").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn rejects_non_local_stdio_executor_before_starting_a_process() {
        let result = StdioMcpClient::new(
            McpServerStdioConfig {
                command: "ignored".into(),
                args: None,
                env: None,
                cwd: None,
                executor: Some(crate::agent::mcp::McpExecutor::Kaos),
                common: Default::default(),
            },
            StdioMcpClientOptions::default(),
        );
        let Err(error) = result else {
            panic!("kaos executor should be rejected");
        };
        assert_eq!(
            error.to_string(),
            "MCP stdio executor 'kaos' is not yet implemented"
        );
    }

    #[tokio::test]
    async fn reports_not_connected_until_connect_completes() {
        let client = StdioMcpClient::new(
            McpServerStdioConfig {
                command: "ignored".into(),
                args: None,
                env: Some(Env::new()),
                cwd: None,
                executor: None,
                common: Default::default(),
            },
            StdioMcpClientOptions::default(),
        )
        .unwrap();
        let error = client.list_tools().await.unwrap_err();
        assert_eq!(error.to_string(), "MCP stdio client is not connected");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connects_to_a_stdio_server_and_adapts_tool_requests() {
        let client = StdioMcpClient::new(
            McpServerStdioConfig {
                command: "sh".into(),
                args: Some(vec![
                    "-c".into(),
                    concat!(
                        "IFS= read -r _; ",
                        "printf '%s\\n' ",
                        "'{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"mock\",\"version\":\"1\"}}}'; ",
                        "IFS= read -r _; IFS= read -r _; ",
                        "printf '%s\\n' ",
                        "'{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"echo\",\"description\":\"Echo\",\"inputSchema\":{\"type\":\"object\"}}]}}'; ",
                        "IFS= read -r _; ",
                        "printf '%s\\n' ",
                        "'{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}'; ",
                        "cat"
                    )
                    .into(),
                ]),
                env: Some(Env::new()),
                cwd: None,
                executor: None,
                common: Default::default(),
            },
            StdioMcpClientOptions::default(),
        )
        .unwrap();
        client.connect().await.unwrap();

        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, "Echo");

        let result = client.call_tool("echo", Map::new(), None).await.unwrap();
        assert_eq!(result.content[0].text.as_deref(), Some("done"));
        assert!(!result.is_error);

        client.close().await.unwrap();
    }
}
