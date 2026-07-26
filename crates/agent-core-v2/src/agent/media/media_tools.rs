//! Agent-scope media-tools registrar contract.
//!
//! Original: `packages/agent-core-v2/src/agent/media/mediaTools.ts`.

use std::{ops::Deref, sync::Arc};

use crate::_base::di::{
    instantiation::ServiceIdentifier,
    lifecycle::{Disposable, DisposeResult},
};

/// Marker contract for the Agent-scope service that keeps media tools in sync
/// with the currently bound model.
pub trait AgentMediaToolsRegistrarContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct AgentMediaToolsRegistrarHandle(pub Arc<dyn AgentMediaToolsRegistrarContract>);

impl Deref for AgentMediaToolsRegistrarHandle {
    type Target = dyn AgentMediaToolsRegistrarContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentMediaToolsRegistrarHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_MEDIA_TOOLS_REGISTRAR_ID: ServiceIdentifier<AgentMediaToolsRegistrarHandle> =
    ServiceIdentifier::new("agentMediaToolsRegistrar");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_matches_source_decorator() {
        assert_eq!(
            AGENT_MEDIA_TOOLS_REGISTRAR_ID.to_string(),
            "agentMediaToolsRegistrar"
        );
    }
}
