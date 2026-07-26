use std::{ops::Deref, sync::Arc};

use crate::_base::di::{
    instantiation::ServiceIdentifier,
    lifecycle::{Disposable, DisposeResult},
};

pub use super::user_prompt::RenderedHookResult as RenderedExternalHookResult;

/// Marker contract for the Agent-scope observer that installs external-hook
/// listeners. Its behavior is activated by eager construction.
pub trait AgentExternalHooksServiceContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct AgentExternalHooksServiceHandle(pub Arc<dyn AgentExternalHooksServiceContract>);

impl Deref for AgentExternalHooksServiceHandle {
    type Target = dyn AgentExternalHooksServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentExternalHooksServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

// Original:
//   packages/agent-core-v2/src/agent/externalHooks/externalHooks.ts
//   IAgentExternalHooksService
pub const AGENT_EXTERNAL_HOOKS_SERVICE_ID: ServiceIdentifier<AgentExternalHooksServiceHandle> =
    ServiceIdentifier::new("agentExternalHooksService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_and_rendered_result_shape_match_source() {
        assert_eq!(
            AGENT_EXTERNAL_HOOKS_SERVICE_ID.to_string(),
            "agentExternalHooksService"
        );
        let rendered = RenderedExternalHookResult {
            event: "Stop".into(),
            message: "done".into(),
            text: "wrapped".into(),
        };
        assert_eq!(rendered.event, "Stop");
        assert_eq!(rendered.message, "done");
        assert_eq!(rendered.text, "wrapped");
    }
}
