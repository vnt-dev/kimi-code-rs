//! Agent context projection service contract.
//!
//! Original: `packages/agent-core-v2/src/agent/contextProjector/contextProjector.ts`.

use std::{ops::Deref, sync::Arc};

use crate::{
    _base::di::instantiation::ServiceIdentifier, agent::context_memory::ContextMessage,
    kosong::contract::message::Message,
};

/// Opaque identities of media that a provider rejected for the current turn.
#[derive(Clone, Debug, Default)]
pub struct MediaStripSnapshot {
    pub(crate) keys: std::collections::HashSet<String>,
}

pub type ContextProjectorResult<T> = Result<T, ContextProjectorError>;

#[derive(Debug, thiserror::Error)]
pub enum ContextProjectorError {
    #[error(
        "Tool result message content cannot be empty after removing empty text blocks: {tool_call_id:?}"
    )]
    EmptyToolResult { tool_call_id: Option<String> },
}

pub trait AgentContextProjectorServiceContract: Send + Sync {
    fn project(&self, messages: &[ContextMessage]) -> ContextProjectorResult<Vec<Message>>;
    fn project_strict(&self, messages: &[ContextMessage]) -> ContextProjectorResult<Vec<Message>>;
    fn project_media_degraded(
        &self,
        messages: &[ContextMessage],
    ) -> ContextProjectorResult<Vec<Message>>;
    fn capture_media_strip_snapshot(
        &self,
        messages: &[ContextMessage],
    ) -> ContextProjectorResult<MediaStripSnapshot>;
    fn project_media_stripped(
        &self,
        messages: &[ContextMessage],
        snapshot: Option<&MediaStripSnapshot>,
    ) -> ContextProjectorResult<Vec<Message>>;
}

#[derive(Clone)]
pub struct AgentContextProjectorServiceHandle(pub Arc<dyn AgentContextProjectorServiceContract>);

impl Deref for AgentContextProjectorServiceHandle {
    type Target = dyn AgentContextProjectorServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_CONTEXT_PROJECTOR_SERVICE_ID: ServiceIdentifier<
    AgentContextProjectorServiceHandle,
> = ServiceIdentifier::new("agentContextProjectorService");
