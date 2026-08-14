use std::{sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::{
        goal::{
            AGENT_GOAL_SERVICE_ID, AgentGoalServiceHandle, GoalActor, GoalReasonInput,
            GoalSnapshot, GoalToolResult, ResumeGoalInput,
        },
        scope_context::AGENT_SCOPE_CONTEXT_ID,
        tool_registry::{ToolContributionOptions, register_tool},
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema,
    },
};

use super::{build_goal_blocked_reason_prompt, build_goal_completion_summary_prompt};

const UPDATE_GOAL_DESCRIPTION: &str = include_str!("update_goal.md");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateGoalStatus {
    Active,
    Complete,
    Blocked,
}

impl UpdateGoalStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "complete" => Some(Self::Complete),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Complete => "complete",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpdateGoalInput {
    pub status: UpdateGoalStatus,
}

impl<'de> Deserialize<'de> for UpdateGoalInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_update_goal_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_update_goal_input(value: &Value) -> Result<UpdateGoalInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "UpdateGoal input must be an object".to_owned())?;
    if object.len() != 1 || !object.contains_key("status") {
        return Err("UpdateGoal input must contain only status".into());
    }
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .and_then(UpdateGoalStatus::parse)
        .ok_or_else(|| "status must be active, complete, or blocked".to_owned())?;
    Ok(UpdateGoalInput { status })
}

pub static UPDATE_GOAL_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
    json!({"type":"object","properties":{"status":{"type":"string","enum":["active","complete","blocked"],"description":"The lifecycle status to set for the current goal. Use blocked only after the blocked audit threshold."}},"required":["status"],"additionalProperties":false})
    .as_object().cloned().expect("UpdateGoal schema is an object")
)
});

pub trait UpdateGoalProvider: Send + Sync {
    fn get_goal(&self) -> Result<GoalToolResult, String>;
    fn is_goal_tool_target(&self, turn_id: crate::agent::TurnId, goal_id: &str) -> bool;
    fn resume_goal(&self) -> BoxFuture<'static, Result<GoalSnapshot, String>>;
    fn mark_complete(&self) -> BoxFuture<'static, Result<Option<GoalSnapshot>, String>>;
    fn mark_blocked(&self) -> BoxFuture<'static, Result<Option<GoalSnapshot>, String>>;
}

impl UpdateGoalProvider for AgentGoalServiceHandle {
    fn get_goal(&self) -> Result<GoalToolResult, String> {
        (**self).get_goal().map_err(|error| error.to_string())
    }
    fn is_goal_tool_target(&self, turn_id: crate::agent::TurnId, goal_id: &str) -> bool {
        (**self)
            .is_goal_tool_target(turn_id, goal_id)
            .unwrap_or(false)
    }
    fn resume_goal(&self) -> BoxFuture<'static, Result<GoalSnapshot, String>> {
        let service = self.clone();
        Box::pin(async move {
            service
                .0
                .resume_goal(Some(ResumeGoalInput::default()), Some(GoalActor::Model))
                .await
                .map_err(|error| error.to_string())
        })
    }
    fn mark_complete(&self) -> BoxFuture<'static, Result<Option<GoalSnapshot>, String>> {
        let service = self.clone();
        Box::pin(async move {
            service
                .0
                .mark_complete(Some(GoalReasonInput::default()), Some(GoalActor::Model))
                .await
                .map_err(|error| error.to_string())
        })
    }
    fn mark_blocked(&self) -> BoxFuture<'static, Result<Option<GoalSnapshot>, String>> {
        let service = self.clone();
        Box::pin(async move {
            service
                .0
                .mark_blocked(Some(GoalReasonInput::default()), Some(GoalActor::Model))
                .await
                .map_err(|error| error.to_string())
        })
    }
}

pub struct UpdateGoalTool {
    goal: Arc<dyn UpdateGoalProvider>,
    definition: Tool,
}

impl UpdateGoalTool {
    pub fn new(goal: Arc<dyn UpdateGoalProvider>) -> Self {
        Self {
            goal,
            definition: Tool {
                name: "UpdateGoal".into(),
                description: UPDATE_GOAL_DESCRIPTION.into(),
                parameters: UPDATE_GOAL_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for UpdateGoalTool {
    type Input = UpdateGoalInput;
    fn tool(&self) -> &Tool {
        &self.definition
    }
    async fn resolve_execution(&self, args: UpdateGoalInput) -> ToolExecution {
        let current = self.goal.get_goal().ok().and_then(|result| result.goal);
        let goal_is_active = current
            .as_ref()
            .is_some_and(|goal| matches!(goal.status, crate::agent::goal::GoalStatus::Active));
        let goal = Arc::clone(&self.goal);
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let goal = Arc::clone(&goal);
            let current = current.clone();
            Box::pin(async move {
                let at_execution = match goal.get_goal() {
                    Ok(result) => result.goal,
                    Err(error) => return ExecutableToolResult::error(error),
                };
                let Some(at_execution) = at_execution else {
                    return ExecutableToolResult::success(missing_goal_output(args.status));
                };
                if current
                    .as_ref()
                    .is_none_or(|at_resolution| at_resolution.goal_id != at_execution.goal_id)
                    && !goal.is_goal_tool_target(context.turn_id, &at_execution.goal_id)
                {
                    return ExecutableToolResult::success(changed_goal_output(args.status));
                }
                match args.status {
                    UpdateGoalStatus::Active => match goal.resume_goal().await {
                        Ok(_) => ExecutableToolResult::success("Goal resumed."),
                        Err(error) => ExecutableToolResult::error(error),
                    },
                    UpdateGoalStatus::Complete => match goal.mark_complete().await {
                        Ok(Some(goal)) => stop_result(build_goal_completion_summary_prompt(&goal)),
                        Ok(None) => {
                            ExecutableToolResult::success("Goal not completed: no active goal.")
                        }
                        Err(error) => ExecutableToolResult::error(error),
                    },
                    UpdateGoalStatus::Blocked => match goal.mark_blocked().await {
                        Ok(Some(goal)) => stop_result(build_goal_blocked_reason_prompt(&goal)),
                        Ok(None) => {
                            ExecutableToolResult::success("Goal not blocked: no active goal.")
                        }
                        Err(error) => ExecutableToolResult::error(error),
                    },
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("UpdateGoal", execute);
        execution.description = Some(format!("Setting goal status: {}", args.status.as_str()));
        execution.stop_batch_after_this =
            (args.status != UpdateGoalStatus::Active && goal_is_active).then_some(true);
        ToolExecution::Runnable(execution)
    }
}

fn stop_result(output: String) -> ExecutableToolResult {
    let mut result = ExecutableToolResult::success(output);
    result.stop_turn = Some(true);
    result
}
fn missing_goal_output(status: UpdateGoalStatus) -> &'static str {
    match status {
        UpdateGoalStatus::Active => "Goal not resumed: no current goal.",
        UpdateGoalStatus::Complete => "Goal not completed: no active goal.",
        UpdateGoalStatus::Blocked => "Goal not blocked: no active goal.",
    }
}
fn changed_goal_output(status: UpdateGoalStatus) -> &'static str {
    match status {
        UpdateGoalStatus::Active => "Goal not resumed: the current goal changed.",
        UpdateGoalStatus::Complete => "Goal not completed: the current goal changed.",
        UpdateGoalStatus::Blocked => "Goal not blocked: the current goal changed.",
    }
}

pub fn register_update_goal_tool() {
    register_tool(
        Arc::new(|accessor| {
            let goal = accessor.get(AGENT_GOAL_SERVICE_ID)?;
            Ok(Arc::new(UpdateGoalTool::new(Arc::new((*goal).clone()))))
        }),
        ToolContributionOptions {
            source: None,
            when: Some(Arc::new(|accessor| {
                accessor
                    .get(AGENT_SCOPE_CONTEXT_ID)
                    .is_ok_and(|context| context.agent_id == "main")
            })),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_the_source_status_enum() {
        assert!(parse_update_goal_input(&json!({"status":"active"})).is_ok());
        assert!(parse_update_goal_input(&json!({"status":"paused"})).is_err());
        assert!(parse_update_goal_input(&json!({"status":"blocked","x":1})).is_err());
        assert_eq!(
            missing_goal_output(UpdateGoalStatus::Complete),
            "Goal not completed: no active goal."
        );
    }
}
