//! Session-scoped merged agent profile catalog contract.
//!
//! Original: `packages/agent-core-v2/src/session/sessionAgentProfileCatalog/sessionAgentProfileCatalog.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::{di::instantiation::ServiceIdentifier, event::Event},
    app::agent_profile_catalog::{AgentProfile, MissingDefaultAgentProfile},
};

pub type SessionAgentProfileCatalogError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait SessionAgentProfileCatalogContract: Send + Sync {
    async fn ready(&self) -> Result<(), SessionAgentProfileCatalogError>;
    fn on_did_change(&self) -> Event<String>;
    fn get(&self, name: &str) -> Option<Arc<AgentProfile>>;
    fn get_default(&self) -> Result<Arc<AgentProfile>, MissingDefaultAgentProfile>;
    fn list(&self) -> Vec<Arc<AgentProfile>>;
    async fn load(&self) -> Result<(), SessionAgentProfileCatalogError>;
    async fn reload(&self) -> Result<(), SessionAgentProfileCatalogError>;
}

#[derive(Clone)]
pub struct SessionAgentProfileCatalogHandle(pub Arc<dyn SessionAgentProfileCatalogContract>);

impl Deref for SessionAgentProfileCatalogHandle {
    type Target = dyn SessionAgentProfileCatalogContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_AGENT_PROFILE_CATALOG_ID: ServiceIdentifier<SessionAgentProfileCatalogHandle> =
    ServiceIdentifier::new("sessionAgentProfileCatalog");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_source_decorator() {
        assert_eq!(
            SESSION_AGENT_PROFILE_CATALOG_ID.to_string(),
            "sessionAgentProfileCatalog"
        );
    }
}
