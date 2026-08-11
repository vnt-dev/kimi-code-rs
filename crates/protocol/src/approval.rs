use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::time::IsoDateTime;
use super::validation::non_empty;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approved,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalScope {
    Session,
}

// Original: approval.ts, approvalRequestSchema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    #[serde(deserialize_with = "non_empty")]
    pub approval_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::TurnId>,
    #[serde(deserialize_with = "non_empty")]
    pub tool_call_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub tool_name: String,
    pub action: String,
    pub tool_input_display: Value,
    pub created_at: IsoDateTime,
    pub expires_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub decision: ApprovalDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ApprovalScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_label: Option<String>,
}
