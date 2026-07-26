//! Agent-facing MCP tool registration service.
//!
//! Original: `agent/mcp/mcpService.ts`.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableHandle, DisposeResult, dispose_all},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::{serialize::make_error_payload, unexpected_error::on_unexpected_error},
        event::Event,
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::{AbortError, AbortSignal},
    },
    agent::{
        mcp::{
            AGENT_MCP_SERVICE_ID, AgentMcpServiceContract, AgentMcpServiceHandle,
            MCP_DISCOVERY_MODEL, MCP_TOOL_NAME_COLLISION, MCP_TOOLS_DISCOVERED, McpOAuthService,
            McpResolvedServer, McpServerEntry, McpServerStatus, McpToolCollision,
            McpToolCollisionWith, McpToolDefinition, McpToolsDiscoveredPayload,
            ensure_mcp_errors_registered, mcp_tools_discovered, qualify_mcp_tool_name,
            tools::{McpAuthToolOptions, McpToolOptions, create_mcp_auth_tool, create_mcp_tool},
        },
        media::session_media_originals_dir,
        tool_executor::{
            AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle,
            ToolBeforeExecuteContext,
        },
        tool_registry::{
            AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle, ToolRegistrationOptions,
        },
    },
    app::{
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle},
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryServiceHandle},
    },
    hooks::HookRegisterOptions,
    session::{
        mcp::{SESSION_MCP_SERVICE_ID, SessionMcpService, SessionMcpServiceHandle},
        session_context::{SESSION_CONTEXT_ID, SessionContext},
    },
    tool::{ExecutableTool, ToolSource},
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        wire_service::WireService,
    },
};

struct ToolRegistration {
    disposable: DisposableHandle,
    server_name: String,
}

#[derive(Clone)]
struct PendingDiscovery {
    server_name: String,
    raw_tools: Vec<McpToolDefinition>,
    enabled_names: Vec<String>,
    collisions: Vec<McpToolCollision>,
}

#[derive(Default)]
struct MutableState {
    tools: HashMap<String, ToolRegistration>,
    tools_by_server: HashMap<String, Vec<String>>,
    pending_discoveries: Vec<PendingDiscovery>,
    discovery_writes_ready: bool,
}

pub struct AgentMcpService {
    session: Arc<SessionMcpService>,
    session_context: SessionContext,
    registry: AgentToolRegistryServiceHandle,
    event_bus: EventBusHandle,
    wire: Arc<WireService>,
    telemetry: TelemetryServiceHandle,
    state: Mutex<MutableState>,
    status_subscription: DisposableHandle,
    executor_subscription: DisposableHandle,
    restore_subscription: DisposableHandle,
    status_sender: mpsc::UnboundedSender<McpServerEntry>,
    status_worker: Mutex<Option<JoinHandle<()>>>,
    disposed: AtomicBool,
}

impl AgentMcpService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: Arc<SessionMcpService>,
        session_context: SessionContext,
        registry: AgentToolRegistryServiceHandle,
        event_bus: EventBusHandle,
        tool_executor: AgentToolExecutorServiceHandle,
        wire: Arc<WireService>,
        telemetry: TelemetryServiceHandle,
    ) -> Arc<Self> {
        ensure_mcp_errors_registered();
        // The source module defines these eagerly. Force both lazy Rust
        // definitions before Wire restore can encounter persisted records.
        std::sync::LazyLock::force(&MCP_DISCOVERY_MODEL);
        std::sync::LazyLock::force(&MCP_TOOLS_DISCOVERED);

        let (status_sender, mut status_receiver) = mpsc::unbounded_channel();
        let service = Arc::new_cyclic(|weak: &Weak<Self>| -> Self {
            let status_subscription = {
                let status_sender = status_sender.clone();
                session
                    .connection_manager()
                    .on_status_change()
                    .subscribe(move |entry| {
                        let _ = status_sender.send(entry.clone());
                    })
            };
            let executor_subscription = {
                let weak = weak.clone();
                tool_executor
                    .hooks()
                    .on_before_execute_tool
                    .register(
                        "mcp-wait-for-initial-load",
                        Arc::new(move |context: &mut ToolBeforeExecuteContext, next| {
                            let weak = weak.clone();
                            Box::pin(async move {
                                if let Some(service) = weak.upgrade() {
                                    service
                                        .wait_for_initial_load(Some(&context.signal))
                                        .await
                                        .map_err(|error| Box::new((*error).clone()) as BoxError)?;
                                }
                                next(context).await
                            })
                        }),
                        HookRegisterOptions::default(),
                    )
                    .expect("fixed MCP executor hook registration must be valid")
            };
            let restore_subscription = {
                let weak = weak.clone();
                wire.hooks()
                    .on_did_restore
                    .register(
                        "mcp",
                        Arc::new(move |context, next| {
                            let weak = weak.clone();
                            Box::pin(async move {
                                if let Some(service) = weak.upgrade() {
                                    service
                                        .flush_pending_discoveries()
                                        .map_err(|error| Box::new(error) as BoxError)?;
                                }
                                next(context).await
                            })
                        }),
                        HookRegisterOptions::default(),
                    )
                    .expect("fixed MCP restore hook registration must be valid")
            };
            Self {
                session,
                session_context,
                registry,
                event_bus,
                wire,
                telemetry,
                state: Mutex::new(MutableState::default()),
                status_subscription,
                executor_subscription,
                restore_subscription,
                status_sender: status_sender.clone(),
                status_worker: Mutex::new(None),
                disposed: AtomicBool::new(false),
            }
        });
        let weak = Arc::downgrade(&service);
        *service.status_worker.lock().unwrap() = Some(tokio::spawn(async move {
            while let Some(entry) = status_receiver.recv().await {
                let Some(service) = weak.upgrade() else {
                    break;
                };
                service.handle_status_change(entry).await;
            }
        }));
        let initialize = Arc::clone(&service);
        tokio::spawn(async move {
            initialize.attach_existing_tools().await;
        });
        service
    }

    async fn attach_existing_tools(self: &Arc<Self>) {
        for entry in self.list().await {
            let _ = self.status_sender.send(entry);
        }
    }

    async fn handle_status_change(self: &Arc<Self>, entry: McpServerEntry) {
        if self.disposed.load(Ordering::Acquire) {
            return;
        }
        self.publish_server_status(&entry);
        match entry.status {
            McpServerStatus::Connected => self.register_connected_server(&entry).await,
            McpServerStatus::NeedsAuth => self.register_needs_auth_server(&entry).await,
            McpServerStatus::Failed => {
                self.unregister_server(&entry.name);
                self.publish_tool_list_updated("mcp.failed", &entry.name);
            }
            McpServerStatus::Disabled | McpServerStatus::Pending => {
                if self.unregister_server(&entry.name) {
                    self.publish_tool_list_updated("mcp.disconnected", &entry.name);
                }
            }
        }
    }

    fn publish_server_status(&self, entry: &McpServerEntry) {
        self.event_bus.publish(DomainEvent::new(
            "mcp.server.status",
            Map::from_iter([(
                "server".into(),
                serde_json::to_value(entry).unwrap_or(Value::Null),
            )]),
        ));
    }

    async fn register_connected_server(self: &Arc<Self>, entry: &McpServerEntry) {
        let Some(resolved) = self.resolved(&entry.name).await else {
            return;
        };
        if self.disposed.load(Ordering::Acquire) {
            return;
        }
        let collisions = self.register_server(&entry.name, &resolved);
        self.emit_tool_collisions(&entry.name, &collisions);
        self.record_discovery(
            entry.name.clone(),
            resolved.raw_tools,
            resolved.enabled_names,
            collisions,
        );
        self.publish_tool_list_updated("mcp.connected", &entry.name);
    }

    async fn register_needs_auth_server(self: &Arc<Self>, entry: &McpServerEntry) {
        self.unregister_server(&entry.name);
        let (Some(oauth_service), Some(server_url)) = (
            self.oauth_service(),
            self.get_remote_server_url(&entry.name).await,
        ) else {
            return;
        };
        if self.disposed.load(Ordering::Acquire) {
            return;
        }
        let server_name = entry.name.clone();
        let weak = Arc::downgrade(self);
        let tool = create_mcp_auth_tool(McpAuthToolOptions {
            server_name: server_name.clone(),
            server_url,
            oauth_service,
            reconnect: Arc::new(move |signal| {
                let weak = weak.clone();
                let server_name = server_name.clone();
                Box::pin(async move {
                    weak.upgrade()
                        .ok_or_else(|| "MCP service closed".to_owned())?
                        .reconnect(&server_name, Some(&signal))
                        .await
                        .map_err(|error| error.to_string())
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
        let mut state = self.state.lock().unwrap();
        state.tools.insert(
            name.clone(),
            ToolRegistration {
                disposable,
                server_name: entry.name.clone(),
            },
        );
        state.tools_by_server.insert(entry.name.clone(), vec![name]);
        drop(state);
        self.publish_tool_list_updated("mcp.connected", &entry.name);
    }

    fn register_server(
        &self,
        server_name: &str,
        resolved: &McpResolvedServer,
    ) -> Vec<McpToolCollision> {
        self.unregister_server(server_name);
        let mut registered_names = Vec::new();
        let mut collisions = Vec::new();
        let mut seen_this_call = HashMap::<String, String>::new();
        let mut state = self.state.lock().unwrap();
        for tool in &resolved.tools {
            if !resolved.enabled_names.contains(&tool.name) {
                continue;
            }
            let qualified = qualify_mcp_tool_name(server_name, &tool.name);
            if let Some(first_tool_name) = seen_this_call.get(&qualified) {
                collisions.push(McpToolCollision {
                    qualified,
                    tool_name: tool.name.clone(),
                    collides_with: McpToolCollisionWith::SameServer {
                        tool_name: first_tool_name.clone(),
                    },
                });
                continue;
            }
            if let Some(existing) = state.tools.get(&qualified) {
                collisions.push(McpToolCollision {
                    qualified,
                    tool_name: tool.name.clone(),
                    collides_with: McpToolCollisionWith::OtherServer {
                        server_name: existing.server_name.clone(),
                    },
                });
                continue;
            }
            seen_this_call.insert(qualified.clone(), tool.name.clone());
            let disposable = self.registry.register(
                Arc::new(create_mcp_tool(
                    qualified.clone(),
                    tool.clone(),
                    Arc::clone(&resolved.client),
                    McpToolOptions {
                        originals_dir: Some(session_media_originals_dir(
                            &self.session_context.session_dir,
                        )),
                        telemetry: Some(self.telemetry.clone()),
                    },
                )),
                ToolRegistrationOptions {
                    source: Some(ToolSource::Mcp),
                },
            );
            state.tools.insert(
                qualified.clone(),
                ToolRegistration {
                    disposable,
                    server_name: server_name.to_owned(),
                },
            );
            registered_names.push(qualified);
        }
        state
            .tools_by_server
            .insert(server_name.to_owned(), registered_names);
        collisions
    }

    fn unregister_server(&self, server_name: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(names) = state.tools_by_server.remove(server_name) else {
            return false;
        };
        for name in names {
            if let Some(entry) = state.tools.remove(&name) {
                let _ = entry.disposable.dispose();
            }
        }
        true
    }

    fn record_discovery(
        &self,
        server_name: String,
        raw_tools: Vec<McpToolDefinition>,
        enabled_names: HashSet<String>,
        collisions: Vec<McpToolCollision>,
    ) {
        let mut enabled_names = enabled_names.into_iter().collect::<Vec<_>>();
        enabled_names.sort();
        let discovery = PendingDiscovery {
            server_name,
            raw_tools,
            enabled_names,
            collisions,
        };
        let mut state = self.state.lock().unwrap();
        if !state.discovery_writes_ready {
            state.pending_discoveries.push(discovery);
            return;
        }
        drop(state);
        if let Err(error) = self.write_discovery(discovery) {
            on_unexpected_error(&error);
        }
    }

    fn flush_pending_discoveries(&self) -> Result<(), DiscoveryWriteError> {
        let pending = {
            let mut state = self.state.lock().unwrap();
            state.discovery_writes_ready = true;
            std::mem::take(&mut state.pending_discoveries)
        };
        for discovery in pending {
            self.write_discovery(discovery)?;
        }
        Ok(())
    }

    fn write_discovery(&self, discovery: PendingDiscovery) -> Result<(), DiscoveryWriteError> {
        let hash = discovery_hash(
            &discovery.raw_tools,
            &discovery.enabled_names,
            &discovery.collisions,
        )?;
        let key = format!("{}\n{hash}", discovery.server_name);
        if self
            .wire
            .get_model(&MCP_DISCOVERY_MODEL)
            .seen
            .contains(&key)
        {
            return Ok(());
        }
        self.wire
            .dispatch([mcp_tools_discovered(McpToolsDiscoveredPayload {
                server_name: discovery.server_name,
                hash,
                tools: discovery.raw_tools,
                enabled_names: discovery.enabled_names,
                collisions: (!discovery.collisions.is_empty()).then_some(discovery.collisions),
            })?])?;
        Ok(())
    }

    fn emit_tool_collisions(&self, server_name: &str, collisions: &[McpToolCollision]) {
        if collisions.is_empty() {
            return;
        }
        let summary = collisions
            .iter()
            .map(|collision| match &collision.collides_with {
                McpToolCollisionWith::SameServer { tool_name } => format!(
                    "\"{}\" -> {} (collides with \"{}\" from the same server)",
                    collision.tool_name, collision.qualified, tool_name
                ),
                McpToolCollisionWith::OtherServer { server_name } => format!(
                    "\"{}\" -> {} (collides with server \"{}\")",
                    collision.tool_name, collision.qualified, server_name
                ),
            })
            .collect::<Vec<_>>()
            .join("; ");
        let plural = if collisions.len() == 1 { "" } else { "s" };
        let details = Map::from_iter([
            ("serverName".into(), Value::String(server_name.to_owned())),
            (
                "collisions".into(),
                serde_json::to_value(collisions).unwrap_or(Value::Null),
            ),
        ]);
        let payload = make_error_payload(
            MCP_TOOL_NAME_COLLISION,
            format!(
                "MCP server \"{server_name}\" registered {} tool name{plural} that collide with existing qualified names; the losing tools were dropped: {summary}",
                collisions.len()
            ),
            Some(details),
            None,
        );
        let fields = serde_json::to_value(payload)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        self.event_bus.publish(DomainEvent::new("error", fields));
    }

    fn publish_tool_list_updated(&self, reason: &str, server_name: &str) {
        self.event_bus.publish(DomainEvent::new(
            "tool.list.updated",
            Map::from_iter([
                ("reason".into(), Value::String(reason.into())),
                ("serverName".into(), Value::String(server_name.into())),
            ]),
        ));
    }
}

#[derive(Debug, thiserror::Error)]
enum DiscoveryWriteError {
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Wire(#[from] crate::wire::wire_service::WireServiceError),
}

fn discovery_hash(
    tools: &[McpToolDefinition],
    enabled_names: &[String],
    collisions: &[McpToolCollision],
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct HashInput<'a> {
        tools: &'a [McpToolDefinition],
        enabled_names: &'a [String],
        collisions: &'a [McpToolCollision],
    }

    let input = serde_json::to_vec(&HashInput {
        tools,
        enabled_names,
        collisions,
    })?;
    Ok(Sha256::digest(input)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[async_trait]
impl AgentMcpServiceContract for AgentMcpService {
    fn oauth_service(&self) -> Option<Arc<McpOAuthService>> {
        self.session.oauth_service()
    }

    async fn wait_for_initial_load(
        &self,
        signal: Option<&AbortSignal>,
    ) -> Result<(), Arc<AbortError>> {
        self.session
            .connection_manager()
            .wait_for_initial_load(signal)
            .await
    }

    async fn initial_load_duration_ms(&self) -> u128 {
        self.session
            .connection_manager()
            .initial_load_duration_ms()
            .await
    }

    async fn list(&self) -> Vec<McpServerEntry> {
        self.session.connection_manager().list().await
    }

    async fn resolved(&self, name: &str) -> Option<McpResolvedServer> {
        let (client, tools, raw_tools, enabled_names) =
            self.session.connection_manager().resolved(name).await?;
        Some(McpResolvedServer {
            client,
            tools,
            raw_tools,
            enabled_names,
        })
    }

    async fn get_remote_server_url(&self, name: &str) -> Option<String> {
        self.session
            .connection_manager()
            .get_remote_server_url(name)
            .await
    }

    async fn reconnect(&self, name: &str, signal: Option<&AbortSignal>) -> Result<(), BoxError> {
        if let Some(signal) = signal {
            signal
                .throw_if_aborted()
                .map_err(|error| Box::new((*error).clone()) as BoxError)?;
        }
        self.session
            .connection_manager()
            .reconnect(name)
            .await
            .map_err(|error| Box::new(error) as BoxError)?;
        if let Some(signal) = signal {
            signal
                .throw_if_aborted()
                .map_err(|error| Box::new((*error).clone()) as BoxError)?;
        }
        Ok(())
    }

    fn on_status_change(&self) -> Event<McpServerEntry> {
        self.session.connection_manager().on_status_change()
    }
}

impl Disposable for AgentMcpService {
    fn dispose(&self) -> DisposeResult {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let mut disposables = vec![
            self.status_subscription.clone(),
            self.executor_subscription.clone(),
            self.restore_subscription.clone(),
        ];
        if let Some(worker) = self.status_worker.lock().unwrap().take() {
            worker.abort();
        }
        let mut state = self.state.lock().unwrap();
        disposables.extend(
            std::mem::take(&mut state.tools)
                .into_values()
                .map(|entry| entry.disposable),
        );
        state.tools_by_server.clear();
        state.pending_discoveries.clear();
        drop(state);
        dispose_all(disposables)
    }
}

pub fn register_agent_mcp_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_MCP_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let SessionMcpServiceHandle(session) = (*accessor.get(SESSION_MCP_SERVICE_ID)?).clone();
            let session_context = (*accessor.get(SESSION_CONTEXT_ID)?).clone();
            let registry = (*accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?).clone();
            let event_bus = (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone();
            let tool_executor = (*accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?).clone();
            let WireServiceHandle(wire) = (*accessor.get(WIRE_SERVICE_ID)?).clone();
            let telemetry = (*accessor.get(TELEMETRY_SERVICE_ID)?).clone();
            let service = AgentMcpService::new(
                session,
                session_context,
                registry,
                event_bus,
                tool_executor,
                wire,
                telemetry,
            );
            let contract: Arc<dyn AgentMcpServiceContract> = service;
            Ok(AgentMcpServiceHandle(contract))
        })
        .disposable(),
        InstantiationType::Eager,
        "mcp",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_tool(name: &str) -> McpToolDefinition {
        McpToolDefinition {
            name: name.into(),
            description: "Query".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn discovery_hash_matches_node_json_and_sha256_order() {
        assert_eq!(
            discovery_hash(&[raw_tool("query_range")], &["query_range".into()], &[]).unwrap(),
            "4fa0893c99d834a97db177a74c4ddc5df6f4b0ab1aae1b2ab8ecdca0882e1a8d"
        );
    }

    #[test]
    fn discovery_hash_changes_with_enabled_names_and_collision_outcome() {
        let tools = [raw_tool("query_range")];
        let baseline = discovery_hash(&tools, &["query_range".into()], &[]).unwrap();
        let disabled = discovery_hash(&tools, &[], &[]).unwrap();
        let collided = discovery_hash(
            &tools,
            &["query_range".into()],
            &[McpToolCollision {
                qualified: "mcp__server__query_range".into(),
                tool_name: "query_range".into(),
                collides_with: McpToolCollisionWith::OtherServer {
                    server_name: "other".into(),
                },
            }],
        )
        .unwrap();
        assert_ne!(baseline, disabled);
        assert_ne!(baseline, collided);
    }

    #[test]
    fn status_event_omits_an_absent_error_like_typescript() {
        let entry = McpServerEntry {
            name: "local".into(),
            transport: "stdio".into(),
            status: McpServerStatus::Connected,
            tool_count: 2,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(entry).unwrap(),
            serde_json::json!({
                "name": "local",
                "transport": "stdio",
                "status": "connected",
                "toolCount": 2
            })
        );
    }
}
