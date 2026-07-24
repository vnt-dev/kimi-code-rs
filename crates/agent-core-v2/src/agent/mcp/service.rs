//! Agent-facing MCP tool registration service.
//!
//! Original: `agent/mcp/mcp.ts` and `agent/mcp/mcpService.ts`.

use std::{collections::HashMap, sync::Arc};

use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::{
    _base::{di::lifecycle::DisposableHandle, event::Event, utils::abort::AbortSignal},
    agent::{
        mcp::{
            McpOAuthService, McpServerEntry, McpServerStatus, qualify_mcp_tool_name,
            tools::{McpAuthToolOptions, create_mcp_auth_tool, create_mcp_tool},
        },
        tool_registry::contract::{AgentToolRegistryServiceHandle, ToolRegistrationOptions},
    },
    app::event::event_bus::{DomainEvent, EventBusHandle},
    session::mcp::SessionMcpService,
    tool::{ExecutableTool, ToolSource},
};

struct Registration {
    disposable: DisposableHandle,
}

pub struct AgentMcpService {
    session: Arc<SessionMcpService>,
    registry: AgentToolRegistryServiceHandle,
    events: EventBusHandle,
    registrations: Mutex<HashMap<String, Registration>>,
    by_server: Mutex<HashMap<String, Vec<String>>>,
    status_subscription: DisposableHandle,
}

impl AgentMcpService {
    // Original: AgentMcpService.constructor() + attachMcpTools().
    pub async fn new(
        session: Arc<SessionMcpService>,
        registry: AgentToolRegistryServiceHandle,
        events: EventBusHandle,
    ) -> Arc<Self> {
        let service = Arc::new_cyclic(|weak: &std::sync::Weak<Self>| {
            let weak_for_listener = weak.clone();
            let subscription =
                session
                    .connection_manager()
                    .on_status_change()
                    .subscribe(move |entry| {
                        let entry = entry.clone();
                        let weak = weak_for_listener.clone();
                        tokio::spawn(async move {
                            if let Some(service) = weak.upgrade() {
                                service.handle_status(entry).await;
                            }
                        });
                    });
            Self {
                session,
                registry,
                events,
                registrations: Mutex::new(HashMap::new()),
                by_server: Mutex::new(HashMap::new()),
                status_subscription: subscription,
            }
        });
        service.initialize().await;
        service
    }

    pub fn oauth_service(&self) -> Option<Arc<McpOAuthService>> {
        self.session.oauth_service()
    }

    pub async fn wait_for_initial_load(
        &self,
        signal: Option<&AbortSignal>,
    ) -> Result<(), Arc<crate::_base::utils::abort::AbortError>> {
        self.session
            .connection_manager()
            .wait_for_initial_load(signal)
            .await
    }

    pub async fn initial_load_duration_ms(&self) -> u128 {
        self.session
            .connection_manager()
            .initial_load_duration_ms()
            .await
    }

    pub async fn list(&self) -> Vec<McpServerEntry> {
        self.session.connection_manager().list().await
    }

    pub async fn reconnect(&self, name: &str, signal: Option<&AbortSignal>) -> Result<(), String> {
        if let Some(signal) = signal {
            signal
                .throw_if_aborted()
                .map_err(|error| error.to_string())?;
        }
        self.session
            .connection_manager()
            .reconnect(name)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(signal) = signal {
            signal
                .throw_if_aborted()
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn on_status_change(&self) -> Event<McpServerEntry> {
        self.session.connection_manager().on_status_change()
    }

    pub async fn initialize(self: &Arc<Self>) {
        for entry in self.list().await {
            self.handle_status(entry).await;
        }
    }

    async fn handle_status(self: &Arc<Self>, entry: McpServerEntry) {
        self.events.publish(DomainEvent::new(
            "mcp.server.status",
            Map::from_iter([(
                "server".into(),
                serde_json::to_value(&entry).unwrap_or(Value::Null),
            )]),
        ));
        match entry.status {
            McpServerStatus::Connected => {
                self.register_connected(&entry).await;
                self.tool_list_updated("mcp.connected", &entry.name);
            }
            McpServerStatus::NeedsAuth => {
                self.register_needs_auth(&entry).await;
            }
            McpServerStatus::Failed => {
                self.unregister_server(&entry.name).await;
                self.tool_list_updated("mcp.failed", &entry.name);
            }
            McpServerStatus::Disabled | McpServerStatus::Pending => {
                if self.unregister_server(&entry.name).await {
                    self.tool_list_updated("mcp.disconnected", &entry.name);
                }
            }
        }
    }

    async fn register_connected(self: &Arc<Self>, entry: &McpServerEntry) {
        let Some((client, tools, _, enabled)) = self
            .session
            .connection_manager()
            .resolved(&entry.name)
            .await
        else {
            return;
        };
        self.unregister_server(&entry.name).await;
        let mut names = Vec::new();
        for tool in tools {
            if !enabled.contains(&tool.name) {
                continue;
            }
            let name = qualify_mcp_tool_name(&entry.name, &tool.name);
            if self.registrations.lock().await.contains_key(&name) {
                continue;
            }
            let disposable = self.registry.register(
                Arc::new(create_mcp_tool(
                    name.clone(),
                    tool,
                    Arc::clone(&client),
                    Default::default(),
                )),
                ToolRegistrationOptions {
                    source: Some(ToolSource::Mcp),
                },
            );
            self.registrations
                .lock()
                .await
                .insert(name.clone(), Registration { disposable });
            names.push(name);
        }
        self.by_server
            .lock()
            .await
            .insert(entry.name.clone(), names);
    }

    async fn register_needs_auth(self: &Arc<Self>, entry: &McpServerEntry) {
        self.unregister_server(&entry.name).await;
        let (Some(oauth), Some(url)) = (
            self.oauth_service(),
            self.session
                .connection_manager()
                .get_remote_server_url(&entry.name)
                .await,
        ) else {
            return;
        };
        let server_name = entry.name.clone();
        let weak = Arc::downgrade(self);
        let tool = create_mcp_auth_tool(McpAuthToolOptions {
            server_name: server_name.clone(),
            server_url: url,
            oauth_service: oauth,
            reconnect: Arc::new(move |signal| {
                let weak = weak.clone();
                let server_name = server_name.clone();
                Box::pin(async move {
                    weak.upgrade()
                        .ok_or_else(|| "MCP service closed".to_owned())?
                        .reconnect(&server_name, Some(&signal))
                        .await
                })
            }),
            timeout_ms: None,
        });
        let name = tool.tool().name.clone();
        let disposable = self.registry.register(
            Arc::new(tool),
            ToolRegistrationOptions {
                source: Some(ToolSource::Mcp),
            },
        );
        self.registrations
            .lock()
            .await
            .insert(name.clone(), Registration { disposable });
        self.by_server
            .lock()
            .await
            .insert(entry.name.clone(), vec![name]);
        self.tool_list_updated("mcp.connected", &entry.name);
    }

    async fn unregister_server(&self, server_name: &str) -> bool {
        let names = self.by_server.lock().await.remove(server_name);
        let Some(names) = names else {
            return false;
        };
        let mut registrations = self.registrations.lock().await;
        for name in names {
            if let Some(registration) = registrations.remove(&name) {
                let _ = registration.disposable.dispose();
            }
        }
        true
    }

    fn tool_list_updated(&self, reason: &str, server_name: &str) {
        self.events.publish(DomainEvent::new(
            "tool.list.updated",
            Map::from_iter([
                ("reason".into(), Value::String(reason.into())),
                ("serverName".into(), Value::String(server_name.into())),
            ]),
        ));
    }
}

impl Drop for AgentMcpService {
    fn drop(&mut self) {
        let _ = self.status_subscription.dispose();
    }
}
