use std::{sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::{
        goal::{
            AGENT_GOAL_SERVICE_ID, AgentGoalServiceHandle, GoalBudgetLimits, GoalSnapshot,
            GoalToolResult, SetGoalBudgetLimitsInput,
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

const SET_GOAL_BUDGET_DESCRIPTION: &str = include_str!("set_goal_budget.md");
const MIN_REASONABLE_TIME_BUDGET_MS: f64 = 1_000.0;
const MAX_REASONABLE_TIME_BUDGET_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetUnit {
    Turns,
    Tokens,
    Milliseconds,
    Seconds,
    Minutes,
    Hours,
}

impl BudgetUnit {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "turns" => Some(Self::Turns),
            "tokens" => Some(Self::Tokens),
            "milliseconds" => Some(Self::Milliseconds),
            "seconds" => Some(Self::Seconds),
            "minutes" => Some(Self::Minutes),
            "hours" => Some(Self::Hours),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Turns => "turns",
            Self::Tokens => "tokens",
            Self::Milliseconds => "milliseconds",
            Self::Seconds => "seconds",
            Self::Minutes => "minutes",
            Self::Hours => "hours",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SetGoalBudgetInput {
    pub value: f64,
    pub unit: BudgetUnit,
}

impl<'de> Deserialize<'de> for SetGoalBudgetInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_set_goal_budget_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_set_goal_budget_input(value: &Value) -> Result<SetGoalBudgetInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "SetGoalBudget input must be an object".to_owned())?;
    if object.len() != 2 || !object.contains_key("value") || !object.contains_key("unit") {
        return Err("SetGoalBudget input must contain only value and unit".into());
    }
    let value = object
        .get("value")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| "value must be a positive number".to_owned())?;
    let unit = object
        .get("unit")
        .and_then(Value::as_str)
        .and_then(BudgetUnit::parse)
        .ok_or_else(|| {
            "unit must be turns, tokens, milliseconds, seconds, minutes, or hours".to_owned()
        })?;
    Ok(SetGoalBudgetInput { value, unit })
}

pub static SET_GOAL_BUDGET_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "value": {"type": "number", "exclusiveMinimum": 0, "description": "The positive numeric budget value."},
                "unit": {"type": "string", "enum": ["turns", "tokens", "milliseconds", "seconds", "minutes", "hours"]}
            },
            "required": ["value", "unit"],
            "additionalProperties": false
        })
        .as_object()
        .cloned()
        .expect("SetGoalBudget schema is an object"),
    )
});

pub trait SetGoalBudgetProvider: Send + Sync {
    fn get_goal(&self) -> Result<GoalToolResult, String>;
    fn is_goal_tool_target(&self, turn_id: crate::agent::TurnId, goal_id: &str) -> bool;
    fn set_budget_limits(
        &self,
        input: SetGoalBudgetLimitsInput,
    ) -> BoxFuture<'static, Result<GoalSnapshot, String>>;
}

impl SetGoalBudgetProvider for AgentGoalServiceHandle {
    fn get_goal(&self) -> Result<GoalToolResult, String> {
        (**self).get_goal().map_err(|error| error.to_string())
    }

    fn is_goal_tool_target(&self, turn_id: crate::agent::TurnId, goal_id: &str) -> bool {
        (**self)
            .is_goal_tool_target(turn_id, goal_id)
            .unwrap_or(false)
    }

    fn set_budget_limits(
        &self,
        input: SetGoalBudgetLimitsInput,
    ) -> BoxFuture<'static, Result<GoalSnapshot, String>> {
        let service = self.clone();
        Box::pin(async move {
            service
                .0
                .set_budget_limits(input, Some(crate::agent::goal::GoalActor::Model))
                .await
                .map_err(|error| error.to_string())
        })
    }
}

pub struct SetGoalBudgetTool {
    goal: Arc<dyn SetGoalBudgetProvider>,
    definition: Tool,
}

impl SetGoalBudgetTool {
    pub fn new(goal: Arc<dyn SetGoalBudgetProvider>) -> Self {
        Self {
            goal,
            definition: Tool {
                name: "SetGoalBudget".into(),
                description: SET_GOAL_BUDGET_DESCRIPTION.into(),
                parameters: SET_GOAL_BUDGET_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for SetGoalBudgetTool {
    type Input = SetGoalBudgetInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: SetGoalBudgetInput) -> ToolExecution {
        let args = normalize_budget_input(args);
        let budget = budget_limits_from_input(args);
        let goal_at_resolution = self.goal.get_goal().ok().and_then(|result| result.goal);
        let over_budget_after_set = budget.is_some_and(|budget| {
            goal_at_resolution
                .as_ref()
                .is_some_and(|goal| would_exceed_budget(goal, budget))
        });
        let description = format!("Setting goal budget: {}", format_budget(args));
        let goal = Arc::clone(&self.goal);
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let goal = Arc::clone(&goal);
            let goal_at_resolution = goal_at_resolution.clone();
            Box::pin(async move {
                let current = match goal.get_goal() {
                    Ok(result) => result.goal,
                    Err(error) => return ExecutableToolResult::error(error),
                };
                let Some(current) = current else {
                    return ExecutableToolResult::success("Goal budget not set: no current goal.");
                };
                if goal_at_resolution
                    .as_ref()
                    .is_none_or(|resolved| resolved.goal_id != current.goal_id)
                    && !goal.is_goal_tool_target(context.turn_id, &current.goal_id)
                {
                    return ExecutableToolResult::success(
                        "Goal budget not set: the current goal changed.",
                    );
                }
                let Some(budget) = budget else {
                    return ExecutableToolResult::success(format!(
                        "Goal budget not set: {} is not a reasonable goal budget.",
                        format_budget(args)
                    ));
                };
                match goal
                    .set_budget_limits(SetGoalBudgetLimitsInput {
                        budget_limits: budget,
                    })
                    .await
                {
                    Ok(snapshot) if snapshot.budget.over_budget => {
                        let mut result = ExecutableToolResult::success(format!(
                            "Goal budget set: {}. The goal has already reached this budget and will stop now.",
                            format_budget(args)
                        ));
                        result.stop_turn = Some(true);
                        result
                    }
                    Ok(_) => ExecutableToolResult::success(format!(
                        "Goal budget set: {}.",
                        format_budget(args)
                    )),
                    Err(error) => ExecutableToolResult::error(error),
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("SetGoalBudget", execute);
        execution.description = Some(description);
        execution.stop_batch_after_this = over_budget_after_set.then_some(true);
        ToolExecution::Runnable(execution)
    }
}

pub fn normalize_budget_input(mut input: SetGoalBudgetInput) -> SetGoalBudgetInput {
    if matches!(input.unit, BudgetUnit::Turns | BudgetUnit::Tokens) {
        input.value = input.value.round().max(1.0);
    }
    input
}

pub fn budget_limits_from_input(input: SetGoalBudgetInput) -> Option<GoalBudgetLimits> {
    match input.unit {
        BudgetUnit::Turns => Some(GoalBudgetLimits {
            turn_budget: Some(input.value as u64),
            ..Default::default()
        }),
        BudgetUnit::Tokens => Some(GoalBudgetLimits {
            token_budget: Some(input.value as u64),
            ..Default::default()
        }),
        BudgetUnit::Milliseconds
        | BudgetUnit::Seconds
        | BudgetUnit::Minutes
        | BudgetUnit::Hours => {
            let milliseconds = to_milliseconds(input.value, input.unit).round();
            ((MIN_REASONABLE_TIME_BUDGET_MS..=MAX_REASONABLE_TIME_BUDGET_MS)
                .contains(&milliseconds))
            .then(|| GoalBudgetLimits {
                wall_clock_budget_ms: Some(milliseconds as u64),
                ..Default::default()
            })
        }
    }
}

pub fn would_exceed_budget(goal: &GoalSnapshot, new_limits: GoalBudgetLimits) -> bool {
    let turns = new_limits.turn_budget.or(goal.budget.turn_budget);
    let tokens = new_limits.token_budget.or(goal.budget.token_budget);
    let time = new_limits
        .wall_clock_budget_ms
        .or(goal.budget.wall_clock_budget_ms);
    turns.is_some_and(|budget| goal.turns_used >= budget)
        || tokens.is_some_and(|budget| goal.tokens_used >= budget)
        || time.is_some_and(|budget| goal.wall_clock_ms >= budget)
}

fn to_milliseconds(value: f64, unit: BudgetUnit) -> f64 {
    match unit {
        BudgetUnit::Milliseconds => value,
        BudgetUnit::Seconds => value * 1000.0,
        BudgetUnit::Minutes => value * 60.0 * 1000.0,
        BudgetUnit::Hours => value * 60.0 * 60.0 * 1000.0,
        BudgetUnit::Turns | BudgetUnit::Tokens => {
            unreachable!("only time units convert to milliseconds")
        }
    }
}

fn format_budget(input: SetGoalBudgetInput) -> String {
    let unit = input.unit.as_str();
    let singular = unit.strip_suffix('s').unwrap_or(unit);
    format!(
        "{} {}",
        javascript_number(input.value),
        if input.value == 1.0 { singular } else { unit }
    )
}

fn javascript_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

pub fn register_set_goal_budget_tool() {
    register_tool(
        Arc::new(|accessor| {
            let goal = accessor.get(AGENT_GOAL_SERVICE_ID)?;
            Ok(Arc::new(SetGoalBudgetTool::new(Arc::new((*goal).clone()))))
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
    fn validates_normalizes_and_bounds_source_budget_units() {
        assert!(parse_set_goal_budget_input(&json!({"value": 1.2, "unit": "turns"})).is_ok());
        assert!(parse_set_goal_budget_input(&json!({"value": 0, "unit": "turns"})).is_err());
        assert!(parse_set_goal_budget_input(&json!({"value": 1, "unit": "days"})).is_err());
        let turns = normalize_budget_input(SetGoalBudgetInput {
            value: 1.5,
            unit: BudgetUnit::Turns,
        });
        assert_eq!(turns.value, 2.0);
        assert_eq!(
            budget_limits_from_input(turns).unwrap().turn_budget,
            Some(2)
        );
        assert_eq!(
            budget_limits_from_input(SetGoalBudgetInput {
                value: 999.0,
                unit: BudgetUnit::Milliseconds
            }),
            None
        );
        assert_eq!(
            budget_limits_from_input(SetGoalBudgetInput {
                value: 1.0,
                unit: BudgetUnit::Seconds
            })
            .unwrap()
            .wall_clock_budget_ms,
            Some(1000)
        );
    }
}
