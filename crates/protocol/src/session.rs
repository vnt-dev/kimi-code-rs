use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::rest::prompt::{GoalControl, PromptPermissionMode, PromptThinking};
use super::time::IsoDateTime;
use super::validation::{non_empty, optional_non_empty};
use super::workspace::WorkspaceId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    #[serde(deserialize_with = "deserialize_nonnegative_f64")]
    pub total_cost_usd: f64,
    pub context_tokens: u64,
    pub context_limit: u64,
    pub turn_count: u64,
}

fn deserialize_nonnegative_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = f64::deserialize(deserializer)?;
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom("must be nonnegative"))
    }
}

// Original: session.ts, emptySessionUsage()
pub fn empty_session_usage() -> SessionUsage {
    SessionUsage {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        total_cost_usd: 0.0,
        context_tokens: 0,
        context_limit: 0,
        turn_count: 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleMatcherKind {
    CommandPrefix,
    PathGlob,
    ExactInput,
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRuleMatcher {
    pub kind: PermissionRuleMatcherKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionRuleDecision {
    Approved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionRuleCreator {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    #[serde(deserialize_with = "non_empty")]
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<PermissionRuleMatcher>,
    pub decision: PermissionRuleDecision,
    pub created_at: IsoDateTime,
    pub created_by: PermissionRuleCreator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAgentConfig {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<String>>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionAgentConfigPartial {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<String>>,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMetadata {
    #[serde(deserialize_with = "non_empty")]
    pub cwd: String,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionMetadataPartial {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub cwd: Option<String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionPendingInteraction {
    None,
    Approval,
    Question,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionLastTurnReason {
    Completed,
    Cancelled,
    Failed,
}

// Original: session.ts, sessionSchema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub created_at: IsoDateTime,
    pub updated_at: IsoDateTime,
    pub busy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_turn_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_interaction: Option<SessionPendingInteraction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_turn_reason: Option<SessionLastTurnReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub current_prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    pub metadata: SessionMetadata,
    pub agent_config: SessionAgentConfig,
    pub usage: SessionUsage,
    pub permission_rules: Vec<PermissionRule>,
    pub message_count: u64,
    pub last_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionCreate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SessionMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<SessionAgentConfigPartial>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SessionMetadataPartial>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<SessionAgentConfigPartial>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_rules: Option<Vec<PermissionRule>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionFork {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
}

pub type SessionChildCreate = SessionFork;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_schema_preserves_usage_metadata_and_partial_controls() {
        let session: Session = serde_json::from_value(serde_json::json!({
            "id":"sess_1","workspace_id":"wd_kimi_0123456789ab","title":"Test",
            "created_at":"2026-06-04T18:30:00+08:00",
            "updated_at":"2026-06-04T10:35:00Z","busy":true,
            "metadata":{"cwd":"/tmp/test","custom_flag":"on"},
            "agent_config":{"model":"moonshot-v1"},"usage":{
                "input_tokens":0,"output_tokens":0,"cache_read_tokens":0,
                "cache_creation_tokens":0,"total_cost_usd":0,"context_tokens":0,
                "context_limit":0,"turn_count":0
            },"permission_rules":[],"message_count":0,"last_seq":0
        }))
        .unwrap();
        assert_eq!(session.created_at, "2026-06-04T10:30:00.000Z");
        assert_eq!(session.metadata.extra["custom_flag"], "on");
        assert_eq!(empty_session_usage().turn_count, 0);

        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "agent_config":{"thinking":"mega","permission_mode":"yolo","plan_mode":true}
        }))
        .unwrap();
        assert_eq!(
            update.agent_config.unwrap().thinking.unwrap().as_str(),
            "mega"
        );
        assert!(serde_json::from_value::<Session>(serde_json::json!({
            "id":"s","workspace_id":"bad","title":"x",
            "created_at":"2026-06-04T10:30:00Z","updated_at":"2026-06-04T10:30:00Z",
            "busy":false,"metadata":{"cwd":"/tmp"},"agent_config":{"model":"m"},
            "usage":{"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,
                "cache_creation_tokens":0,"total_cost_usd":0,"context_tokens":0,
                "context_limit":0,"turn_count":0},"permission_rules":[],"message_count":0,"last_seq":0
        }))
        .is_err());
    }
}
