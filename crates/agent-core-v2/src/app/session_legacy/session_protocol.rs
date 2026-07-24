//! Session v1 wire DTOs.
//! Original: `packages/agent-core-v2/src/app/sessionLegacy/sessionProtocol.ts`.
use crate::_base::utils::iso_date_time::IsoDateTime;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionWarning {
    pub code: String,
    pub message: String,
    pub severity: SessionWarningSeverity,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionWarningSeverity {
    Info,
    Warning,
    Error,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionWarningsResponse {
    pub warnings: Vec<SessionWarning>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptPermissionMode {
    Manual,
    Yolo,
    Auto,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalControl {
    Pause,
    Resume,
    Cancel,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    pub thinking: Option<String>,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleMatcherKind {
    CommandPrefix,
    PathGlob,
    ExactInput,
    Always,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LegacyPermissionRuleMatcher {
    pub kind: PermissionRuleMatcherKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LegacyPermissionRuleCreatedBy {
    User,
    Agent,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LegacyPermissionRule {
    pub id: String,
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<LegacyPermissionRuleMatcher>,
    pub decision: LegacyPermissionRuleDecision,
    pub created_at: IsoDateTime,
    pub created_by: LegacyPermissionRuleCreatedBy,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LegacyPermissionRuleDecision {
    #[serde(rename = "approved")]
    Approved,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateSessionProfileRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<SessionAgentConfigPartial>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_rules: Option<Vec<LegacyPermissionRule>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    pub context_usage: f64,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_and_status_preserve_legacy_underscore_wire_fields() {
        let request = UpdateSessionProfileRequest {
            title: Some("title".into()),
            metadata: None,
            agent_config: Some(SessionAgentConfigPartial {
                plan_mode: Some(true),
                ..Default::default()
            }),
            permission_rules: None,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"title":"title","agent_config":{"plan_mode":true}})
        );
        let status = SessionStatusResponse {
            busy: false,
            model: None,
            thinking_level: "off".into(),
            permission: "manual".into(),
            plan_mode: false,
            swarm_mode: false,
            context_tokens: 0,
            max_context_tokens: 0,
            context_usage: 0.0,
        };
        assert!(
            serde_json::to_value(status)
                .unwrap()
                .get("context_usage")
                .is_some()
        );
    }
}
