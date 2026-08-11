use serde::{Deserialize, Serialize};

use crate::display::OptionalJsonValue;
use crate::validation::{non_empty, required_nullable};
use crate::{ApprovalRequest, Message, QuestionRequest, Session, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InFlightProgressKind {
    Stdout,
    Stderr,
    Progress,
    Status,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InFlightToolProgress {
    pub kind: InFlightProgressKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InFlightToolCall {
    #[serde(deserialize_with = "non_empty")]
    pub tool_call_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub args: OptionalJsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub display: OptionalJsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_progress: Option<InFlightToolProgress>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InFlightTurn {
    pub turn_id: crate::TurnId,
    pub assistant_text: String,
    pub thinking_text: String,
    pub running_tools: Vec<InFlightToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_prompt_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotSubagentPhase {
    Queued,
    Working,
    Suspended,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSubagent {
    #[serde(flatten)]
    pub task: Task,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_phase: Option<SnapshotSubagentPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspended_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swarm_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_in_background: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotMessages {
    pub items: Vec<Message>,
    pub has_more: bool,
}

// Original: rest/snapshot.ts, sessionSnapshotResponseSchema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshotResponse {
    pub as_of_seq: u64,
    #[serde(deserialize_with = "non_empty")]
    pub epoch: String,
    pub session: Session,
    pub messages: SnapshotMessages,
    #[serde(deserialize_with = "required_nullable")]
    pub in_flight_turn: Option<InFlightTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<SnapshotSubagent>>,
    pub pending_approvals: Vec<ApprovalRequest>,
    pub pending_questions: Vec<QuestionRequest>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_preserves_watermark_required_null_and_inflight_unknowns() {
        let task = serde_json::json!({
            "id":"t","session_id":"s","kind":"subagent","description":"worker",
            "status":"running","created_at":"2026-06-04T10:30:00Z"
        });
        let subagent: SnapshotSubagent = serde_json::from_value(serde_json::json!({
            "id":"t","session_id":"s","kind":"subagent","description":"worker",
            "status":"running","created_at":"2026-06-04T10:30:00Z",
            "subagent_phase":"working","swarm_index":0
        }))
        .unwrap();
        assert_eq!(serde_json::to_value(subagent).unwrap()["id"], task["id"]);

        let call: InFlightToolCall = serde_json::from_value(serde_json::json!({
            "tool_call_id":"c","name":"Bash","args":null,"display":null
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(call).unwrap()["args"],
            serde_json::Value::Null
        );

        // in_flight_turn is required even when its value is null.
        let missing = serde_json::json!({
            "as_of_seq":0,"epoch":"e","session":{
                "id":"s","workspace_id":"wd_kimi_0123456789ab","title":"Test",
                "created_at":"2026-06-04T10:30:00Z","updated_at":"2026-06-04T10:30:00Z",
                "busy":false,"metadata":{"cwd":"/tmp"},"agent_config":{"model":"m"},
                "usage":{"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,
                    "cache_creation_tokens":0,"total_cost_usd":0,"context_tokens":0,
                    "context_limit":0,"turn_count":0},"permission_rules":[],
                "message_count":0,"last_seq":0
            },"messages":{"items":[],"has_more":false},
            "pending_approvals":[],"pending_questions":[]
        });
        let error = serde_json::from_value::<SessionSnapshotResponse>(missing).unwrap_err();
        assert!(error.to_string().contains("in_flight_turn"));
    }
}
