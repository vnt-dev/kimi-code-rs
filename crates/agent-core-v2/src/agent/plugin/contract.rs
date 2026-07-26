//! Agent-scope plugin integration contract.
//!
//! Original: `packages/agent-core-v2/src/agent/plugin/agentPlugin.ts`.

use std::{ops::Deref, sync::Arc};

use crate::_base::di::{
    instantiation::ServiceIdentifier,
    lifecycle::{Disposable, DisposeResult},
};

pub trait AgentPluginServiceContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct AgentPluginServiceHandle(pub Arc<dyn AgentPluginServiceContract>);

impl Deref for AgentPluginServiceHandle {
    type Target = dyn AgentPluginServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentPluginServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_PLUGIN_SERVICE_ID: ServiceIdentifier<AgentPluginServiceHandle> =
    ServiceIdentifier::new("agentPluginService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_matches_source_decorator() {
        assert_eq!(AGENT_PLUGIN_SERVICE_ID.to_string(), "agentPluginService");
    }
}
