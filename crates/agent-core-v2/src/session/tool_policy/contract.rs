//! Session-wide client-managed tool restriction contract.
//!
//! Original: `packages/agent-core-v2/src/session/sessionToolPolicy/sessionToolPolicy.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::{
    di::instantiation::ServiceIdentifier,
    event::{AsyncEvent, Event},
};

pub type SessionToolPolicyChangedEvent = AsyncEvent<()>;
pub type SessionToolPolicyError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait SessionToolPolicyContract: Send + Sync {
    async fn ready(&self) -> Result<(), SessionToolPolicyError>;
    fn on_did_change(&self) -> Event<SessionToolPolicyChangedEvent>;
    fn disabled_tools(&self) -> Vec<String>;
    async fn set_disabled_tools(&self, names: Vec<String>) -> Result<(), SessionToolPolicyError>;
}

#[derive(Clone)]
pub struct SessionToolPolicyHandle(pub Arc<dyn SessionToolPolicyContract>);

impl Deref for SessionToolPolicyHandle {
    type Target = dyn SessionToolPolicyContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_TOOL_POLICY_ID: ServiceIdentifier<SessionToolPolicyHandle> =
    ServiceIdentifier::new("sessionToolPolicy");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_and_wait_until_event_mapping_match_source() {
        assert_eq!(SESSION_TOOL_POLICY_ID.to_string(), "sessionToolPolicy");
        let _: Option<SessionToolPolicyChangedEvent> = None;
    }
}
