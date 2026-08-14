use serde::Serialize;

use crate::agent::goal::{GoalBudgetReport, GoalSnapshot, GoalStatus, GoalToolResult};

// Original: packages/agent-core-v2/src/agent/goal/tools/serialize.ts
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSnapshotForModel {
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    pub status: GoalStatus,
    pub turns_used: u64,
    pub tokens_used: u64,
    pub wall_clock_ms: u64,
    pub budget: GoalBudgetReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

pub fn goal_for_model(goal: GoalSnapshot) -> GoalSnapshotForModel {
    GoalSnapshotForModel {
        objective: goal.objective,
        completion_criterion: goal.completion_criterion,
        status: goal.status,
        turns_used: goal.turns_used,
        tokens_used: goal.tokens_used,
        wall_clock_ms: goal.wall_clock_ms,
        budget: goal.budget,
        terminal_reason: goal.terminal_reason,
    }
}

pub fn goal_result_for_model(result: GoalToolResult) -> GoalResultForModel {
    GoalResultForModel {
        goal: result.goal.map(goal_for_model),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GoalResultForModel {
    pub goal: Option<GoalSnapshotForModel>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::goal::{GoalBudgetReport, GoalStatus};

    fn snapshot() -> GoalSnapshot {
        GoalSnapshot {
            goal_id: "private-id".into(),
            objective: "ship".into(),
            completion_criterion: None,
            status: GoalStatus::Active,
            turns_used: 1,
            tokens_used: 2,
            wall_clock_ms: 3,
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
        }
    }

    #[test]
    fn strips_goal_id_from_model_visible_results() {
        let result = goal_result_for_model(GoalToolResult {
            goal: Some(snapshot()),
        });
        let value = serde_json::to_value(result).unwrap();
        assert!(value["goal"].get("goalId").is_none());
        assert_eq!(value["goal"]["objective"], "ship");
    }
}
