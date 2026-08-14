//! Agent-scope progressive tool-disclosure service.
//!
//! Original: `toolSelectService.ts`.

use parking_lot::Mutex;
use std::sync::Arc;
use std::{collections::BTreeSet, ops::Deref};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        lifecycle::{Disposable, DisposableStore, DisposeResult},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract,
            AgentContextMemoryServiceHandle, ContextMessage, PromptOrigin,
        },
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle},
        tool_executor::{AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle},
        tool_policy::{AGENT_TOOL_POLICY_SERVICE_ID, AgentToolPolicyServiceHandle},
        tool_registry::{
            AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceContract,
            AgentToolRegistryServiceHandle,
        },
    },
    app::{
        event::event_bus::{EVENT_BUS_SERVICE_ID, EventBusHandle},
        flag::{FLAG_SERVICE_ID, FlagServiceHandle},
    },
    kosong::contract::{
        message::{Message, Role},
        tool::Tool,
    },
    tool::{ToolInfo, ToolSource},
};

use super::{
    DYNAMIC_TOOL_SCHEMA_VARIANT, TOOL_SELECT_FLAG_ID, collect_loaded_dynamic_tool_names,
    fold_announced_tool_names, register_tool_select_flag, render_loadable_tools_announcement,
    strip_dynamic_tool_context,
};

pub const SELECT_TOOLS_TOOL_NAME: &str = "select_tools";

#[derive(Clone, Debug, PartialEq)]
pub struct ShapedToolEntry {
    pub info: ToolInfo,
    pub deferred: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadToolsResult {
    pub to_load: Vec<String>,
    pub already_available: Vec<String>,
    pub unknown: Vec<String>,
}
pub trait AgentToolSelectServiceContract: Disposable + Send + Sync {
    fn enabled(&self) -> bool;
    fn shape_tools(&self, entries: &[ToolInfo]) -> Vec<ShapedToolEntry>;
    fn shape_history(&self, messages: &[ContextMessage]) -> Vec<ContextMessage>;
    fn load(&self, names: Vec<String>) -> LoadToolsResult;
    fn loadable_tools_announcement(&self) -> Option<String>;
}
#[derive(Clone)]
pub struct AgentToolSelectServiceHandle(pub Arc<dyn AgentToolSelectServiceContract>);
impl Deref for AgentToolSelectServiceHandle {
    type Target = dyn AgentToolSelectServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
impl Disposable for AgentToolSelectServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}
pub const AGENT_TOOL_SELECT_SERVICE_ID: ServiceIdentifier<AgentToolSelectServiceHandle> =
    ServiceIdentifier::new("agentToolSelectService");

pub struct AgentToolSelectService {
    registry: Arc<dyn AgentToolRegistryServiceContract>,
    profile: AgentProfileServiceHandle,
    policy: AgentToolPolicyServiceHandle,
    context: Arc<dyn AgentContextMemoryServiceContract>,
    flags: FlagServiceHandle,
    pending: Arc<Mutex<BTreeSet<String>>>,
    disposables: DisposableStore,
}
impl AgentToolSelectService {
    pub fn new(
        registry: Arc<dyn AgentToolRegistryServiceContract>,
        profile: AgentProfileServiceHandle,
        policy: AgentToolPolicyServiceHandle,
        context: Arc<dyn AgentContextMemoryServiceContract>,
        executor: AgentToolExecutorServiceHandle,
        flags: FlagServiceHandle,
        event_bus: EventBusHandle,
    ) -> Self {
        let pending = Arc::new(Mutex::new(BTreeSet::new()));
        let disposables = DisposableStore::new();
        let weak_pending = Arc::downgrade(&pending);
        let service_enabled_profile = profile.clone();
        let service_enabled_flags = flags.clone();
        let registry_for_unavailable = Arc::clone(&registry);
        let policy_for_unavailable = policy.clone();
        let context_for_unavailable = Arc::clone(&context);
        disposables.add(
            executor.register_unavailable_tool_describer(Arc::new(move |name| {
                describe_unavailable(
                    name,
                    &registry_for_unavailable,
                    &policy_for_unavailable,
                    &context_for_unavailable,
                    service_enabled(&service_enabled_profile, &service_enabled_flags),
                    weak_pending.upgrade().as_ref(),
                )
            })),
        );
        let pending_missing = Arc::downgrade(&pending);
        let registry_missing = Arc::clone(&registry);
        let profile_missing = profile.clone();
        let flags_missing = flags.clone();
        let context_missing = Arc::clone(&context);
        disposables.add(executor.register_missing_tool_describer(Arc::new(move |name| {
            if !service_enabled(&profile_missing, &flags_missing)
                || registry_missing.resolve(name).is_some()
            {
                return None;
            }
            let loaded = loaded_names(&context_missing, pending_missing.upgrade().as_ref());
            loaded.contains(name).then(|| format!(
                "Tool \"{name}\" was loaded but its MCP server is currently disconnected. \
                 It may become available again when the server reconnects; do not retry immediately."
            ))
        })));
        let pending_compaction = Arc::downgrade(&pending);
        disposables.add(event_bus.subscribe_type(
            "compaction.completed",
            Arc::new(move |_| {
                if let Some(pending) = pending_compaction.upgrade() {
                    pending.lock().clear();
                }
            }),
        ));
        let pending_splice = Arc::downgrade(&pending);
        let context_splice = Arc::clone(&context);
        disposables.add(event_bus.subscribe_type(
            "context.spliced",
            Arc::new(move |event| {
                if event
                    .fields
                    .get("deleteCount")
                    .and_then(|value| value.as_u64())
                    == Some(0)
                {
                    return;
                }
                let Some(pending) = pending_splice.upgrade() else {
                    return;
                };
                if pending.lock().is_empty() {
                    return;
                }
                let landed = collect_loaded_dynamic_tool_names(&context_splice.get());
                pending.lock().retain(|name| landed.contains(name));
            }),
        ));
        Self {
            registry,
            profile,
            policy,
            context,
            flags,
            pending,
            disposables,
        }
    }
    fn loadable(&self) -> Vec<String> {
        let mut names = self
            .registry
            .list()
            .into_iter()
            .filter(|entry| {
                entry.source == ToolSource::Mcp
                    && self
                        .policy
                        .is_tool_active(&entry.name, entry.source)
                        .unwrap_or(false)
            })
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}
impl AgentToolSelectServiceContract for AgentToolSelectService {
    fn enabled(&self) -> bool {
        service_enabled(&self.profile, &self.flags)
    }
    fn shape_tools(&self, entries: &[ToolInfo]) -> Vec<ShapedToolEntry> {
        let disclosure = self.enabled();
        let loaded = loaded_names(&self.context, Some(&self.pending));
        entries
            .iter()
            .filter_map(|entry| {
                let active = self
                    .policy
                    .is_tool_active(&entry.name, entry.source)
                    .unwrap_or(false)
                    || (disclosure
                        && entry.name == SELECT_TOOLS_TOOL_NAME
                        && self
                            .policy
                            .is_tool_active_for_disclosure(&entry.name, entry.source)
                            .unwrap_or(false));
                if !active || (!disclosure && entry.name == SELECT_TOOLS_TOOL_NAME) {
                    return None;
                }
                if disclosure && entry.source == ToolSource::Mcp && !loaded.contains(&entry.name) {
                    return None;
                }
                Some(ShapedToolEntry {
                    info: entry.clone(),
                    deferred: disclosure && entry.source == ToolSource::Mcp,
                })
            })
            .collect()
    }
    fn shape_history(&self, messages: &[ContextMessage]) -> Vec<ContextMessage> {
        if !self.enabled() {
            return strip_dynamic_tool_context(messages);
        }
        messages
            .iter()
            .filter_map(|message| {
                let Some(tools) = &message.message.tools else {
                    return Some(message.clone());
                };
                let kept = tools
                    .iter()
                    .filter(|tool| {
                        self.policy
                            .is_tool_active(&tool.name, ToolSource::Mcp)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if kept.len() == tools.len() {
                    return Some(message.clone());
                }
                let mut message = message.clone();
                message.message.tools = (!kept.is_empty()).then_some(kept);
                (!message.message.content.is_empty() || !message.message.tool_calls.is_empty())
                    .then_some(message)
            })
            .collect()
    }
    fn load(&self, names: Vec<String>) -> LoadToolsResult {
        let loadable = self.loadable().into_iter().collect::<BTreeSet<_>>();
        let loaded = loaded_names(&self.context, Some(&self.pending));
        let mut result = LoadToolsResult::default();
        let mut seen = BTreeSet::new();
        for name in names {
            if !seen.insert(name.clone()) {
                continue;
            }
            if loaded.contains(&name) {
                result.already_available.push(name)
            } else if loadable.contains(&name) {
                result.to_load.push(name)
            } else {
                result.unknown.push(name)
            }
        }
        if !result.to_load.is_empty() {
            result.to_load.sort();
            let tools = result
                .to_load
                .iter()
                .filter_map(|name| self.registry.resolve(name))
                .map(|tool| {
                    let definition = tool.current_tool();
                    Tool {
                        name: definition.name,
                        description: definition.description,
                        parameters: definition.parameters,
                        deferred: None,
                    }
                })
                .collect();
            let mut message = Message::new(Role::System, Vec::new(), Vec::new());
            message.tools = Some(tools);
            let _ = self.context.append(vec![ContextMessage {
                message,
                id: None,
                provider_message_id: None,
                origin: Some(PromptOrigin::Injection {
                    variant: DYNAMIC_TOOL_SCHEMA_VARIANT.into(),
                }),
                is_error: None,
                note: None,
                attachments: Vec::new(),
            }]);
            self.pending.lock().extend(result.to_load.iter().cloned());
        }
        result
    }
    fn loadable_tools_announcement(&self) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let loadable = self.loadable();
        let announced = fold_announced_tool_names(&self.context.get());
        let added = loadable
            .iter()
            .filter(|name| !announced.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        let removed = announced
            .into_iter()
            .filter(|name| !loadable.contains(name))
            .collect::<Vec<_>>();
        (!added.is_empty() || !removed.is_empty())
            .then(|| render_loadable_tools_announcement(&added, &removed))
    }
}
impl Disposable for AgentToolSelectService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}
fn service_enabled(profile: &AgentProfileServiceHandle, flags: &FlagServiceHandle) -> bool {
    profile
        .get_model_capabilities()
        .map(|capabilities| {
            capabilities.dynamically_loaded_tools == Some(true) && capabilities.tool_use
        })
        .unwrap_or(false)
        && flags.enabled(TOOL_SELECT_FLAG_ID)
}
fn loaded_names(
    context: &Arc<dyn AgentContextMemoryServiceContract>,
    pending: Option<&Arc<Mutex<BTreeSet<String>>>>,
) -> BTreeSet<String> {
    let mut names = collect_loaded_dynamic_tool_names(&context.get());
    if let Some(pending) = pending {
        names.extend(pending.lock().iter().cloned());
    }
    names
}
fn describe_unavailable(
    name: &str,
    registry: &Arc<dyn AgentToolRegistryServiceContract>,
    policy: &AgentToolPolicyServiceHandle,
    context: &Arc<dyn AgentContextMemoryServiceContract>,
    enabled: bool,
    pending: Option<&Arc<Mutex<BTreeSet<String>>>>,
) -> Option<String> {
    if !enabled {
        return None;
    }
    let loaded = loaded_names(context, pending);
    let active = policy
        .is_tool_active(name, ToolSource::Mcp)
        .unwrap_or(false);
    if loaded.contains(name) && !active {
        return Some(format!(
            "Tool \"{name}\" was loaded but is no longer active. Ask the user to enable it before calling it again."
        ));
    }
    let source = registry
        .list()
        .into_iter()
        .find(|entry| entry.name == name)?
        .source;
    (source == ToolSource::Mcp && active && !loaded.contains(name)).then(|| format!("Tool \"{name}\" is available but not loaded. Call select_tools with [\"{name}\"] first, then call the tool."))
}
pub fn register_agent_tool_select_service() {
    register_tool_select_flag();
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TOOL_SELECT_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let registry: AgentToolRegistryServiceHandle =
                (*accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?).clone();
            let profile: AgentProfileServiceHandle =
                (*accessor.get(AGENT_PROFILE_SERVICE_ID)?).clone();
            let policy: AgentToolPolicyServiceHandle =
                (*accessor.get(AGENT_TOOL_POLICY_SERVICE_ID)?).clone();
            let context: AgentContextMemoryServiceHandle =
                (*accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?).clone();
            let executor: AgentToolExecutorServiceHandle =
                (*accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?).clone();
            let flags: FlagServiceHandle = (*accessor.get(FLAG_SERVICE_ID)?).clone();
            let event_bus: EventBusHandle = (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone();
            let service: Arc<dyn AgentToolSelectServiceContract> =
                Arc::new(AgentToolSelectService::new(
                    registry.0, profile, policy, context.0, executor, flags, event_bus,
                ));
            Ok(AgentToolSelectServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "toolSelect",
    );
}
