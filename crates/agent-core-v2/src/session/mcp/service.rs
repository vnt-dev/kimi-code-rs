//! Session-owned MCP connection startup.
//!
//! Original: `session/mcp/sessionMcp.ts` and
//! `session/mcp/sessionMcpService.ts`.

use std::{collections::HashMap, ops::Deref, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde_json::{Map, Value};
use tokio::sync::{Mutex, watch};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        log::{LOG_SERVICE_ID, LogContext, LogPayload, LogServiceHandle, Logger},
    },
    agent::mcp::{
        McpConnectionManager, McpConnectionManagerOptions, McpOAuthService, McpOAuthServiceOptions,
        McpServerConfig, ResolveSessionMcpConfigInput, SessionMcpConfig, merge_caller_mcp_servers,
        oauth::store::AtomicMcpOAuthStore, resolve_session_mcp_config,
    },
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        plugin::{PLUGIN_SERVICE_ID, PluginServiceHandle},
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryProperties, TelemetryServiceHandle},
    },
    persistence::interface::atomic_document_store::ATOMIC_DOCUMENT_STORE_SERVICE_ID,
    session::workspace_context::SESSION_WORKSPACE_CONTEXT_ID,
};

pub type McpPluginServersResult =
    Result<HashMap<String, McpServerConfig>, Box<dyn std::error::Error + Send + Sync>>;

#[async_trait]
pub trait McpPluginServerSource: Send + Sync {
    async fn enabled_mcp_servers(&self) -> McpPluginServersResult;
}

pub type SessionMcpConfigLoader = Arc<
    dyn Fn(
            ResolveSessionMcpConfigInput,
        ) -> BoxFuture<
            'static,
            Result<Option<SessionMcpConfig>, crate::agent::mcp::McpConfigLoadError>,
        > + Send
        + Sync,
>;

#[derive(Clone)]
pub struct SessionMcpServiceOptions {
    pub work_dir: std::path::PathBuf,
    pub home_dir: std::path::PathBuf,
    pub plugin_servers: Arc<dyn McpPluginServerSource>,
    pub log: Arc<dyn Logger>,
    pub telemetry: TelemetryServiceHandle,
    pub config_loader: Option<SessionMcpConfigLoader>,
    /// Source construction owns this service through the session atomic
    /// document store. It is injected here because this Rust service uses
    /// narrow dependencies rather than the TypeScript DI container.
    pub oauth_service: Option<Arc<McpOAuthService>>,
}

struct InitialLoad {
    started: bool,
    completed: watch::Sender<bool>,
}

/// Session-wide MCP manager shared by all agents in that session.
///
/// Rust adaptation: dependencies are supplied as narrow contracts so this
/// service can be constructed outside the TypeScript DI container. The shared
/// manager itself is eager because construction has no I/O; server connection
/// is still lazy and cached by `ensure_mcp_ready`.
pub struct SessionMcpService {
    manager: Arc<McpConnectionManager>,
    oauth_service: Option<Arc<McpOAuthService>>,
    options: SessionMcpServiceOptions,
    initial_load: Mutex<InitialLoad>,
}

#[derive(Clone)]
pub struct SessionMcpServiceHandle(pub Arc<SessionMcpService>);

impl Deref for SessionMcpServiceHandle {
    type Target = SessionMcpService;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for SessionMcpServiceHandle {
    fn dispose(&self) -> DisposeResult {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let service = Arc::clone(&self.0);
            runtime.spawn(async move {
                service.shutdown().await;
            });
        }
        Ok(())
    }
}

pub const SESSION_MCP_SERVICE_ID: ServiceIdentifier<SessionMcpServiceHandle> =
    ServiceIdentifier::new("sessionMcpService");

struct PluginServerSource(PluginServiceHandle);

#[async_trait]
impl McpPluginServerSource for PluginServerSource {
    async fn enabled_mcp_servers(&self) -> McpPluginServersResult {
        self.0.enabled_mcp_servers().await
    }
}

struct SessionMcpLogger(LogServiceHandle);

impl Logger for SessionMcpLogger {
    fn error(&self, message: &str, payload: Option<LogPayload>) {
        self.0.0.error(message, payload);
    }
    fn warn(&self, message: &str, payload: Option<LogPayload>) {
        self.0.0.warn(message, payload);
    }
    fn info(&self, message: &str, payload: Option<LogPayload>) {
        self.0.0.info(message, payload);
    }
    fn debug(&self, message: &str, payload: Option<LogPayload>) {
        self.0.0.debug(message, payload);
    }
    fn child(&self, context: LogContext) -> Arc<dyn Logger> {
        self.0.0.child(context)
    }
}

impl SessionMcpService {
    // Original: SessionMcpService.constructor().
    pub fn new(options: SessionMcpServiceOptions) -> Arc<Self> {
        let (completed, _) = watch::channel(false);
        let manager = McpConnectionManager::new(McpConnectionManagerOptions {
            stdio_cwd: Some(options.work_dir.clone()),
            log: Some(Arc::clone(&options.log)),
            oauth_service: options.oauth_service.clone(),
            ..Default::default()
        });
        Arc::new(Self {
            manager,
            oauth_service: options.oauth_service.clone(),
            options,
            initial_load: Mutex::new(InitialLoad {
                started: false,
                completed,
            }),
        })
    }

    // Original: connectionManager().
    pub fn connection_manager(&self) -> Arc<McpConnectionManager> {
        Arc::clone(&self.manager)
    }

    // Original: AgentMcpService.oauthService delegation.
    pub fn oauth_service(&self) -> Option<Arc<McpOAuthService>> {
        self.oauth_service.clone()
    }

    // Original: ensureMcpReady(). The first caller's server override is the
    // only one used; subsequent callers await that same initial operation.
    pub async fn ensure_mcp_ready(
        self: &Arc<Self>,
        caller_servers: Option<HashMap<String, McpServerConfig>>,
    ) {
        let mut completion = {
            let mut initial = self.initial_load.lock().await;
            if !initial.started {
                initial.started = true;
                let service = Arc::clone(self);
                tokio::spawn(async move {
                    service.run_initial_load(caller_servers).await;
                });
            }
            initial.completed.subscribe()
        };
        if *completion.borrow() {
            return;
        }
        let _ = completion.changed().await;
    }

    // Original: Disposable cleanup registration for the manager.
    pub async fn shutdown(&self) {
        self.manager.shutdown().await;
    }

    async fn run_initial_load(&self, caller_servers: Option<HashMap<String, McpServerConfig>>) {
        if let Err(error) = self.connect_mcp_servers(caller_servers).await {
            self.options.log.error(
                "mcp initial load failed",
                Some(LogPayload::Context(Map::from_iter([(
                    "error".into(),
                    Value::String(error),
                )]))),
            );
        }
        self.initial_load.lock().await.completed.send_replace(true);
    }

    async fn connect_mcp_servers(
        &self,
        caller_servers: Option<HashMap<String, McpServerConfig>>,
    ) -> Result<(), String> {
        let input = ResolveSessionMcpConfigInput {
            cwd: self.options.work_dir.clone(),
            home_dir: Some(self.options.home_dir.clone()),
        };
        let loader = self.options.config_loader.clone().unwrap_or_else(|| {
            Arc::new(|input| Box::pin(async move { resolve_session_mcp_config(&input).await }))
        });
        let (base, plugin_servers) = tokio::join!(
            loader(input),
            self.options.plugin_servers.enabled_mcp_servers()
        );
        let base = base.map_err(|error| error.to_string())?;
        let plugin_servers = plugin_servers.map_err(|error| error.to_string())?;
        let mut servers = merge_caller_mcp_servers(base.as_ref(), caller_servers.as_ref())
            .map_or_else(HashMap::new, |config| config.servers);
        // Original object-spread order: plugin servers replace file and caller
        // entries with the same name.
        servers.extend(plugin_servers);
        if servers.is_empty() {
            return Ok(());
        }
        self.manager.connect_all(servers).await;
        self.track_mcp_initial_load().await;
        Ok(())
    }

    async fn track_mcp_initial_load(&self) {
        let entries = self
            .manager
            .list()
            .await
            .into_iter()
            .filter(|entry| entry.status != crate::agent::mcp::McpServerStatus::Disabled)
            .collect::<Vec<_>>();
        let total_count = entries.len();
        if total_count == 0 {
            return;
        }
        let connected_count = entries
            .iter()
            .filter(|entry| entry.status == crate::agent::mcp::McpServerStatus::Connected)
            .count();
        if connected_count > 0 {
            self.options.telemetry.track(
                "mcp_connected",
                Some(&TelemetryProperties::from([
                    ("server_count".into(), Some(Value::from(connected_count))),
                    ("total_count".into(), Some(Value::from(total_count))),
                ])),
            );
        }
        let failed_count = entries
            .iter()
            .filter(|entry| entry.status == crate::agent::mcp::McpServerStatus::Failed)
            .count();
        if failed_count > 0 {
            self.options.telemetry.track(
                "mcp_failed",
                Some(&TelemetryProperties::from([
                    ("failed_count".into(), Some(Value::from(failed_count))),
                    ("total_count".into(), Some(Value::from(total_count))),
                ])),
            );
        }
    }
}

pub fn register_session_mcp_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_MCP_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap: Arc<BootstrapServiceHandle> = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let plugins = accessor.get(PLUGIN_SERVICE_ID)?;
            let documents = accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let telemetry = accessor.get(TELEMETRY_SERVICE_ID)?;
            let oauth_service = Arc::new(McpOAuthService::new(McpOAuthServiceOptions {
                store: Arc::new(AtomicMcpOAuthStore::new((*documents).clone())),
                client_label: None,
            }));
            Ok(SessionMcpServiceHandle(SessionMcpService::new(
                SessionMcpServiceOptions {
                    work_dir: workspace.work_dir(),
                    home_dir: bootstrap.home_dir().to_path_buf(),
                    plugin_servers: Arc::new(PluginServerSource((*plugins).clone())),
                    log: Arc::new(SessionMcpLogger((*log).clone())),
                    telemetry: (*telemetry).clone(),
                    config_loader: None,
                    oauth_service: Some(oauth_service),
                },
            )))
        })
        .disposable(),
        InstantiationType::Eager,
        "sessionMcp",
    );
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use super::*;
    use crate::{
        _base::log::contract::{LogContext, LogPayload},
        agent::mcp::{McpServerCommonFields, McpServerStdioConfig},
        app::telemetry::contract::noop_telemetry_service,
    };

    struct EmptyPlugins;

    #[async_trait]
    impl McpPluginServerSource for EmptyPlugins {
        async fn enabled_mcp_servers(&self) -> McpPluginServersResult {
            Ok(HashMap::new())
        }
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

    fn disabled_server(command: &str) -> McpServerConfig {
        McpServerConfig::Stdio(McpServerStdioConfig {
            command: command.into(),
            args: None,
            env: None,
            cwd: None,
            executor: None,
            common: McpServerCommonFields {
                enabled: Some(false),
                ..Default::default()
            },
        })
    }

    #[tokio::test]
    async fn first_ensure_call_wins_the_caller_server_configuration() {
        let loader: SessionMcpConfigLoader = Arc::new(|_| Box::pin(async { Ok(None) }));
        let service = SessionMcpService::new(SessionMcpServiceOptions {
            work_dir: "/workspace".into(),
            home_dir: "/home/kimi".into(),
            plugin_servers: Arc::new(EmptyPlugins),
            log: Arc::new(SilentLogger),
            telemetry: noop_telemetry_service(),
            config_loader: Some(loader),
            oauth_service: None,
        });

        service
            .ensure_mcp_ready(Some(HashMap::from([(
                "first".into(),
                disabled_server("first-command"),
            )])))
            .await;
        service
            .ensure_mcp_ready(Some(HashMap::from([(
                "later".into(),
                disabled_server("later-command"),
            )])))
            .await;

        assert!(service.connection_manager().get("first").await.is_some());
        assert!(service.connection_manager().get("later").await.is_none());
    }
}
