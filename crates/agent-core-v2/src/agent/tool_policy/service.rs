//! Agent-scoped composed tool-activation policy.
//!
//! Original: `packages/agent-core-v2/src/agent/toolPolicy/toolPolicyService.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        lifecycle::{Disposable, DisposableStore, DisposeResult},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::{
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle},
        tool_executor::{AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle},
    },
    app::config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    session::tool_policy::{SESSION_TOOL_POLICY_ID, SessionToolPolicyHandle},
    tool::ToolSource,
};

use super::{
    GlobalToolsPolicy, TOOLS_SECTION, ToolActivationPolicy, ToolPolicyLayers, ToolsConfig,
    is_tool_active_composed,
};

pub type ToolPolicyError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait AgentToolPolicyServiceContract: Disposable + Send + Sync {
    fn is_tool_active(&self, name: &str, source: ToolSource) -> Result<bool, ToolPolicyError>;
    fn is_tool_active_for_disclosure(
        &self,
        name: &str,
        source: ToolSource,
    ) -> Result<bool, ToolPolicyError>;
    fn is_tool_active_for_profile(
        &self,
        profile: &ToolActivationPolicy,
        name: &str,
        source: ToolSource,
    ) -> bool;
    async fn set_session_disabled_tools(&self, names: Vec<String>) -> Result<(), ToolPolicyError>;
}

#[derive(Clone)]
pub struct AgentToolPolicyServiceHandle(pub Arc<dyn AgentToolPolicyServiceContract>);
impl Deref for AgentToolPolicyServiceHandle {
    type Target = dyn AgentToolPolicyServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
impl Disposable for AgentToolPolicyServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}
pub const AGENT_TOOL_POLICY_SERVICE_ID: ServiceIdentifier<AgentToolPolicyServiceHandle> =
    ServiceIdentifier::new("agentToolPolicyService");

pub struct AgentToolPolicyService {
    profile: AgentProfileServiceHandle,
    config: ConfigServiceHandle,
    session: SessionToolPolicyHandle,
    disposables: DisposableStore,
}

impl AgentToolPolicyService {
    pub fn new(
        profile: AgentProfileServiceHandle,
        config: ConfigServiceHandle,
        session: SessionToolPolicyHandle,
        executor: AgentToolExecutorServiceHandle,
    ) -> Self {
        let disposables = DisposableStore::new();
        let profile_for_guard = profile.clone();
        let config_for_guard = config.clone();
        let session_for_guard = session.clone();
        let guard = executor.register_tool_call_guard(Arc::new(move |input| {
            let profile_data = profile_for_guard.data().ok()?;
            let profile = ToolActivationPolicy {
                tools: profile_data.active_tool_names,
                disallowed_tools: profile_data.disallowed_tools,
            };
            let global = global_policy(config_for_guard.get(TOOLS_SECTION));
            let active = is_tool_active_composed(
                ToolPolicyLayers {
                    profile: &profile,
                    global: global.as_ref(),
                    session_disabled_tools: Some(&session_for_guard.disabled_tools()),
                },
                &input.name,
                input.source,
            );
            (!active).then(|| {
                format!(
                    "Tool \"{}\" is disabled by the active tool policy",
                    input.name
                )
            })
        }));
        disposables.add(guard);
        Self {
            profile,
            config,
            session,
            disposables,
        }
    }

    fn global(&self) -> Option<GlobalToolsPolicy> {
        global_policy(self.config.get(TOOLS_SECTION))
    }
}

#[async_trait]
impl AgentToolPolicyServiceContract for AgentToolPolicyService {
    fn is_tool_active(&self, name: &str, source: ToolSource) -> Result<bool, ToolPolicyError> {
        let data = self.profile.data()?;
        Ok(self.is_tool_active_for_profile(
            &ToolActivationPolicy {
                tools: data.active_tool_names,
                disallowed_tools: data.disallowed_tools,
            },
            name,
            source,
        ))
    }
    fn is_tool_active_for_disclosure(
        &self,
        name: &str,
        source: ToolSource,
    ) -> Result<bool, ToolPolicyError> {
        let data = self.profile.data()?;
        Ok(is_tool_active_composed(
            ToolPolicyLayers {
                profile: &ToolActivationPolicy {
                    tools: None,
                    disallowed_tools: data.disallowed_tools,
                },
                global: self.global().as_ref(),
                session_disabled_tools: Some(&self.session.disabled_tools()),
            },
            name,
            source,
        ))
    }
    fn is_tool_active_for_profile(
        &self,
        profile: &ToolActivationPolicy,
        name: &str,
        source: ToolSource,
    ) -> bool {
        let global = self.global();
        let session = self.session.disabled_tools();
        is_tool_active_composed(
            ToolPolicyLayers {
                profile,
                global: global.as_ref(),
                session_disabled_tools: Some(&session),
            },
            name,
            source,
        )
    }
    async fn set_session_disabled_tools(&self, names: Vec<String>) -> Result<(), ToolPolicyError> {
        let data = self.profile.data()?;
        if data.config.profile_name.is_none() {
            return Err(Box::new(std::io::Error::other(
                "Cannot set session disabled tools: agent profile is not bound",
            )));
        }
        self.session.set_disabled_tools(names).await
    }
}
impl Disposable for AgentToolPolicyService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

fn global_policy(value: Option<serde_json::Value>) -> Option<GlobalToolsPolicy> {
    serde_json::from_value::<ToolsConfig>(value?)
        .ok()
        .map(|config| GlobalToolsPolicy {
            enabled: config.enabled,
            disabled: config.disabled,
        })
}

pub fn register_agent_tool_policy_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TOOL_POLICY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let profile: AgentProfileServiceHandle =
                (*accessor.get(AGENT_PROFILE_SERVICE_ID)?).clone();
            let config: ConfigServiceHandle = (*accessor.get(CONFIG_SERVICE_ID)?).clone();
            let session: SessionToolPolicyHandle = (*accessor.get(SESSION_TOOL_POLICY_ID)?).clone();
            let executor: AgentToolExecutorServiceHandle =
                (*accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?).clone();
            let service: Arc<dyn AgentToolPolicyServiceContract> = Arc::new(
                AgentToolPolicyService::new(profile, config, session, executor),
            );
            Ok(AgentToolPolicyServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "toolPolicy",
    );
}
