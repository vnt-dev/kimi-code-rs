//! Read-only folded agent activity projection contract.
//!
//! Original: `packages/agent-core-v2/src/agent/activityView/activityView.ts`.

use std::{ops::Deref, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{
    _base::di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposeResult},
    },
    agent::{context_memory::PromptOrigin, loop_::TurnEndReason},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    Running,
    Streaming,
    ToolCall,
    Retrying,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStream {
    Assistant,
    Thinking,
    ToolCall,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRef {
    pub approval_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub since: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRef {
    pub tool_call_id: String,
    pub name: String,
    pub since: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRetryState {
    pub failed_attempt: u64,
    pub next_attempt: u64,
    pub max_attempts: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub delay_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityEndingReason {
    Aborted,
    MaxSteps,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTurnState {
    pub turn_id: crate::agent::TurnId,
    pub origin: PromptOrigin,
    pub phase: TurnPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<ActivityStream>,
    pub step: crate::agent::StepId,
    pub ending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ending_reason: Option<ActivityEndingReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<ActivityRetryState>,
    pub pending_approvals: Vec<ApprovalRef>,
    pub active_tool_calls: Vec<ToolCallRef>,
    pub since: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLastTurnState {
    pub turn_id: crate::agent::TurnId,
    pub reason: TurnEndReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    pub at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundRef {
    pub kind: String,
    pub id: String,
    pub since: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityViewLifecycle {
    Ready,
    Disposed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityState {
    pub lifecycle: ActivityViewLifecycle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<ActivityTurnState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<ActivityLastTurnState>,
    pub background: Vec<BackgroundRef>,
}

impl Default for AgentActivityState {
    fn default() -> Self {
        Self {
            lifecycle: ActivityViewLifecycle::Ready,
            turn: None,
            last_turn: None,
            background: Vec::new(),
        }
    }
}

pub trait AgentActivityViewContract: Disposable + Send + Sync {
    fn state(&self) -> AgentActivityState;
}

#[derive(Clone)]
pub struct AgentActivityViewHandle(pub Arc<dyn AgentActivityViewContract>);

impl Deref for AgentActivityViewHandle {
    type Target = dyn AgentActivityViewContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentActivityViewHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_ACTIVITY_VIEW_ID: ServiceIdentifier<AgentActivityViewHandle> =
    ServiceIdentifier::new("agentActivityView");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn contract_types_preserve_source_wire_shape() {
        assert_eq!(AGENT_ACTIVITY_VIEW_ID.to_string(), "agentActivityView");
        assert_eq!(
            serde_json::to_value(AgentActivityState::default()).unwrap(),
            json!({"lifecycle": "ready", "background": []})
        );
        assert_eq!(
            serde_json::to_value(ActivityRetryState {
                failed_attempt: 1,
                next_attempt: 2,
                max_attempts: 3,
                delay_ms: 500,
                error_name: Some("ProviderError".into()),
                status_code: Some(429),
            })
            .unwrap(),
            json!({
                "failedAttempt": 1,
                "nextAttempt": 2,
                "maxAttempts": 3,
                "delayMs": 500,
                "errorName": "ProviderError",
                "statusCode": 429
            })
        );
    }
}
