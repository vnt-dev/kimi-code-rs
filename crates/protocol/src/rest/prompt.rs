use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;

use crate::events::LiveUserMessage;
use crate::message::MessageContent;
use crate::time::IsoDateTime;
use crate::validation::{non_empty, optional_non_empty};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PromptThinking(String);

impl PromptThinking {
    pub fn new(value: impl Into<String>) -> Result<Self, PromptThinkingError> {
        let value = value.into();
        if value.is_empty() {
            Err(PromptThinkingError)
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for PromptThinking {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PromptThinking {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptThinkingError;

impl fmt::Display for PromptThinkingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("thinking effort must not be empty")
    }
}

impl std::error::Error for PromptThinkingError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptPermissionMode {
    Manual,
    Yolo,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalControl {
    Pause,
    Resume,
    Cancel,
}

// Original: rest/prompt.ts, promptSubmissionSchema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSubmission {
    #[serde(deserialize_with = "deserialize_content")]
    pub content: Vec<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub profile: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<PromptThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PromptPermissionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swarm_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_objective: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_control: Option<GoalControl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
}

fn deserialize_content<'de, D>(deserializer: D) -> Result<Vec<MessageContent>, D::Error>
where
    D: Deserializer<'de>,
{
    let content = Vec::<MessageContent>::deserialize(deserializer)?;
    if content.is_empty() {
        Err(serde::de::Error::custom("prompt content must not be empty"))
    } else {
        Ok(content)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptStatus {
    Running,
    Queued,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptItem {
    #[serde(deserialize_with = "non_empty")]
    pub prompt_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub user_message_id: String,
    pub status: PromptStatus,
    #[serde(deserialize_with = "deserialize_content")]
    pub content: Vec<MessageContent>,
    pub created_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptListResponse {
    #[serde(deserialize_with = "deserialize_nullable_prompt")]
    pub active: Option<PromptItem>,
    pub queued: Vec<PromptItem>,
}

fn deserialize_nullable_prompt<'de, D>(deserializer: D) -> Result<Option<PromptItem>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<PromptItem>::deserialize(deserializer)
}

pub type PromptSubmitResult = PromptItem;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSteerRequest {
    #[serde(deserialize_with = "deserialize_prompt_ids")]
    pub prompt_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSteerResult {
    #[serde(deserialize_with = "deserialize_true")]
    pub steered: bool,
    #[serde(deserialize_with = "deserialize_prompt_ids")]
    pub prompt_ids: Vec<String>,
}

fn deserialize_prompt_ids<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let ids = Vec::<String>::deserialize(deserializer)?;
    if ids.is_empty() || ids.iter().any(String::is_empty) {
        Err(serde::de::Error::custom(
            "at least one non-empty prompt ID is required",
        ))
    } else {
        Ok(ids)
    }
}

fn deserialize_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match bool::deserialize(deserializer)? {
        true => Ok(true),
        false => Err(serde::de::Error::custom("must be true")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAbortResponse {
    pub aborted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_seq: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptCompletedReason {
    Completed,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCompletedEventPayload {
    #[serde(rename = "type")]
    pub event_type: RestPromptCompletedEventType,
    pub agent_id: String,
    pub session_id: String,
    pub prompt_id: String,
    pub finished_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PromptCompletedReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestPromptCompletedEventType {
    #[serde(rename = "prompt.completed")]
    PromptCompleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptAbortedEventPayload {
    #[serde(rename = "type")]
    pub event_type: RestPromptAbortedEventType,
    pub agent_id: String,
    pub session_id: String,
    pub prompt_id: String,
    pub aborted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestPromptAbortedEventType {
    #[serde(rename = "prompt.aborted")]
    PromptAborted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSteeredEventPayload {
    #[serde(rename = "type")]
    pub event_type: RestPromptSteeredEventType,
    pub agent_id: String,
    pub session_id: String,
    pub active_prompt_id: String,
    pub prompt_ids: Vec<String>,
    pub content: Vec<MessageContent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_messages: Vec<LiveUserMessage>,
    pub steered_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestPromptSteeredEventType {
    #[serde(rename = "prompt.steered")]
    PromptSteered,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_schema_keeps_open_efforts_and_strict_control_literals() {
        let submission: PromptSubmission = serde_json::from_value(serde_json::json!({
            "content":[{"type":"text","text":"hello"}],
            "thinking":"mega","permission_mode":"yolo","disabled_tools":[]
        }))
        .unwrap();
        assert_eq!(submission.thinking.unwrap().as_str(), "mega");
        assert_eq!(submission.permission_mode, Some(PromptPermissionMode::Yolo));
        assert!(
            serde_json::from_value::<PromptSubmission>(serde_json::json!({
                "content":[],"thinking":"high"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<PromptSteerResult>(serde_json::json!({
                "steered":false,"prompt_ids":["p"]
            }))
            .is_err()
        );
    }
}
