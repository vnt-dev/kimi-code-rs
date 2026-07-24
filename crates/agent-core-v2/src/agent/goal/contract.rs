use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::_base::di::instantiation::ServiceIdentifier;

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

#[async_trait]
pub trait AgentGoalServiceContract: Send + Sync {
    fn get_goal(&self) -> GoalToolResult;
    fn is_goal_tool_target(&self, turn_id: f64, goal_id: &str) -> bool;
    async fn create_goal(
        &self,
        input: CreateGoalInput,
        actor: Option<GoalActor>,
    ) -> Result<GoalSnapshot, String>;
    async fn pause_goal(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> Result<GoalSnapshot, String>;
    async fn resume_goal(
        &self,
        input: Option<ResumeGoalInput>,
        actor: Option<GoalActor>,
    ) -> Result<GoalSnapshot, String>;
    async fn cancel_goal(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> Result<GoalSnapshot, String>;
    async fn set_budget_limits(
        &self,
        input: SetGoalBudgetLimitsInput,
        actor: Option<GoalActor>,
    ) -> Result<GoalSnapshot, String>;
    async fn mark_complete(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> Result<Option<GoalSnapshot>, String>;
    async fn mark_blocked(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> Result<Option<GoalSnapshot>, String>;
}

#[derive(Clone)]
pub struct AgentGoalServiceHandle(pub Arc<dyn AgentGoalServiceContract>);

impl Deref for AgentGoalServiceHandle {
    type Target = dyn AgentGoalServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
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
