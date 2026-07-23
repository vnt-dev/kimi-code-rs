use serde::{Deserialize, Serialize};

use crate::session::approval::ApprovalResponse;

// Original:
//   packages/agent-core-v2/src/agent/permissionRules/permissionRules.ts
//   PermissionRuleDecision / PermissionRuleScope / PermissionRule
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionRuleDecision {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PermissionRuleScope {
    #[serde(rename = "turn-override")]
    TurnOverride,
    #[serde(rename = "session-runtime")]
    SessionRuntime,
    #[serde(rename = "project")]
    Project,
    #[serde(rename = "user")]
    User,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionRule {
    pub decision: PermissionRuleDecision,
    pub scope: PermissionRuleScope,
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// Original: permissionRules.ts, PermissionApprovalResultRecord.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionApprovalResultRecord {
    pub turn_id: f64,
    pub tool_call_id: String,
    pub tool_name: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_approval_rule: Option<String>,
    pub result: ApprovalResponse,
}
