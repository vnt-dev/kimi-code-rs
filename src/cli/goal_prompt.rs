use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    sdk::types::GoalSnapshot,
    tui::commands::goal::{ParsedGoalCommand, parse_goal_command},
};

pub const GOAL_EXIT_CODE_COMPLETE: i32 = 0;
pub const GOAL_EXIT_CODE_BLOCKED: i32 = 3;
pub const GOAL_EXIT_CODE_PAUSED: i32 = 6;

pub fn goal_exit_code(status: Option<&str>) -> i32 {
    match status {
        Some("blocked") => GOAL_EXIT_CODE_BLOCKED,
        Some("paused") => GOAL_EXIT_CODE_PAUSED,
        _ => GOAL_EXIT_CODE_COMPLETE,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessGoalCreate {
    pub objective: String,
    pub replace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessGoalError(String);

impl fmt::Display for HeadlessGoalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for HeadlessGoalError {}

// Original:
//   apps/kimi-code/src/cli/goal-prompt.ts
//   parseHeadlessGoalCreate()
pub fn parse_headless_goal_create(
    prompt: &str,
) -> Result<Option<HeadlessGoalCreate>, HeadlessGoalError> {
    let trimmed = prompt.trim();
    let Some(args) = trimmed.strip_prefix("/goal") else {
        return Ok(None);
    };
    if !args.is_empty() && !args.starts_with(char::is_whitespace) {
        return Ok(None);
    }

    match parse_goal_command(args.trim()) {
        ParsedGoalCommand::Error { message, .. } => Err(HeadlessGoalError(message)),
        ParsedGoalCommand::Create { objective, replace } => {
            Ok(Some(HeadlessGoalCreate { objective, replace }))
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSummary {
    #[serde(rename = "type")]
    pub kind: String,
    pub goal_id: Option<String>,
    pub status: Option<String>,
    pub reason: Option<String>,
    pub turns_used: Option<u64>,
    pub tokens_used: Option<u64>,
    pub wall_clock_ms: Option<u64>,
}

pub fn goal_summary_json(goal: Option<&GoalSnapshot>) -> GoalSummary {
    match goal {
        None => GoalSummary {
            kind: "goal.summary".to_owned(),
            goal_id: None,
            status: None,
            reason: None,
            turns_used: None,
            tokens_used: None,
            wall_clock_ms: None,
        },
        Some(goal) => GoalSummary {
            kind: "goal.summary".to_owned(),
            goal_id: Some(goal.goal_id.clone()),
            status: Some(goal.status.as_str().to_owned()),
            reason: goal.terminal_reason.clone(),
            turns_used: Some(goal.turns_used),
            tokens_used: Some(goal.tokens_used),
            wall_clock_ms: Some(goal.wall_clock_ms),
        },
    }
}

pub fn format_goal_summary_text(goal: Option<&GoalSnapshot>) -> String {
    let Some(goal) = goal else {
        return "Goal: no goal found.".to_owned();
    };
    let mut heading = format!("Goal [{}]", goal.status.as_str());
    if let Some(reason) = &goal.terminal_reason {
        heading.push_str(": ");
        heading.push_str(reason);
    }
    format!(
        "{heading} (turns: {}, tokens: {})",
        goal.turns_used, goal.tokens_used
    )
}

#[cfg(test)]
mod tests {
    use crate::sdk::types::{GoalBudgetReport, GoalStatus};

    use super::*;

    fn snapshot(status: GoalStatus, reason: Option<&str>) -> GoalSnapshot {
        GoalSnapshot {
            goal_id: "g1".to_owned(),
            objective: "work".to_owned(),
            completion_criterion: None,
            status,
            turns_used: 2,
            tokens_used: 120,
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
            terminal_reason: reason.map(str::to_owned),
        }
    }

    #[test]
    fn maps_final_statuses_to_distinct_exit_codes() {
        assert_eq!(goal_exit_code(Some("complete")), 0);
        assert_eq!(goal_exit_code(Some("blocked")), 3);
        assert_eq!(goal_exit_code(Some("paused")), 6);
        assert_eq!(goal_exit_code(None), 0);
        assert_eq!(goal_exit_code(Some("impossible")), 0);
    }

    #[test]
    fn parses_only_goal_create_prompts() {
        assert_eq!(
            parse_headless_goal_create(" /goal Ship feature X ").expect("parse"),
            Some(HeadlessGoalCreate {
                objective: "Ship feature X".to_owned(),
                replace: false,
            })
        );
        assert_eq!(
            parse_headless_goal_create("/goal replace Ship feature Y").expect("parse"),
            Some(HeadlessGoalCreate {
                objective: "Ship feature Y".to_owned(),
                replace: true,
            })
        );
        for prompt in ["say hello", "/goalkeeper", "/goal status", "/goal pause"] {
            assert_eq!(parse_headless_goal_create(prompt).expect("parse"), None);
        }
    }

    #[test]
    fn rejects_malformed_goal_create_prompts() {
        let error = parse_headless_goal_create(&format!("/goal {}", "x".repeat(4_001)))
            .expect_err("long objective");
        assert!(error.to_string().contains("Goal objective is too long"));
    }

    #[test]
    fn builds_machine_readable_and_text_summaries() {
        let goal = snapshot(GoalStatus::Blocked, Some("need creds"));
        let summary = goal_summary_json(Some(&goal));
        assert_eq!(summary.kind, "goal.summary");
        assert_eq!(summary.goal_id.as_deref(), Some("g1"));
        assert_eq!(summary.status.as_deref(), Some("blocked"));
        assert_eq!(summary.reason.as_deref(), Some("need creds"));
        assert_eq!(summary.turns_used, Some(2));
        assert_eq!(summary.tokens_used, Some(120));
        assert_eq!(
            format_goal_summary_text(Some(&goal)),
            "Goal [blocked]: need creds (turns: 2, tokens: 120)"
        );
    }

    #[test]
    fn renders_an_absent_goal_with_explicit_json_nulls() {
        let summary = goal_summary_json(None);
        assert_eq!(summary.status, None);
        assert_eq!(format_goal_summary_text(None), "Goal: no goal found.");
        assert_eq!(
            serde_json::to_value(summary).expect("summary json"),
            serde_json::json!({
                "type": "goal.summary",
                "goalId": null,
                "status": null,
                "reason": null,
                "turnsUsed": null,
                "tokensUsed": null,
                "wallClockMs": null,
            })
        );
    }
}
