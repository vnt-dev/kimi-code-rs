use crate::agent::goal::GoalSnapshot;

// Original: packages/agent-core-v2/src/agent/goal/tools/outcome-prompts.ts
pub fn build_goal_completion_summary_prompt(goal: &GoalSnapshot) -> String {
    format!(
        "{}\n\nWrite a concise final message for the user. State that the goal is complete, summarize the main work completed, and mention any validation you ran. Do not call more goal tools.",
        build_goal_completion_prompt_message(goal)
    )
}

pub fn build_goal_blocked_reason_prompt(goal: &GoalSnapshot) -> String {
    format!(
        "{}\n\nWrite a concise final message for the user. State that the goal is blocked, explain the concrete blocker, and say what input or change is needed before work can continue. Do not call more goal tools.",
        build_goal_blocked_message(goal)
    )
}

fn build_goal_completion_prompt_message(goal: &GoalSnapshot) -> String {
    let reason = goal
        .terminal_reason
        .as_ref()
        .map_or_else(String::new, |reason| format!(": {reason}"));
    format!(
        "Goal completed successfully{reason}.\n{}",
        goal_worked_stats(goal)
    )
}

fn build_goal_blocked_message(goal: &GoalSnapshot) -> String {
    format!("Goal blocked.\n{}", goal_worked_stats(goal))
}

fn goal_worked_stats(goal: &GoalSnapshot) -> String {
    let plural = if goal.turns_used == 1.0 { "" } else { "s" };
    format!(
        "Worked {} turn{plural} over {}, using {} tokens.",
        javascript_number(goal.turns_used),
        format_elapsed(goal.wall_clock_ms),
        format_tokens(goal.tokens_used),
    )
}

fn format_elapsed(milliseconds: f64) -> String {
    let seconds = (milliseconds / 1000.0).round() as i64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m{seconds:02}s");
    }
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

fn format_tokens(tokens: f64) -> String {
    if tokens < 1000.0 {
        javascript_number(tokens)
    } else if tokens < 1_000_000.0 {
        format!("{:.1}k", tokens / 1000.0)
    } else {
        format!("{:.1}M", tokens / 1_000_000.0)
    }
}

fn javascript_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::goal::{GoalBudgetReport, GoalStatus};

    fn snapshot() -> GoalSnapshot {
        GoalSnapshot {
            goal_id: "g".into(),
            objective: "ship".into(),
            completion_criterion: None,
            status: GoalStatus::Complete,
            turns_used: 2.0,
            tokens_used: 1500.0,
            wall_clock_ms: 61_000.0,
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
            terminal_reason: Some("done".into()),
        }
    }

    #[test]
    fn completion_and_blocked_prompts_preserve_source_stats_formatting() {
        let goal = snapshot();
        let completion = build_goal_completion_summary_prompt(&goal);
        assert!(completion.starts_with(
            "Goal completed successfully: done.\nWorked 2 turns over 1m01s, using 1.5k tokens."
        ));
        assert!(completion.ends_with("Do not call more goal tools."));
        let blocked = build_goal_blocked_reason_prompt(&goal);
        assert!(
            blocked.starts_with("Goal blocked.\nWorked 2 turns over 1m01s, using 1.5k tokens.")
        );
    }
}
