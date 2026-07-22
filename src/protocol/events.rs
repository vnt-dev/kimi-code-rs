use serde::{Deserialize, Serialize};

use super::validation::required_nullable;

// Original: packages/protocol/src/events.ts, SkillSource.
// This module is expanded as the event schema migration proceeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Project,
    User,
    Extra,
    Builtin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalActor {
    User,
    Model,
    Runtime,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GoalBudgetLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_budget: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_clock_budget_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalBudgetReport {
    #[serde(deserialize_with = "required_nullable")]
    pub token_budget: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub turn_budget: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub wall_clock_budget_ms: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub remaining_tokens: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub remaining_turns: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub remaining_wall_clock_ms: Option<f64>,
    pub token_budget_reached: bool,
    pub turn_budget_reached: bool,
    pub wall_clock_budget_reached: bool,
    pub over_budget: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSnapshot {
    pub goal_id: String,
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    pub status: GoalStatus,
    pub turns_used: f64,
    pub tokens_used: f64,
    pub wall_clock_ms: f64,
    pub budget: GoalBudgetReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalToolResult {
    #[serde(deserialize_with = "required_nullable")]
    pub goal: Option<GoalSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalChangeStats {
    pub turns_used: f64,
    pub tokens_used: f64,
    pub wall_clock_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalChangeKind {
    Lifecycle,
    Completion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalChange {
    pub kind: GoalChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<GoalStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<GoalChangeStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<GoalActor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_snapshot_preserves_camel_case_and_required_nullable_budget() {
        let snapshot: GoalSnapshot = serde_json::from_value(serde_json::json!({
            "goalId":"g","objective":"ship","status":"active","turnsUsed":1,
            "tokensUsed":2,"wallClockMs":3,"budget":{"tokenBudget":null,
                "turnBudget":null,"wallClockBudgetMs":null,"remainingTokens":null,
                "remainingTurns":null,"remainingWallClockMs":null,
                "tokenBudgetReached":false,"turnBudgetReached":false,
                "wallClockBudgetReached":false,"overBudget":false}
        }))
        .unwrap();
        assert_eq!(snapshot.status, GoalStatus::Active);
        assert_eq!(serde_json::to_value(snapshot).unwrap()["goalId"], "g");
        assert!(serde_json::from_value::<GoalToolResult>(serde_json::json!({})).is_err());
    }
}
