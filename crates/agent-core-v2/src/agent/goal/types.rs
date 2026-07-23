use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalActor {
    User,
    Model,
    Runtime,
    System,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalBudgetLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_budget: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_clock_budget_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalBudgetReport {
    pub token_budget: Option<f64>,
    pub turn_budget: Option<f64>,
    pub wall_clock_budget_ms: Option<f64>,
    pub remaining_tokens: Option<f64>,
    pub remaining_turns: Option<f64>,
    pub remaining_wall_clock_ms: Option<f64>,
    pub token_budget_reached: bool,
    pub turn_budget_reached: bool,
    pub wall_clock_budget_reached: bool,
    pub over_budget: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSnapshot {
    pub goal_id: String,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    pub status: GoalStatus,
    pub turns_used: f64,
    pub tokens_used: f64,
    pub wall_clock_ms: f64,
    pub budget: GoalBudgetReport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GoalToolResult {
    pub goal: Option<GoalSnapshot>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalChangeStats {
    pub turns_used: f64,
    pub tokens_used: f64,
    pub wall_clock_ms: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalChangeKind {
    Lifecycle,
    Completion,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GoalChange {
    pub kind: GoalChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<GoalStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<GoalChangeStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<GoalActor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGoalInput {
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_report_serializes_absent_limits_as_required_nulls() {
        let report = GoalBudgetReport {
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
        };
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::json!({
                "tokenBudget": null,
                "turnBudget": null,
                "wallClockBudgetMs": null,
                "remainingTokens": null,
                "remainingTurns": null,
                "remainingWallClockMs": null,
                "tokenBudgetReached": false,
                "turnBudgetReached": false,
                "wallClockBudgetReached": false,
                "overBudget": false
            })
        );
    }

    #[test]
    fn lifecycle_types_preserve_camel_case_and_omit_undefined_fields() {
        let input = CreateGoalInput {
            objective: "ship".into(),
            completion_criterion: None,
            replace: Some(false),
        };
        assert_eq!(
            serde_json::to_value(input).unwrap(),
            serde_json::json!({"objective": "ship", "replace": false})
        );
        let change = GoalChange {
            kind: GoalChangeKind::Lifecycle,
            status: Some(GoalStatus::Blocked),
            reason: Some("waiting".into()),
            stats: None,
            actor: Some(GoalActor::Runtime),
        };
        assert_eq!(
            serde_json::to_value(change).unwrap(),
            serde_json::json!({
                "kind": "lifecycle",
                "status": "blocked",
                "reason": "waiting",
                "actor": "runtime"
            })
        );
    }
}
