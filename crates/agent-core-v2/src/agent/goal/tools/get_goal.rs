use std::{sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::{
        goal::{AGENT_GOAL_SERVICE_ID, AgentGoalServiceHandle, GoalToolResult},
        scope_context::AGENT_SCOPE_CONTEXT_ID,
        tool_registry::{ToolContributionOptions, register_tool},
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema,
    },
};

use super::goal_result_for_model;

const GET_GOAL_DESCRIPTION: &str = include_str!("get_goal.md");

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GetGoalInput {}

pub static GET_GOAL_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({"type": "object", "properties": {}, "additionalProperties": false})
            .as_object()
            .cloned()
            .expect("GetGoal schema is an object"),
    )
});

pub trait GetGoalProvider: Send + Sync {
    fn get_goal(&self) -> Result<GoalToolResult, String>;
}

impl GetGoalProvider for AgentGoalServiceHandle {
    fn get_goal(&self) -> Result<GoalToolResult, String> {
        (**self).get_goal().map_err(|error| error.to_string())
    }
}

pub struct GetGoalTool {
    goal: Arc<dyn GetGoalProvider>,
    definition: Tool,
}

impl GetGoalTool {
    pub fn new(goal: Arc<dyn GetGoalProvider>) -> Self {
        Self {
            goal,
            definition: Tool {
                name: "GetGoal".into(),
                description: GET_GOAL_DESCRIPTION.into(),
                parameters: GET_GOAL_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }

    pub fn from_goal_service(goal: AgentGoalServiceHandle) -> Self {
        Self::new(Arc::new(goal))
    }
}

#[async_trait]
impl ExecutableTool for GetGoalTool {
    type Input = GetGoalInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    // Original: get-goal.ts, GetGoalTool.resolveExecution().
    async fn resolve_execution(&self, _args: GetGoalInput) -> ToolExecution {
        let goal = Arc::clone(&self.goal);
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let goal = Arc::clone(&goal);
            Box::pin(async move {
                let result = match goal.get_goal() {
                    Ok(result) => goal_result_for_model(result),
                    Err(error) => return ExecutableToolResult::error(error),
                };
                ExecutableToolResult::success(
                    serde_json::to_string_pretty(&result)
                        .expect("GoalResultForModel is always serializable"),
                )
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("GetGoal", execute);
        execution.description = Some("Reading the current goal".into());
        ToolExecution::Runnable(execution)
    }
}

// Original: registerTool(GetGoalTool, { when: agentId === "main" }).
pub fn register_get_goal_tool() {
    register_tool(
        Arc::new(|accessor| {
            let goal = accessor.get(AGENT_GOAL_SERVICE_ID)?;
            Ok(Arc::new(GetGoalTool::from_goal_service((*goal).clone())))
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
    use crate::{
        _base::utils::abort::AbortController,
        agent::goal::{GoalBudgetReport, GoalSnapshot, GoalStatus},
        tool::{ExecutableToolOutput, ToolExecution},
    };

    struct StubGoal(GoalToolResult);

    impl GetGoalProvider for StubGoal {
        fn get_goal(&self) -> Result<GoalToolResult, String> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn strict_empty_input_returns_a_pretty_model_visible_goal() {
        assert!(serde_json::from_value::<GetGoalInput>(json!({})).is_ok());
        assert!(serde_json::from_value::<GetGoalInput>(json!({"extra": true})).is_err());
        let goal = GoalToolResult {
            goal: Some(GoalSnapshot {
                goal_id: "hidden".into(),
                objective: "ship".into(),
                completion_criterion: None,
                status: GoalStatus::Active,
                turns_used: 0,
                tokens_used: 0,
                wall_clock_ms: 0,
                budget: GoalBudgetReport {
                    token_budget: None,
                    turn_budget: None,
                    wall_clock_budget_ms: None,
                    remaining_tokens: None,
                    remaining_turns: None,
                    remaining_wall_clock_ms: None,
                    token_budget_reached: false,
                    turn_budget_reached: false,
                    wall_clock_budget_reached: false,
                    over_budget: false,
                },
                terminal_reason: None,
            }),
        };
        let tool = GetGoalTool::new(Arc::new(StubGoal(goal)));
        let ToolExecution::Runnable(execution) = tool.resolve_execution(GetGoalInput {}).await
        else {
            panic!("GetGoal must be runnable");
        };
        let result = execution
            .execute(ExecutableToolContext {
                turn_id: crate::agent::TurnId::new(1),
                tool_call_id: "call".into(),
                trace: None,
                metadata: None,
                signal: AbortController::new().signal(),
                on_update: None,
                on_foreground_task_start: None,
            })
            .await;
        assert_eq!(
            execution.description.as_deref(),
            Some("Reading the current goal")
        );
        let ExecutableToolOutput::Text(output) = result.output else {
            panic!("GetGoal must return text");
        };
        assert!(output.contains("\n  \"goal\": {"));
        assert!(output.contains("\"objective\": \"ship\""));
        assert!(!output.contains("hidden"));
    }
}
