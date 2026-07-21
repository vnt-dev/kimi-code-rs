/// Original:
///   apps/kimi-code/src/tui/components/messages/goal-format.ts
///   formatGoalElapsed()
pub fn format_goal_elapsed(milliseconds: u64) -> String {
    let total_seconds = milliseconds.saturating_add(500) / 1_000;
    if total_seconds < 60 {
        return format!("{total_seconds}s");
    }
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = minutes / 60;
    format!("{hours}h {:02}m", minutes % 60)
}

/// Original:
///   apps/kimi-code/src/tui/components/messages/goal-format.ts
///   pluralizeGoalCount()
pub fn pluralize_goal_count(count: u64, singular: &str, plural: Option<&str>) -> String {
    let noun = if count == 1 {
        singular.to_owned()
    } else {
        plural
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{singular}s"))
    };
    format!("{count} {noun}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_and_formats_seconds_minutes_and_hours() {
        assert_eq!(format_goal_elapsed(499), "0s");
        assert_eq!(format_goal_elapsed(500), "1s");
        assert_eq!(format_goal_elapsed(59_499), "59s");
        assert_eq!(format_goal_elapsed(59_500), "1m 00s");
        assert_eq!(format_goal_elapsed(3_599_499), "59m 59s");
        assert_eq!(format_goal_elapsed(3_599_500), "1h 00m");
        assert_eq!(format_goal_elapsed(3_660_000), "1h 01m");
    }

    #[test]
    fn pluralizes_regular_and_irregular_counts() {
        assert_eq!(pluralize_goal_count(1, "turn", None), "1 turn");
        assert_eq!(pluralize_goal_count(2, "turn", None), "2 turns");
        assert_eq!(
            pluralize_goal_count(0, "entry", Some("entries")),
            "0 entries"
        );
    }
}
