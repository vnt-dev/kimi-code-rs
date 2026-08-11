//! Pure goal-service calculations.
//!
//! Original: `packages/agent-core-v2/src/agent/goal/goalService.ts`,
//! `computeBudgetReport()`, `matchesGoal()`, `isGoalMutationTool()`,
//! `goalBudgetBlockReason()`, `budgetTelemetryProperties()`, and
//! `hasStepBudgetRemaining()`.

use super::{GoalBudgetLimits, GoalBudgetReport, GoalState};

pub const GOAL_BUDGET_BLOCK_PREFIX: &str = "Blocked after goal budget reached";

pub fn compute_budget_report(state: &GoalState, wall_clock_ms: f64) -> GoalBudgetReport {
    let token_budget = state.budget_limits.token_budget;
    let turn_budget = state.budget_limits.turn_budget;
    let wall_clock_budget_ms = state.budget_limits.wall_clock_budget_ms;

    let token_budget_reached = token_budget.is_some_and(|budget| state.tokens_used >= budget);
    let turn_budget_reached = turn_budget.is_some_and(|budget| state.turns_used >= budget);
    let wall_clock_budget_reached =
        wall_clock_budget_ms.is_some_and(|budget| wall_clock_ms >= budget);
    GoalBudgetReport {
        token_budget,
        turn_budget,
        wall_clock_budget_ms,
        remaining_tokens: token_budget.map(|budget| (budget - state.tokens_used).max(0.0)),
        remaining_turns: turn_budget.map(|budget| (budget - state.turns_used).max(0.0)),
        remaining_wall_clock_ms: wall_clock_budget_ms
            .map(|budget| (budget - wall_clock_ms).max(0.0)),
        token_budget_reached,
        turn_budget_reached,
        wall_clock_budget_reached,
        over_budget: token_budget_reached || turn_budget_reached || wall_clock_budget_reached,
    }
}

pub fn matches_goal(state: &GoalState, goal_id: Option<&str>) -> bool {
    goal_id.is_none_or(|goal_id| state.goal_id == goal_id)
}

pub fn is_goal_mutation_tool(tool_name: &str) -> bool {
    matches!(tool_name, "CreateGoal" | "UpdateGoal" | "SetGoalBudget")
}

pub fn goal_budget_block_reason(budget: &GoalBudgetReport) -> Option<String> {
    let mut reached = Vec::new();
    if budget.turn_budget_reached {
        reached.push(format!(
            "turn budget {}",
            number_or_empty(budget.turn_budget)
        ));
    }
    if budget.token_budget_reached {
        reached.push(format!(
            "token budget {}",
            number_or_empty(budget.token_budget)
        ));
    }
    if budget.wall_clock_budget_reached {
        reached.push(format!(
            "wall-clock budget {}ms",
            number_or_empty(budget.wall_clock_budget_ms)
        ));
    }
    (!reached.is_empty()).then(|| format!("{GOAL_BUDGET_BLOCK_PREFIX}: {}", reached.join(", ")))
}

pub fn budget_telemetry_properties(limits: GoalBudgetLimits) -> GoalBudgetTelemetryProperties {
    GoalBudgetTelemetryProperties {
        has_token_budget: limits.token_budget.is_some(),
        has_turn_budget: limits.turn_budget.is_some(),
        has_wall_clock_budget: limits.wall_clock_budget_ms.is_some(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GoalBudgetTelemetryProperties {
    pub has_token_budget: bool,
    pub has_turn_budget: bool,
    pub has_wall_clock_budget: bool,
}

pub fn has_step_budget_remaining(
    max_steps: Option<u64>,
    current_step: crate::agent::StepId,
) -> bool {
    max_steps.is_none_or(|max_steps| max_steps == 0 || current_step.get() < max_steps)
}

fn number_or_empty(value: Option<f64>) -> String {
    value.map_or_else(String::new, javascript_number)
}

fn javascript_number(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value == f64::INFINITY {
        "Infinity".into()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".into()
    } else if value == 0.0 {
        "0".into()
    } else if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::goal::{GoalBudgetLimits, GoalState, GoalStatus};

    fn state() -> GoalState {
        GoalState {
            goal_id: "g1".into(),
            objective: "ship".into(),
            completion_criterion: None,
            status: GoalStatus::Active,
            turns_used: 3.0,
            tokens_used: 12.0,
            wall_clock_ms: 0.0,
            wall_clock_resumed_at: None,
            budget_limits: GoalBudgetLimits {
                token_budget: Some(10.0),
                turn_budget: Some(3.0),
                wall_clock_budget_ms: Some(50.0),
            },
            terminal_reason: None,
        }
    }

    #[test]
    fn reports_each_budget_independently_and_formats_the_source_reason_order() {
        let report = compute_budget_report(&state(), 60.0);
        assert_eq!(report.remaining_tokens, Some(0.0));
        assert_eq!(report.remaining_turns, Some(0.0));
        assert_eq!(report.remaining_wall_clock_ms, Some(0.0));
        assert!(report.over_budget);
        assert_eq!(
            goal_budget_block_reason(&report).as_deref(),
            Some(
                "Blocked after goal budget reached: turn budget 3, token budget 10, wall-clock budget 50ms"
            )
        );
    }

    #[test]
    fn helpers_preserve_goal_matching_tool_names_and_step_budget_rules() {
        assert!(matches_goal(&state(), None));
        assert!(matches_goal(&state(), Some("g1")));
        assert!(!matches_goal(&state(), Some("other")));
        assert!(is_goal_mutation_tool("UpdateGoal"));
        assert!(!is_goal_mutation_tool("ReadFile"));
        assert!(has_step_budget_remaining(
            None,
            crate::agent::StepId::new(100)
        ));
        assert!(has_step_budget_remaining(
            Some(0),
            crate::agent::StepId::new(100)
        ));
        assert!(has_step_budget_remaining(
            Some(2),
            crate::agent::StepId::new(1)
        ));
        assert!(!has_step_budget_remaining(
            Some(2),
            crate::agent::StepId::new(2)
        ));
    }
}
