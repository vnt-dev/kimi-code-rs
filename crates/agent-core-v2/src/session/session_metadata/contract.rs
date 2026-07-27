use std::{collections::BTreeMap, ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::_base::{di::instantiation::ServiceIdentifier, event::Event};

pub const SESSION_META_VERSION: u64 = 2;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homedir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<AgentMetaType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_item: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMetaType {
    Main,
    Sub,
    Independent,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_custom_title: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<BTreeMap<String, AgentMeta>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetaPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_custom_title: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<BTreeMap<String, AgentMeta>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionMetadataChangedEvent {
    pub changed: Vec<String>,
}

/// Session-scoped durable session metadata.
///
/// Original: `session/sessionMetadata/sessionMetadata.ts`, `ISessionMetadata`.
pub type SessionMetadataError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait SessionMetadataContract: Send + Sync {
    async fn ready(&self) -> Result<(), SessionMetadataError>;
    fn on_did_change_metadata(&self) -> Event<SessionMetadataChangedEvent>;
    async fn read(&self) -> Result<SessionMeta, SessionMetadataError>;
    async fn update(&self, patch: SessionMetaPatch) -> Result<(), SessionMetadataError>;
    async fn set_title(&self, title: String) -> Result<(), SessionMetadataError>;
    async fn set_archived(&self, archived: bool) -> Result<(), SessionMetadataError>;
    async fn register_agent(
        &self,
        agent_id: String,
        meta: AgentMeta,
    ) -> Result<(), SessionMetadataError>;
}

#[derive(Clone)]
pub struct SessionMetadataHandle(pub Arc<dyn SessionMetadataContract>);

impl Deref for SessionMetadataHandle {
    type Target = dyn SessionMetadataContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_METADATA_ID: ServiceIdentifier<SessionMetadataHandle> =
    ServiceIdentifier::new("sessionMetadata");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_source_decorator() {
        assert_eq!(SESSION_METADATA_ID.to_string(), "sessionMetadata");
    }
}
