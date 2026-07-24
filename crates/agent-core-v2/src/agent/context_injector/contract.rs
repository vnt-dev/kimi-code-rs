use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;

use crate::{
    _base::di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposableHandle, DisposeResult},
    },
    kosong::contract::message::ContentPart,
};

// Original:
//   packages/agent-core-v2/src/agent/contextInjector/contextInjector.ts
//   ContextInjectionContext
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextInjectionContext {
    pub injected_positions: Vec<usize>,
    pub last_injected_at: Option<usize>,
    pub is_new_turn: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextInjectionContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl From<String> for ContextInjectionContent {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<Vec<ContentPart>> for ContextInjectionContent {
    fn from(value: Vec<ContentPart>) -> Self {
        Self::Parts(value)
    }
}

pub type ContextInjectionError = Box<dyn Error + Send + Sync>;
pub type ContextInjectionResult = Result<Option<ContextInjectionContent>, ContextInjectionError>;
pub type ContextInjectionProvider = Arc<
    dyn Fn(ContextInjectionContext) -> BoxFuture<'static, ContextInjectionResult> + Send + Sync,
>;

#[async_trait]
pub trait AgentContextInjectorServiceContract: Disposable + Send + Sync {
    fn register(&self, name: String, provider: ContextInjectionProvider) -> DisposableHandle;

    async fn inject_after_compaction(&self) -> Result<(), ContextInjectionError>;
}

#[derive(Clone)]
pub struct AgentContextInjectorServiceHandle(pub Arc<dyn AgentContextInjectorServiceContract>);

impl Deref for AgentContextInjectorServiceHandle {
    type Target = dyn AgentContextInjectorServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentContextInjectorServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_CONTEXT_INJECTOR_SERVICE_ID: ServiceIdentifier<AgentContextInjectorServiceHandle> =
    ServiceIdentifier::new("agentContextInjectorService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_preserves_context_positions_and_service_identity() {
        assert_eq!(
            ContextInjectionContext {
                injected_positions: vec![1, 4],
                last_injected_at: Some(4),
                is_new_turn: true,
            },
            ContextInjectionContext {
                injected_positions: vec![1, 4],
                last_injected_at: Some(4),
                is_new_turn: true,
            }
        );
        assert_eq!(
            AGENT_CONTEXT_INJECTOR_SERVICE_ID.to_string(),
            "agentContextInjectorService"
        );
    }

    #[test]
    fn content_variants_keep_text_and_structured_parts_distinct() {
        assert_eq!(
            ContextInjectionContent::from("reminder".to_owned()),
            ContextInjectionContent::Text("reminder".into())
        );
        let parts = vec![ContentPart::Text {
            text: "structured".into(),
        }];
        assert_eq!(
            ContextInjectionContent::from(parts.clone()),
            ContextInjectionContent::Parts(parts)
        );
    }
}
