use serde::{Deserialize, Serialize};

use crate::tool::ToolInputDisplay;

// Original:
//   packages/agent-core-v2/src/session/approval/approval.ts
//   ApprovalRequest
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub tool_name: String,
    pub action: String,
    pub display: ToolInputDisplay,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approved,
    Rejected,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalScope {
    Session,
}

// Original: approval.ts, ApprovalResponse.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResponse {
    pub decision: ApprovalDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ApprovalScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_label: Option<String>,
}

#[cfg(test)]
mod tests {
    use kimi_code_protocol::CommandLanguage;
    use serde_json::json;

    use super::*;

    #[test]
    fn request_preserves_optional_camel_case_fields_and_display_shape() {
        let request = ApprovalRequest {
            id: Some("approval-1".into()),
            session_id: Some("session-1".into()),
            agent_id: Some("agent-1".into()),
            turn_id: Some(2.5),
            tool_call_id: Some("call-1".into()),
            tool_name: "Bash".into(),
            action: "run command".into(),
            display: ToolInputDisplay::Command {
                command: "git status".into(),
                cwd: None,
                description: None,
                language: Some(CommandLanguage::Bash),
            },
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["sessionId"], "session-1");
        assert_eq!(value["turnId"], 2.5);
        assert_eq!(value["toolCallId"], "call-1");
        assert_eq!(value["display"]["kind"], "command");
        assert_eq!(
            serde_json::from_value::<ApprovalRequest>(value).unwrap(),
            request
        );
    }

    #[test]
    fn response_preserves_decision_scope_and_selected_label() {
        let response = ApprovalResponse {
            decision: ApprovalDecision::Approved,
            scope: Some(ApprovalScope::Session),
            feedback: Some("looks safe".into()),
            selected_label: Some("Always allow".into()),
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(
            value,
            json!({
                "decision": "approved",
                "scope": "session",
                "feedback": "looks safe",
                "selectedLabel": "Always allow"
            })
        );
        assert_eq!(
            serde_json::from_value::<ApprovalResponse>(value).unwrap(),
            response
        );
    }

    #[test]
    fn optional_response_fields_are_absent_not_null() {
        assert_eq!(
            serde_json::to_value(ApprovalResponse {
                decision: ApprovalDecision::Cancelled,
                scope: None,
                feedback: None,
                selected_label: None,
            })
            .unwrap(),
            json!({"decision": "cancelled"})
        );
    }
}
