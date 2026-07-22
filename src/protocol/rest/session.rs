use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::protocol::validation::{literal_true, non_empty};
use crate::protocol::{
    CursorQuery, GoalSnapshot, Message, PageResponse, Session, SessionChildCreate, SessionCreate,
    SessionFork, SessionUpdate,
};

pub type CreateSessionRequest = SessionCreate;
pub type CreateSessionResponse = Session;
pub type GetSessionResponse = Session;
pub type GetSessionProfileResponse = Session;
pub type UpdateSessionProfileRequest = SessionUpdate;
pub type UpdateSessionProfileResponse = Session;
pub type UpdateSessionMetaRequest = UpdateSessionProfileRequest;
pub type UpdateSessionMetaResponse = UpdateSessionProfileResponse;
pub type UpdateSessionRequest = SessionUpdate;
pub type UpdateSessionResponse = Session;
pub type ForkSessionRequest = SessionFork;
pub type ForkSessionResponse = Session;
pub type CreateSessionChildRequest = SessionChildCreate;
pub type CreateSessionChildResponse = Session;
pub type ListSessionChildrenResponse = PageResponse<Session>;
pub type RestoreSessionResponse = Session;
pub type GetSessionGoalResponse = Option<GoalSnapshot>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListSessionsQuery {
    #[serde(flatten)]
    pub cursor: CursorQuery,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "boolean_query"
    )]
    pub busy: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "boolean_query"
    )]
    pub include_archive: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "boolean_query"
    )]
    pub archived_only: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "boolean_query"
    )]
    pub exclude_empty: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListSessionChildrenQuery {
    #[serde(flatten)]
    pub cursor: CursorQuery,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "boolean_query"
    )]
    pub busy: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "boolean_query"
    )]
    pub include_archive: Option<bool>,
}

fn boolean_query<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Bool(value) => Ok(Some(value)),
        Value::String(value) if value == "true" || value == "1" => Ok(Some(true)),
        Value::String(value) if value == "false" || value == "0" => Ok(Some(false)),
        Value::Number(value) if value.as_i64() == Some(1) => Ok(Some(true)),
        Value::Number(value) if value.as_i64() == Some(0) => Ok(Some(false)),
        _ => Err(serde::de::Error::custom("must be a boolean query value")),
    }
}

pub const MAX_SESSION_EXPORT_WEB_LOG_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportSessionParams {
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ExportSessionRequest {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_web_log"
    )]
    pub web_log: Option<String>,
}

fn deserialize_web_log<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.len() <= MAX_SESSION_EXPORT_WEB_LOG_BYTES {
        Ok(Some(value))
    } else {
        Err(serde::de::Error::custom(format!(
            "web_log must not exceed {MAX_SESSION_EXPORT_WEB_LOG_BYTES} UTF-8 bytes"
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartBtwSessionResponse {
    #[serde(deserialize_with = "non_empty")]
    pub agent_id: String,
}

fn deserialize_context_usage<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let usage = f64::deserialize(deserializer)?;
    if (0.0..=1.0).contains(&usage) {
        Ok(usage)
    } else {
        Err(serde::de::Error::custom(
            "context_usage must be between 0 and 1",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionStatusResponse {
    pub busy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub thinking_level: String,
    pub permission: String,
    pub plan_mode: bool,
    pub swarm_mode: bool,
    pub context_tokens: u64,
    pub max_context_tokens: u64,
    #[serde(deserialize_with = "deserialize_context_usage")]
    pub context_usage: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionWarningSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWarning {
    pub code: String,
    pub message: String,
    pub severity: SessionWarningSeverity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWarningsResponse {
    pub warnings: Vec<SessionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompactSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompactSessionResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoSessionRequest {
    #[serde(default = "default_undo_count", deserialize_with = "positive_count")]
    pub count: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "bounded_page_size"
    )]
    pub page_size: Option<u64>,
}

impl Default for UndoSessionRequest {
    fn default() -> Self {
        Self {
            count: default_undo_count(),
            page_size: None,
        }
    }
}

const fn default_undo_count() -> u64 {
    1
}

fn positive_count<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let count = u64::deserialize(deserializer)?;
    if count == 0 {
        Err(serde::de::Error::custom("count must be positive"))
    } else {
        Ok(count)
    }
}

fn bounded_page_size<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let size = u64::deserialize(deserializer)?;
    if (1..=100).contains(&size) {
        Ok(Some(size))
    } else {
        Err(serde::de::Error::custom(
            "page_size must be between 1 and 100",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndoSessionResponse {
    pub messages: PageResponse<Message>,
    pub status: SessionStatusResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveSessionResponse {
    #[serde(deserialize_with = "literal_true")]
    pub archived: bool,
}

#[deprecated(note = "use ArchiveSessionResponse")]
pub type DeleteSessionResponse = ArchiveSessionResponse;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAbortResponse {
    pub aborted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_rest_preserves_query_preprocessing_export_limit_and_defaults() {
        let query: ListSessionsQuery = serde_json::from_value(serde_json::json!({
            "include_archive":"false","archived_only":1,"page_size":20
        }))
        .unwrap();
        assert_eq!(query.include_archive, Some(false));
        assert_eq!(query.archived_only, Some(true));
        assert_eq!(query.cursor.page_size, Some(20));
        assert!(
            serde_json::from_value::<ListSessionsQuery>(serde_json::json!({
                "busy":"frozen"
            }))
            .is_err()
        );

        let boundary = "你".repeat(MAX_SESSION_EXPORT_WEB_LOG_BYTES / 3);
        assert!(
            serde_json::from_value::<ExportSessionRequest>(serde_json::json!({
                "web_log":boundary
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ExportSessionRequest>(serde_json::json!({
                "web_log":"a".repeat(MAX_SESSION_EXPORT_WEB_LOG_BYTES + 1)
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ExportSessionRequest>(serde_json::json!({
                "outputPath":"/tmp/export.zip"
            }))
            .is_err()
        );

        assert_eq!(UndoSessionRequest::default().count, 1);
        assert!(
            serde_json::from_value::<ArchiveSessionResponse>(serde_json::json!({
                "archived":false
            }))
            .is_err()
        );
    }
}
