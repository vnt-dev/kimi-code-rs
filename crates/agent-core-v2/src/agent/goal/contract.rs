use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::_base::{
    di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposeResult},
    },
    errors::errors::Error2,
};

use super::{CreateGoalInput, GoalActor, GoalBudgetLimits, GoalSnapshot, GoalToolResult};

// Original: packages/agent-core-v2/src/agent/goal/goal.ts
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoalReasonInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeGoalInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_if_paused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_if_blocked: Option<bool>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetGoalBudgetLimitsInput {
    pub budget_limits: GoalBudgetLimits,
}

#[derive(Debug, thiserror::Error)]
pub enum GoalServiceError {
    #[error("{0}")]
    Coded(Box<Error2>),
    #[error(transparent)]
    Wire(#[from] crate::wire::wire_service::WireServiceError),
    #[error("goal service hook registration failed: {0}")]
    Hook(#[from] crate::hooks::HookRegistrationError),
    #[error("goal service operation serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("goal continuation enqueue failed: {0}")]
    Loop(crate::agent::loop_::LoopValue),
    #[error("goal reminder append failed: {0}")]
    Reminder(#[from] crate::agent::context_memory::ContextMemoryServiceError),
}

pub type GoalServiceResult<T> = Result<T, GoalServiceError>;

#[async_trait]
pub trait AgentGoalServiceContract: Disposable + Send + Sync {
    fn get_goal(&self) -> GoalServiceResult<GoalToolResult>;
    fn is_goal_tool_target(
        &self,
        turn_id: crate::agent::TurnId,
        goal_id: &str,
    ) -> GoalServiceResult<bool>;
    async fn create_goal(
        &self,
        input: CreateGoalInput,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot>;
    async fn pause_goal(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot>;
    async fn resume_goal(
        &self,
        input: Option<ResumeGoalInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot>;
    async fn cancel_goal(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot>;
    async fn set_budget_limits(
        &self,
        input: SetGoalBudgetLimitsInput,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot>;
    async fn mark_complete(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<Option<GoalSnapshot>>;
    async fn mark_blocked(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<Option<GoalSnapshot>>;
}

#[derive(Clone)]
pub struct AgentGoalServiceHandle(pub Arc<dyn AgentGoalServiceContract>);

impl Deref for AgentGoalServiceHandle {
    type Target = dyn AgentGoalServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentGoalServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_GOAL_SERVICE_ID: ServiceIdentifier<AgentGoalServiceHandle> =
    ServiceIdentifier::new("agentGoalService");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn contract_inputs_and_service_identifier_preserve_source_shape() {
        assert_eq!(AGENT_GOAL_SERVICE_ID.to_string(), "agentGoalService");
        assert_eq!(
            serde_json::to_value(ResumeGoalInput {
                reason: Some("continue".into()),
                continue_if_paused: Some(true),
                continue_if_blocked: None,
            })
            .unwrap(),
            json!({"reason": "continue", "continueIfPaused": true})
        );
    }
}
