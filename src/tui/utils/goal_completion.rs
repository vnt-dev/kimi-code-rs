use crate::utils::usage::usage_format::format_token_count;

/// Final goal statistics used to build deterministic completion text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalCompletionStats {
    pub terminal_reason: Option<String>,
    pub turns_used: u64,
    pub tokens_used: u64,
    pub wall_clock_ms: u64,
}

// Original:
//   apps/kimi-code/src/tui/utils/goal-completion.ts
//   buildGoalCompletionMessage()
//   buildGoalCompletionMessageFromStats()
//
// Rust adaptation:
//   The SDK snapshot is represented by the narrow data structure this pure
//   formatter actually consumes. The SDK adapter can construct it directly.
pub fn build_goal_completion_message(goal: &GoalCompletionStats) -> String {
    let reason = goal
        .terminal_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
        .map(|reason| format!(" — {reason}"))
        .unwrap_or_default();
    let head = format!("✓ Goal complete{reason}.");
    let turns_suffix = if goal.turns_used == 1 { "" } else { "s" };
    let stats = format!(
        "Worked {} turn{} over {}, using {} tokens.",
        goal.turns_used,
        turns_suffix,
        format_elapsed(goal.wall_clock_ms),
        format_token_count(goal.tokens_used as f64),
    );
    format!("{head}\n{stats}")
}

fn format_elapsed(milliseconds: u64) -> String {
    let total_seconds = milliseconds.saturating_add(500) / 1_000;
    if total_seconds < 60 {
        return format!("{total_seconds}s");
    }
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m{seconds:02}s");
    }
    let hours = minutes / 60;
    format!("{hours}h{:02}m", minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::{GoalCompletionStats, build_goal_completion_message, format_elapsed};

    fn stats() -> GoalCompletionStats {
        GoalCompletionStats {
            terminal_reason: Some("all tests pass".to_owned()),
            turns_used: 3,
            tokens_used: 12_500,
            wall_clock_ms: 260_000,
        }
    }

    #[test]
    fn includes_reason_exact_turns_tokens_and_time() {
        let text = build_goal_completion_message(&stats());
        assert!(text.contains("Goal complete — all tests pass."));
        assert!(text.contains("3 turns"));
        assert!(text.contains("12.2k tokens"));
        assert!(text.contains("4m20s"));
    }

    #[test]
    fn omits_dash_without_reason_and_singularizes_one_turn() {
        let text = build_goal_completion_message(&GoalCompletionStats {
            terminal_reason: None,
            turns_used: 1,
            tokens_used: 800,
            wall_clock_ms: 5_000,
        });
        assert!(text.contains("Goal complete."));
        assert!(!text.contains('—'));
        assert!(text.contains("1 turn "));
        assert!(text.contains("800 tokens"));
        assert!(text.contains("5s"));
    }

    #[test]
    fn empty_reason_matches_javascript_falsy_string_behavior() {
        let text = build_goal_completion_message(&GoalCompletionStats {
            terminal_reason: Some(String::new()),
            ..stats()
        });
        assert!(text.starts_with("✓ Goal complete.\n"));
    }

    #[test]
    fn rounds_and_formats_elapsed_time_at_boundaries() {
        assert_eq!(format_elapsed(499), "0s");
        assert_eq!(format_elapsed(500), "1s");
        assert_eq!(format_elapsed(59_500), "1m00s");
        assert_eq!(format_elapsed(3_599_499), "59m59s");
        assert_eq!(format_elapsed(3_599_500), "1h00m");
        assert_eq!(format_elapsed(3_660_000), "1h01m");
    }
}
