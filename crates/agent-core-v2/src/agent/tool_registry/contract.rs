use std::{ops::Deref, sync::Arc};

use crate::{
    _base::di::{instantiation::ServiceIdentifier, lifecycle::DisposableHandle},
    tool::{ErasedExecutableTool, ToolInfo, ToolSource},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ToolRegistrationOptions {
    pub source: Option<ToolSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolReference {
    pub name: String,
    pub source: ToolSource,
}

pub trait AgentToolRegistryServiceContract: Send + Sync {
    fn register(
        &self,
        tool: Arc<dyn ErasedExecutableTool>,
        options: ToolRegistrationOptions,
    ) -> DisposableHandle;
    fn list(&self) -> Vec<ToolInfo>;
    fn list_references(&self) -> Vec<ToolReference>;
    fn resolve(&self, name: &str) -> Option<Arc<dyn ErasedExecutableTool>>;
}

#[derive(Clone)]
pub struct AgentToolRegistryServiceHandle(pub Arc<dyn AgentToolRegistryServiceContract>);

impl Deref for AgentToolRegistryServiceHandle {
    type Target = dyn AgentToolRegistryServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_TOOL_REGISTRY_SERVICE_ID: ServiceIdentifier<AgentToolRegistryServiceHandle> =
    ServiceIdentifier::new("agentToolRegistryService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_and_registration_defaults_match_source() {
        assert_eq!(
            AGENT_TOOL_REGISTRY_SERVICE_ID.to_string(),
            "agentToolRegistryService"
        );
        assert_eq!(ToolRegistrationOptions::default().source, None);
        assert_eq!(
            ToolReference {
                name: "Read".into(),
                source: ToolSource::Builtin,
            },
            ToolReference {
                name: "Read".into(),
                source: ToolSource::Builtin,
            }
        );
    }
}
