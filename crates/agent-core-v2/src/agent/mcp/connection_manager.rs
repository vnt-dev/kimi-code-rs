//! MCP server connection orchestration.
//!
//! Original: `agent/mcp/connection-manager.ts`, `McpConnectionManager`.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::{Mutex, watch};

use crate::{
    _base::event::{Emitter, Event},
    _base::log::contract::{LogContext, LogPayload, Logger},
    _base::utils::abort::{AbortError, AbortSignal},
    agent::mcp::{
        HttpMcpClient, HttpMcpClientOptions, McpClient, McpClientError, McpServerConfig,
        McpToolDefinition, SseMcpClient, SseMcpClientOptions, StdioMcpClient,
        StdioMcpClientOptions, UnexpectedCloseReason, assert_mcp_input_schema,
    },
    kosong::contract::tool::Tool,
};

use super::client_stdio::UnexpectedCloseListener;

pub const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 30_000;

// MIGRATION-TODO:
// Original: `McpConnectionManager.resolveOAuthProvider()` and
// `McpConnectionManager.shouldMarkNeedsAuth()`.
// Missing dependency: the session-scoped `McpOAuthService` and its RMCP auth
// bridge have not yet been migrated. Remote OAuth failures therefore remain
// `Failed` rather than `NeedsAuth` until that service is integrated.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpServerStatus {
    Pending,
    Connected,
    Failed,
    Disabled,
    NeedsAuth,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpServerEntry {
    pub name: String,
    pub transport: String,
    pub status: McpServerStatus,
    pub tool_count: usize,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpConnectionManagerError {
    #[error("Unknown MCP server: {0}")]
    NotFound(String),
    #[error("MCP server is disabled: {0}")]
    Disabled(String),
    #[error("MCP server startup failed: {0}")]
    Startup(String),
}

#[async_trait]
trait RuntimeMcpClient: McpClient {
    async fn connect_runtime(&self) -> Result<(), McpClientError>;
    async fn close_runtime(&self) -> Result<(), McpClientError>;
    async fn on_unexpected_close_runtime(&self, listener: UnexpectedCloseListener);
    async fn stderr_snapshot_runtime(&self) -> Option<String> {
        None
    }
}

#[async_trait]
impl RuntimeMcpClient for StdioMcpClient {
    async fn connect_runtime(&self) -> Result<(), McpClientError> {
        self.connect()
            .await
            .map_err(|error| Box::new(error) as McpClientError)
    }
    async fn close_runtime(&self) -> Result<(), McpClientError> {
        self.close()
            .await
            .map_err(|error| Box::new(error) as McpClientError)
    }
    async fn on_unexpected_close_runtime(&self, listener: UnexpectedCloseListener) {
        self.on_unexpected_close(listener).await;
    }
    async fn stderr_snapshot_runtime(&self) -> Option<String> {
        Some(self.stderr_snapshot().await)
    }
}

#[async_trait]
impl RuntimeMcpClient for HttpMcpClient {
    async fn connect_runtime(&self) -> Result<(), McpClientError> {
        self.connect()
            .await
            .map_err(|error| Box::new(error) as McpClientError)
    }
    async fn close_runtime(&self) -> Result<(), McpClientError> {
        self.close()
            .await
            .map_err(|error| Box::new(error) as McpClientError)
    }
    async fn on_unexpected_close_runtime(&self, listener: UnexpectedCloseListener) {
        self.on_unexpected_close(listener).await;
    }
}

#[async_trait]
impl RuntimeMcpClient for SseMcpClient {
    async fn connect_runtime(&self) -> Result<(), McpClientError> {
        self.connect()
            .await
            .map_err(|error| Box::new(error) as McpClientError)
    }
    async fn close_runtime(&self) -> Result<(), McpClientError> {
        self.close()
            .await
            .map_err(|error| Box::new(error) as McpClientError)
    }
    async fn on_unexpected_close_runtime(&self, listener: UnexpectedCloseListener) {
        self.on_unexpected_close(listener).await;
    }
}

struct InternalEntry {
    name: String,
    config: McpServerConfig,
    attempt_id: u64,
    status: McpServerStatus,
    tools: Option<Vec<Tool>>,
    raw_tools: Option<Vec<McpToolDefinition>>,
    enabled_names: Option<HashSet<String>>,
    error: Option<String>,
    client: Option<Arc<dyn RuntimeMcpClient>>,
}

#[derive(Clone, Default)]
pub struct McpConnectionManagerOptions {
    pub stdio_cwd: Option<PathBuf>,
    pub env_lookup: Option<Arc<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    pub log: Option<Arc<dyn Logger>>,
}

struct SilentLogger;

impl Logger for SilentLogger {
    fn error(&self, _message: &str, _payload: Option<LogPayload>) {}
    fn warn(&self, _message: &str, _payload: Option<LogPayload>) {}
    fn info(&self, _message: &str, _payload: Option<LogPayload>) {}
    fn debug(&self, _message: &str, _payload: Option<LogPayload>) {}
    fn child(&self, _context: LogContext) -> Arc<dyn Logger> {
        Arc::new(Self)
    }
}

pub struct McpConnectionManager {
    entries: Mutex<HashMap<String, InternalEntry>>,
    changed: Arc<Emitter<McpServerEntry>>,
    options: McpConnectionManagerOptions,
    log: Arc<dyn Logger>,
    initial_load_attempt: Mutex<u64>,
    initial_load_completed: watch::Sender<u64>,
    initial_started: Mutex<Option<Instant>>,
    initial_finished: Mutex<Option<Instant>>,
}

impl McpConnectionManager {
    // Original: McpConnectionManager.constructor().
    pub fn new(options: McpConnectionManagerOptions) -> Arc<Self> {
        let (initial_load_completed, _) = watch::channel(0_u64);
        Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            changed: Arc::new(Emitter::new()),
            log: options
                .log
                .clone()
                .unwrap_or_else(|| Arc::new(SilentLogger)),
            options,
            initial_load_attempt: Mutex::new(0),
            initial_load_completed,
            initial_started: Mutex::new(None),
            initial_finished: Mutex::new(None),
        })
    }

    pub fn on_status_change(&self) -> Event<McpServerEntry> {
        self.changed.event()
    }

    pub async fn list(&self) -> Vec<McpServerEntry> {
        self.entries
            .lock()
            .await
            .values()
            .map(to_public_entry)
            .collect()
    }
    pub async fn get(&self, name: &str) -> Option<McpServerEntry> {
        self.entries.lock().await.get(name).map(to_public_entry)
    }

    pub async fn get_remote_server_url(&self, name: &str) -> Option<String> {
        self.entries
            .lock()
            .await
            .get(name)
            .and_then(|entry| match &entry.config {
                McpServerConfig::Http(config) | McpServerConfig::Sse(config) => {
                    Some(config.url.clone())
                }
                McpServerConfig::Stdio(_) => None,
            })
    }

    // Original: getHttpServerUrl(). The source exposes this legacy alias even
    // though it also returns SSE endpoint URLs.
    pub async fn get_http_server_url(&self, name: &str) -> Option<String> {
        self.get_remote_server_url(name).await
    }

    pub async fn resolved(
        &self,
        name: &str,
    ) -> Option<(
        Arc<dyn McpClient>,
        Vec<Tool>,
        Vec<McpToolDefinition>,
        HashSet<String>,
    )> {
        let entries = self.entries.lock().await;
        let entry = entries.get(name)?;
        if entry.status != McpServerStatus::Connected {
            return None;
        }
        let client: Arc<dyn McpClient> = entry.client.as_ref()?.clone();
        Some((
            client,
            entry.tools.clone()?,
            entry.raw_tools.clone()?,
            entry.enabled_names.clone().unwrap_or_default(),
        ))
    }

    // Original: connectAll(). Failures are recorded per server; the aggregate
    // deliberately resolves once every configured connection has settled.
    pub async fn connect_all(self: &Arc<Self>, configs: HashMap<String, McpServerConfig>) {
        let attempt_id = {
            let mut attempt = self.initial_load_attempt.lock().await;
            *attempt += 1;
            *attempt
        };
        *self.initial_started.lock().await = Some(Instant::now());
        *self.initial_finished.lock().await = None;
        let mut tasks = Vec::new();
        for (name, config) in configs {
            let manager = Arc::clone(self);
            tasks.push(tokio::spawn(async move {
                let _ = manager.connect(name, config).await;
            }));
        }
        for task in tasks {
            let _ = task.await;
        }
        if *self.initial_load_attempt.lock().await == attempt_id {
            *self.initial_finished.lock().await = Some(Instant::now());
            self.initial_load_completed.send_replace(attempt_id);
        }
    }

    pub async fn connect(
        self: &Arc<Self>,
        name: String,
        config: McpServerConfig,
    ) -> Result<(), McpConnectionManagerError> {
        let old_client = {
            let mut entries = self.entries.lock().await;
            entries.remove(&name).and_then(|entry| entry.client)
        };
        if let Some(client) = old_client {
            let _ = client.close_runtime().await;
        }
        let disabled = config.common().enabled == Some(false);
        let entry = InternalEntry {
            name: name.clone(),
            config,
            attempt_id: 1,
            status: if disabled {
                McpServerStatus::Disabled
            } else {
                McpServerStatus::Pending
            },
            tools: None,
            raw_tools: None,
            enabled_names: None,
            error: None,
            client: None,
        };
        self.entries.lock().await.insert(name.clone(), entry);
        self.emit(&name).await;
        if disabled {
            return Ok(());
        }
        self.connect_one(name, 1).await
    }

    pub async fn reconnect(self: &Arc<Self>, name: &str) -> Result<(), McpConnectionManagerError> {
        let (config, attempt, client) = {
            let mut entries = self.entries.lock().await;
            let entry = entries
                .get_mut(name)
                .ok_or_else(|| McpConnectionManagerError::NotFound(name.into()))?;
            if entry.config.common().enabled == Some(false) {
                return Err(McpConnectionManagerError::Disabled(name.into()));
            }
            entry.attempt_id += 1;
            entry.status = McpServerStatus::Pending;
            entry.tools = None;
            entry.raw_tools = None;
            entry.enabled_names = None;
            entry.error = None;
            (entry.config.clone(), entry.attempt_id, entry.client.take())
        };
        if let Some(client) = client {
            let _ = client.close_runtime().await;
        }
        let _ = config;
        self.emit(name).await;
        self.connect_one(name.into(), attempt).await
    }

    pub async fn remove(&self, name: &str) -> bool {
        let mut entry = match self.entries.lock().await.remove(name) {
            Some(entry) => entry,
            None => return false,
        };
        let client = entry.client.take();
        entry.status = McpServerStatus::Disabled;
        entry.tools = None;
        entry.raw_tools = None;
        entry.enabled_names = None;
        entry.error = None;
        self.changed.fire(&to_public_entry(&entry));
        if let Some(client) = client {
            let _ = client.close_runtime().await;
        }
        true
    }

    pub async fn shutdown(&self) {
        let clients = self
            .entries
            .lock()
            .await
            .drain()
            .filter_map(|(_, entry)| entry.client)
            .collect::<Vec<_>>();
        for client in clients {
            let _ = client.close_runtime().await;
        }
    }

    pub async fn initial_load_duration_ms(&self) -> u128 {
        let started = *self.initial_started.lock().await;
        let Some(started) = started else {
            return 0;
        };
        self.initial_finished
            .lock()
            .await
            .unwrap_or_else(Instant::now)
            .saturating_duration_since(started)
            .as_millis()
    }

    // Original: waitForInitialLoad(). A caller waits for the latest initial
    // batch that existed when it entered, rather than for later reconnects.
    pub async fn wait_for_initial_load(
        &self,
        signal: Option<&AbortSignal>,
    ) -> Result<(), Arc<AbortError>> {
        if let Some(signal) = signal {
            signal.throw_if_aborted()?;
        }
        let target_attempt = *self.initial_load_attempt.lock().await;
        let mut completed = self.initial_load_completed.subscribe();
        loop {
            if *completed.borrow() >= target_attempt {
                return Ok(());
            }
            if let Some(signal) = signal {
                tokio::select! {
                    biased;
                    reason = signal.cancelled() => return Err(reason),
                    changed = completed.changed() => {
                        if changed.is_err() {
                            return Ok(());
                        }
                    }
                }
            } else if completed.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    async fn connect_one(
        self: &Arc<Self>,
        name: String,
        attempt_id: u64,
    ) -> Result<(), McpConnectionManagerError> {
        let config = {
            let entries = self.entries.lock().await;
            let entry = entries
                .get(&name)
                .ok_or_else(|| McpConnectionManagerError::NotFound(name.clone()))?;
            entry.config.clone()
        };
        let client = match self.create_client(&config) {
            Ok(client) => client,
            Err(error) => {
                let mut entries = self.entries.lock().await;
                let Some(entry) = entries.get_mut(&name) else {
                    return Ok(());
                };
                if entry.attempt_id != attempt_id {
                    return Ok(());
                }
                entry.status = McpServerStatus::Failed;
                entry.error = Some(error.to_string());
                drop(entries);
                self.emit(&name).await;
                return Ok(());
            }
        };
        let timeout = config
            .common()
            .startup_timeout_ms
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT_MS);
        let result = tokio::time::timeout(std::time::Duration::from_millis(timeout), async {
            client.connect_runtime().await?;
            let raw_tools = client.list_tools().await?;
            Ok::<_, McpClientError>(raw_tools)
        })
        .await;
        let outcome = match result {
            Ok(Ok(raw_tools)) => Ok(raw_tools),
            Ok(Err(error)) => Err(format_startup_error(error.to_string(), &client).await),
            Err(_) => Err(format!("Timed out after {timeout}ms")),
        };
        let mut connected = false;
        {
            let mut entries = self.entries.lock().await;
            let Some(entry) = entries.get_mut(&name) else {
                return Ok(());
            };
            if entry.attempt_id != attempt_id {
                return Ok(());
            }
            match outcome {
                Ok(raw_tools) => {
                    let tools = raw_tools
                        .iter()
                        .map(to_tool)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string());
                    match tools {
                        Ok(tools) => {
                            entry.enabled_names =
                                Some(compute_enabled_names(&entry.config, &tools));
                            entry.tools = Some(tools);
                            entry.raw_tools = Some(raw_tools);
                            entry.client = Some(Arc::clone(&client));
                            entry.status = McpServerStatus::Connected;
                            connected = true;
                        }
                        Err(error) => {
                            entry.status = McpServerStatus::Failed;
                            entry.error = Some(error);
                        }
                    }
                }
                Err(error) => {
                    entry.status = McpServerStatus::Failed;
                    entry.error = Some(error);
                }
            }
        }
        if connected {
            self.install_close_watch(name.clone(), attempt_id, Arc::clone(&client))
                .await;
        } else {
            let _ = client.close_runtime().await;
        }
        self.emit(&name).await;
        Ok(())
    }

    fn create_client(
        &self,
        config: &McpServerConfig,
    ) -> Result<Arc<dyn RuntimeMcpClient>, McpConnectionManagerError> {
        let env = self
            .options
            .env_lookup
            .clone()
            .unwrap_or_else(|| Arc::new(|name| std::env::var(name).ok()));
        match config {
            McpServerConfig::Stdio(config) => Ok(Arc::new(
                StdioMcpClient::new(
                    config.clone(),
                    StdioMcpClientOptions {
                        default_cwd: self.options.stdio_cwd.clone(),
                        tool_call_timeout_ms: config.common.tool_timeout_ms,
                        ..Default::default()
                    },
                )
                .map_err(|error| McpConnectionManagerError::Startup(error.to_string()))?,
            )),
            McpServerConfig::Http(config) => Ok(Arc::new(
                HttpMcpClient::new(
                    config.clone(),
                    HttpMcpClientOptions {
                        tool_call_timeout_ms: config.common.tool_timeout_ms,
                        ..Default::default()
                    },
                    move |name| env(name),
                )
                .map_err(|error| McpConnectionManagerError::Startup(error.to_string()))?,
            )),
            McpServerConfig::Sse(config) => Ok(Arc::new(
                SseMcpClient::new(
                    config.clone(),
                    SseMcpClientOptions {
                        tool_call_timeout_ms: config.common.tool_timeout_ms,
                        ..Default::default()
                    },
                    move |name| env(name),
                )
                .map_err(|error| McpConnectionManagerError::Startup(error.to_string()))?,
            )),
        }
    }

    async fn install_close_watch(
        self: &Arc<Self>,
        name: String,
        attempt_id: u64,
        client: Arc<dyn RuntimeMcpClient>,
    ) {
        let weak = Arc::downgrade(self);
        let watched_client = Arc::clone(&client);
        client
            .on_unexpected_close_runtime(Arc::new(move |reason| {
                let name = name.clone();
                let weak = weak.clone();
                let client = Arc::clone(&watched_client);
                tokio::spawn(async move {
                    if let Some(manager) = weak.upgrade() {
                        let mut entries = manager.entries.lock().await;
                        let Some(entry) = entries.get_mut(&name) else {
                            return;
                        };
                        if entry.attempt_id != attempt_id
                            || entry.status != McpServerStatus::Connected
                        {
                            return;
                        }
                        entry.status = McpServerStatus::Failed;
                        entry.error = Some(format_unexpected_close_error(&name, &reason));
                        entry.tools = None;
                        entry.raw_tools = None;
                        entry.enabled_names = None;
                        entry.client = None;
                        drop(entries);
                        let _ = client.close_runtime().await;
                        manager.emit(&name).await;
                    }
                });
            }))
            .await;
    }

    async fn emit(&self, name: &str) {
        if let Some(entry) = self.entries.lock().await.get(name).map(to_public_entry) {
            if matches!(
                entry.status,
                McpServerStatus::Failed | McpServerStatus::NeedsAuth
            ) {
                self.log.error(
                    "mcp server unavailable",
                    Some(LogPayload::Context(Map::from_iter([
                        ("server".into(), Value::String(entry.name.clone())),
                        ("transport".into(), Value::String(entry.transport.clone())),
                        (
                            "status".into(),
                            Value::String(match entry.status {
                                McpServerStatus::Failed => "failed".into(),
                                McpServerStatus::NeedsAuth => "needs-auth".into(),
                                _ => unreachable!("only unavailable statuses are logged"),
                            }),
                        ),
                        (
                            "reason".into(),
                            entry.error.clone().map_or(Value::Null, Value::String),
                        ),
                    ]))),
                );
            }
            self.changed.fire(&entry);
        }
    }
}

fn to_tool(tool: &McpToolDefinition) -> Result<Tool, McpConnectionManagerError> {
    Ok(Tool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: assert_mcp_input_schema(&tool.name, &tool.input_schema)
            .map_err(|error| McpConnectionManagerError::Startup(error.to_string()))?,
        deferred: None,
    })
}
fn compute_enabled_names(config: &McpServerConfig, tools: &[Tool]) -> HashSet<String> {
    let enabled = config
        .common()
        .enabled_tools
        .as_ref()
        .map(|values| values.iter().collect::<HashSet<_>>());
    let disabled = config
        .common()
        .disabled_tools
        .as_ref()
        .map(|values| values.iter().collect::<HashSet<_>>());
    tools
        .iter()
        .filter(|tool| {
            enabled
                .as_ref()
                .is_none_or(|enabled| enabled.contains(&tool.name))
                && disabled
                    .as_ref()
                    .is_none_or(|disabled| !disabled.contains(&tool.name))
        })
        .map(|tool| tool.name.clone())
        .collect()
}
fn to_public_entry(entry: &InternalEntry) -> McpServerEntry {
    McpServerEntry {
        name: entry.name.clone(),
        transport: entry.config.transport().into(),
        status: entry.status,
        tool_count: (entry.status == McpServerStatus::Connected)
            .then(|| entry.enabled_names.as_ref().map_or(0, HashSet::len))
            .unwrap_or(0),
        error: entry.error.clone(),
    }
}
fn format_unexpected_close_error(name: &str, reason: &UnexpectedCloseReason) -> String {
    let mut parts = vec![format!("MCP server \"{name}\" closed unexpectedly")];
    if let Some(error) = &reason.error {
        parts.push(error.clone());
    }
    if let Some(stderr) = &reason.stderr
        && !stderr.is_empty()
    {
        parts.push(format!("stderr: {}", stderr.trim_end()));
    }
    parts.join("\n")
}

async fn format_startup_error(error: String, client: &Arc<dyn RuntimeMcpClient>) -> String {
    let Some(stderr) = client.stderr_snapshot_runtime().await else {
        return error;
    };
    if stderr.is_empty() {
        return error;
    }
    format!("{error}\nstderr: {}", stderr.trim_end())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        _base::log::contract::{LogContext, LogPayload},
        _base::utils::abort::{AbortController, AbortError},
        agent::mcp::{McpExecutor, McpServerCommonFields, McpServerStdioConfig},
    };

    fn stdio_config(common: McpServerCommonFields) -> McpServerConfig {
        McpServerConfig::Stdio(McpServerStdioConfig {
            command: "server".into(),
            args: None,
            env: None,
            cwd: None,
            executor: None,
            common,
        })
    }

    #[test]
    fn computes_enabled_names_with_enabled_and_disabled_filters() {
        let tools = vec![
            Tool {
                name: "one".into(),
                description: String::new(),
                parameters: serde_json::Map::new(),
                deferred: None,
            },
            Tool {
                name: "two".into(),
                description: String::new(),
                parameters: serde_json::Map::new(),
                deferred: None,
            },
        ];
        let config = stdio_config(McpServerCommonFields {
            enabled_tools: Some(vec!["one".into(), "two".into(), "missing".into()]),
            disabled_tools: Some(vec!["two".into()]),
            ..Default::default()
        });

        assert_eq!(
            compute_enabled_names(&config, &tools),
            HashSet::from(["one".into()])
        );
    }

    #[tokio::test]
    async fn disabled_server_is_listed_and_removed_with_status_events() {
        let manager = McpConnectionManager::new(Default::default());
        let statuses = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&statuses);
        let _listener = manager.on_status_change().subscribe(move |entry| {
            captured.lock().unwrap().push(entry.status);
        });

        manager
            .connect(
                "disabled".into(),
                stdio_config(McpServerCommonFields {
                    enabled: Some(false),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();

        assert_eq!(
            manager.get("disabled").await.unwrap().status,
            McpServerStatus::Disabled
        );
        assert!(manager.remove("disabled").await);
        assert!(!manager.remove("disabled").await);
        assert_eq!(
            *statuses.lock().unwrap(),
            vec![McpServerStatus::Disabled, McpServerStatus::Disabled]
        );
    }

    #[tokio::test]
    async fn client_creation_failures_are_recorded_and_logged() {
        #[derive(Default)]
        struct TestLogger(Mutex<Vec<String>>);
        impl Logger for TestLogger {
            fn error(&self, message: &str, _payload: Option<LogPayload>) {
                self.0.lock().unwrap().push(message.into());
            }
            fn warn(&self, _message: &str, _payload: Option<LogPayload>) {}
            fn info(&self, _message: &str, _payload: Option<LogPayload>) {}
            fn debug(&self, _message: &str, _payload: Option<LogPayload>) {}
            fn child(&self, _context: LogContext) -> Arc<dyn Logger> {
                Arc::new(Self::default())
            }
        }

        let log = Arc::new(TestLogger::default());
        let manager = McpConnectionManager::new(McpConnectionManagerOptions {
            log: Some(log.clone()),
            ..Default::default()
        });
        let mut config = match stdio_config(Default::default()) {
            McpServerConfig::Stdio(config) => config,
            _ => unreachable!(),
        };
        config.executor = Some(McpExecutor::Kaos);

        manager
            .connect("unsupported".into(), McpServerConfig::Stdio(config))
            .await
            .unwrap();

        let entry = manager.get("unsupported").await.unwrap();
        assert_eq!(entry.status, McpServerStatus::Failed);
        assert!(entry.error.unwrap().contains("executor 'kaos'"));
        assert_eq!(*log.0.lock().unwrap(), vec!["mcp server unavailable"]);
    }

    #[tokio::test]
    async fn waits_for_the_latest_initial_batch_and_honors_cancellation() {
        let manager = McpConnectionManager::new(Default::default());
        manager.connect_all(HashMap::new()).await;
        manager.wait_for_initial_load(None).await.unwrap();

        let abort = AbortController::new();
        abort.abort(Some(AbortError::new("session closed")));
        let signal = abort.signal();
        assert_eq!(
            manager
                .wait_for_initial_load(Some(&signal))
                .await
                .unwrap_err()
                .to_string(),
            "session closed"
        );
    }
}
