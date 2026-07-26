use std::{sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::{
        goal::{
            AGENT_GOAL_SERVICE_ID, AgentGoalServiceHandle, CreateGoalInput, GoalActor,
            GoalSnapshot, GoalToolResult,
        },
        permission_mode::{AGENT_PERMISSION_MODE_SERVICE_ID, AgentPermissionModeServiceHandle},
        permission_policy::PermissionMode,
        scope_context::AGENT_SCOPE_CONTEXT_ID,
        tool_registry::{ToolContributionOptions, register_tool},
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, ToolInputDisplay, input_schema::to_input_json_schema,
    },
};

use super::goal_for_model;

const CREATE_GOAL_DESCRIPTION: &str = include_str!("create_goal.md");

#[derive(Clone, Debug, PartialEq)]
pub struct CreateGoalToolInput {
    pub objective: String,
    pub completion_criterion: Option<String>,
    pub replace: Option<bool>,
}

impl<'de> Deserialize<'de> for CreateGoalToolInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_create_goal_tool_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_create_goal_tool_input(value: &Value) -> Result<CreateGoalToolInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "CreateGoal input must be an object".to_owned())?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "objective" | "completionCriterion" | "replace"
        )
    }) {
        return Err("CreateGoal input contains an unknown property".into());
    }
    let objective = object
        .get("objective")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "objective must be a non-empty string".to_owned())?
        .to_owned();
    let completion_criterion = match object.get("completionCriterion") {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err("completionCriterion must be a string".into()),
    };
    let replace = match object.get("replace") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err("replace must be a boolean".into()),
    };
    Ok(CreateGoalToolInput {
        objective,
        completion_criterion,
        replace,
    })
}

pub static CREATE_GOAL_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(json!({"type":"object","properties":{"objective":{"type":"string","minLength":1,"description":"The objective to pursue. Must have a verifiable end state."},"completionCriterion":{"type":"string","description":"How to verify the goal is complete. Include when the user provides one."},"replace":{"type":"boolean","description":"Replace an existing goal instead of failing."}},"required":["objective"],"additionalProperties":false}).as_object().cloned().expect("CreateGoal schema is an object"))
});

pub trait CreateGoalProvider: Send + Sync {
    fn get_goal(&self) -> GoalToolResult;
    fn is_goal_tool_target(&self, turn_id: f64, goal_id: &str) -> bool;
    fn create_goal(
        &self,
        input: CreateGoalInput,
    ) -> BoxFuture<'static, Result<GoalSnapshot, String>>;
}
impl CreateGoalProvider for AgentGoalServiceHandle {
    fn get_goal(&self) -> GoalToolResult {
        (**self)
            .get_goal()
            .expect("goal tools are only resolved for supported agents")
    }
    fn is_goal_tool_target(&self, turn_id: f64, goal_id: &str) -> bool {
        (**self)
            .is_goal_tool_target(turn_id, goal_id)
            .unwrap_or(false)
    }
    fn create_goal(
        &self,
        input: CreateGoalInput,
    ) -> BoxFuture<'static, Result<GoalSnapshot, String>> {
        let service = self.clone();
        Box::pin(async move {
            service
                .0
                .create_goal(input, Some(GoalActor::Model))
                .await
                .map_err(|error| error.to_string())
        })
    }
}
pub trait CreateGoalPermissionProvider: Send + Sync {
    fn mode(&self) -> PermissionMode;
}
impl CreateGoalPermissionProvider for AgentPermissionModeServiceHandle {
    fn mode(&self) -> PermissionMode {
        (**self).mode()
    }
}

pub struct CreateGoalTool {
    goal: Arc<dyn CreateGoalProvider>,
    permission: Arc<dyn CreateGoalPermissionProvider>,
    definition: Tool,
}
impl CreateGoalTool {
    pub fn new(
        goal: Arc<dyn CreateGoalProvider>,
        permission: Arc<dyn CreateGoalPermissionProvider>,
    ) -> Self {
        Self {
            goal,
            permission,
            definition: Tool {
                name: "CreateGoal".into(),
                description: CREATE_GOAL_DESCRIPTION.into(),
                parameters: CREATE_GOAL_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for CreateGoalTool {
    type Input = CreateGoalToolInput;
    fn tool(&self) -> &Tool {
        &self.definition
    }
    async fn resolve_execution(&self, args: CreateGoalToolInput) -> ToolExecution {
        let at_resolution = self.goal.get_goal().goal;
        let display = goal_start_display(&args, self.permission.mode());
        let goal = Arc::clone(&self.goal);
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let goal = Arc::clone(&goal);
            let at_resolution = at_resolution.clone();
            let args = args.clone();
            Box::pin(async move {
                let current = goal.get_goal().goal;
                if current.as_ref().map(|goal| &goal.goal_id)
                    != at_resolution.as_ref().map(|goal| &goal.goal_id)
                    && (current.is_none()
                        || !goal.is_goal_tool_target(
                            context.turn_id as f64,
                            &current.as_ref().unwrap().goal_id,
                        ))
                {
                    return ExecutableToolResult::success(
                        "Goal not created: the current goal changed.",
                    );
                }
                match goal
                    .create_goal(CreateGoalInput {
                        objective: args.objective,
                        completion_criterion: args.completion_criterion,
                        replace: args.replace,
                    })
                    .await
                {
                    Ok(snapshot) => ExecutableToolResult::success(
                        serde_json::to_string_pretty(&json!({"goal":goal_for_model(snapshot)}))
                            .expect("goal serializes"),
                    ),
                    Err(error) => ExecutableToolResult::error(error),
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("CreateGoal", execute);
        execution.description = Some("Creating a goal".into());
        execution.display = display;
        ToolExecution::Runnable(execution)
    }
}

fn goal_start_display(
    args: &CreateGoalToolInput,
    mode: PermissionMode,
) -> Option<ToolInputDisplay> {
    match mode {
        PermissionMode::Auto => None,
        PermissionMode::Manual => Some(ToolInputDisplay::GoalStart {
            objective: args.objective.clone(),
            completion_criterion: args.completion_criterion.clone(),
            mode: kimi_code_protocol::GoalStartMode::Manual,
        }),
        PermissionMode::Yolo => Some(ToolInputDisplay::GoalStart {
            objective: args.objective.clone(),
            completion_criterion: args.completion_criterion.clone(),
            mode: kimi_code_protocol::GoalStartMode::Yolo,
        }),
    }
}
pub fn register_create_goal_tool() {
    register_tool(
        Arc::new(|accessor| {
            let goal = accessor.get(AGENT_GOAL_SERVICE_ID)?;
            let permission = accessor.get(AGENT_PERMISSION_MODE_SERVICE_ID)?;
            Ok(Arc::new(CreateGoalTool::new(
                Arc::new((*goal).clone()),
                Arc::new((*permission).clone()),
            )))
        }),
        ToolContributionOptions {
            source: None,
            when: Some(Arc::new(|accessor| {
                accessor
                    .get(AGENT_SCOPE_CONTEXT_ID)
                    .is_ok_and(|context| context.agent_id == "main")
            })),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_input_and_auto_display_match_source() {
        assert!(parse_create_goal_tool_input(&json!({"objective":"ship"})).is_ok());
        assert!(parse_create_goal_tool_input(&json!({"objective":""})).is_err());
        assert!(parse_create_goal_tool_input(&json!({"objective":"x","other":true})).is_err());
        let input = parse_create_goal_tool_input(&json!({"objective":"ship"})).unwrap();
        assert!(goal_start_display(&input, PermissionMode::Auto).is_none());
        assert!(matches!(
            goal_start_display(&input, PermissionMode::Manual),
            Some(ToolInputDisplay::GoalStart { .. })
        ));
    }
}
