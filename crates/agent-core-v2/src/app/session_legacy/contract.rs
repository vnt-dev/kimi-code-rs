//! v1 session adapter contract.
//! Original: `packages/agent-core-v2/src/app/sessionLegacy/sessionLegacy.ts`.
use super::{SessionStatusResponse, UpdateSessionProfileRequest};
use crate::{_base::di::instantiation::ServiceIdentifier, agent::goal::types::GoalSnapshot};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{error::Error, ops::Deref, sync::Arc};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWireFields {
    pub id: String,
    pub workspace_id: String,
    pub root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
    pub archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<Map<String, Value>>,
}
pub type SessionLegacyResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
#[async_trait]
pub trait SessionLegacyServiceContract: Send + Sync {
    async fn update_profile(
        &self,
        session_id: &str,
        body: UpdateSessionProfileRequest,
    ) -> SessionLegacyResult<SessionWireFields>;
    async fn status(&self, session_id: &str) -> SessionLegacyResult<SessionStatusResponse>;
    async fn goal(&self, session_id: &str) -> SessionLegacyResult<Option<GoalSnapshot>>;
}
#[derive(Clone)]
pub struct SessionLegacyServiceHandle(pub Arc<dyn SessionLegacyServiceContract>);
impl Deref for SessionLegacyServiceHandle {
    type Target = dyn SessionLegacyServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const SESSION_LEGACY_SERVICE_ID: ServiceIdentifier<SessionLegacyServiceHandle> =
    ServiceIdentifier::new("sessionLegacyService");
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_wire_fields_use_v1_camel_case() {
        let fields = SessionWireFields {
            id: "s".into(),
            workspace_id: "w".into(),
            root: "/r".into(),
            title: None,
            last_prompt: None,
            created_at: 1.0,
            updated_at: 2.0,
            archived: false,
            custom: None,
        };
        assert_eq!(
            serde_json::to_value(fields).unwrap(),
            serde_json::json!({"id":"s","workspaceId":"w","root":"/r","createdAt":1.0,"updatedAt":2.0,"archived":false})
        );
        assert_eq!(
            SESSION_LEGACY_SERVICE_ID.to_string(),
            "sessionLegacyService"
        );
    }
}
