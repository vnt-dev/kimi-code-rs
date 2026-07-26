//! Session agent lifecycle public contract.
//!
//! Original: `session/agentLifecycle/agentLifecycle.ts`.

use std::{collections::BTreeMap, ops::Deref, sync::Arc};

use futures_util::future::BoxFuture;

use crate::{
    _base::{
        di::{
            instantiation::ServiceIdentifier,
            lifecycle::{Disposable, DisposeResult},
            scope::ScopeHandle,
        },
        event::Event,
        lifecycle::lifecycle_machine::BoxError,
    },
    agent::{permission_policy::PermissionMode, profile::BindAgentInput},
};

pub const MAIN_AGENT_ID: &str = "main";
pub type AgentScopeHandle = ScopeHandle;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateAgentOptions {
    pub agent_id: Option<String>,
    pub binding: Option<BindAgentInput>,
    /// Provenance only: lifecycle business logic must not branch on this id.
    pub forked_from: Option<String>,
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForkAgentBinding {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub strict_thinking: Option<bool>,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForkAgentOptions {
    pub agent_id: Option<String>,
    pub binding: Option<ForkAgentBinding>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentListFilter {
    pub prefix: Option<String>,
}

pub trait AgentLifecycleServiceContract: Disposable + Send + Sync {
    fn on_did_create(&self) -> Event<AgentScopeHandle>;
    fn on_did_dispose(&self) -> Event<String>;
    fn create(
        &self,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<AgentScopeHandle, BoxError>>;
    fn fork(
        &self,
        source_agent_id: String,
        options: ForkAgentOptions,
    ) -> BoxFuture<'static, Result<AgentScopeHandle, BoxError>>;
    fn get(&self, agent_id: &str) -> Option<AgentScopeHandle>;
    fn list(&self, filter: Option<&AgentListFilter>) -> Vec<AgentScopeHandle>;
    fn broadcast_permission_mode(&self, mode: PermissionMode) -> Result<(), BoxError>;
    fn remove(&self, agent_id: String) -> BoxFuture<'static, Result<(), BoxError>>;
}

#[derive(Clone)]
pub struct AgentLifecycleServiceHandle(pub Arc<dyn AgentLifecycleServiceContract>);

impl Deref for AgentLifecycleServiceHandle {
    type Target = dyn AgentLifecycleServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentLifecycleServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_LIFECYCLE_SERVICE_ID: ServiceIdentifier<AgentLifecycleServiceHandle> =
    ServiceIdentifier::new("agentLifecycleService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_preserves_main_id_options_and_service_identity() {
        assert_eq!(MAIN_AGENT_ID, "main");
        assert_eq!(
            AGENT_LIFECYCLE_SERVICE_ID.to_string(),
            "agentLifecycleService"
        );
        assert_eq!(
            CreateAgentOptions {
                agent_id: Some("child".into()),
                forked_from: Some("source".into()),
                labels: Some(BTreeMap::from([("swarmItem".into(), "work".into())])),
                ..Default::default()
            }
            .labels
            .unwrap()["swarmItem"],
            "work"
        );
        assert_eq!(
            ForkAgentOptions {
                binding: Some(ForkAgentBinding {
                    strict_thinking: Some(true),
                    ..ForkAgentBinding::default()
                }),
                ..ForkAgentOptions::default()
            }
            .binding
            .unwrap()
            .strict_thinking,
            Some(true)
        );
    }
}
