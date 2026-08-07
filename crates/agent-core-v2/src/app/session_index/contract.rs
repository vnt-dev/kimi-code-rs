//! Persisted-session query model and service contract.
//!
//! Original: `packages/agent-core-v2/src/app/sessionIndex/sessionIndex.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    _base::di::instantiation::ServiceIdentifier, persistence::interface::query_store::Page,
};

pub const PARENT_SESSION_ID_KEY: &str = "parent_session_id";
pub const CHILD_SESSION_KIND_KEY: &str = "child_session_kind";
pub const CHILD_SESSION_KIND: &str = "child";

/// Backend-neutral projection of persisted session metadata.
///
/// The camel-case names are part of the query-store document format. Timestamps
/// remain finite JSON numbers, matching JavaScript's `number` representation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionListQuery {
    pub workspace_ids: Option<Vec<String>>,
    pub session_id: Option<String>,
    pub include_archived: Option<bool>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    pub child_of: Option<String>,
}

pub type SessionIndexError = Box<dyn Error + Send + Sync>;
pub type SessionIndexResult<T> = Result<T, SessionIndexError>;

#[async_trait]
pub trait SessionIndexContract: Send + Sync {
    async fn list(&self, query: SessionListQuery) -> SessionIndexResult<Page<SessionSummary>>;
    async fn get(&self, id: &str) -> SessionIndexResult<Option<SessionSummary>>;
    async fn remove(&self, id: &str) -> SessionIndexResult<()>;
    async fn count_active(&self, workspace_ids: &[String]) -> SessionIndexResult<usize>;
}

#[derive(Clone)]
pub struct SessionIndexHandle(pub Arc<dyn SessionIndexContract>);

impl Deref for SessionIndexHandle {
    type Target = dyn SessionIndexContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_INDEX_SERVICE_ID: ServiceIdentifier<SessionIndexHandle> =
    ServiceIdentifier::new("sessionIndex");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn summary_preserves_query_store_document_shape_and_custom_values() {
        let summary = SessionSummary {
            id: "session-1".into(),
            workspace_id: "wd_repo".into(),
            cwd: Some("/repo".into()),
            title: None,
            last_prompt: Some("hello".into()),
            created_at: 1.5,
            updated_at: 2.0,
            archived: false,
            custom: Some(Map::from_iter([
                (PARENT_SESSION_ID_KEY.into(), json!("parent-1")),
                (CHILD_SESSION_KIND_KEY.into(), json!(CHILD_SESSION_KIND)),
            ])),
        };

        assert_eq!(
            serde_json::to_value(summary).unwrap(),
            json!({
                "id": "session-1",
                "workspaceId": "wd_repo",
                "cwd": "/repo",
                "lastPrompt": "hello",
                "createdAt": 1.5,
                "updatedAt": 2.0,
                "archived": false,
                "custom": {
                    "parent_session_id": "parent-1",
                    "child_session_kind": "child"
                }
            })
        );
    }

    #[test]
    fn query_defaults_match_absent_typescript_options() {
        assert_eq!(
            SessionListQuery::default(),
            SessionListQuery {
                workspace_ids: None,
                session_id: None,
                include_archived: None,
                cursor: None,
                limit: None,
                child_of: None,
            }
        );
        assert_eq!(SESSION_INDEX_SERVICE_ID.to_string(), "sessionIndex");
    }
}
