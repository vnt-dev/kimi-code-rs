//! `/init` subagent brief and completion reminder.
//!
//! Original: `session/sessionInit/profile/init.ts`.

pub const DEFAULT_INIT_PROMPT: &str = include_str!("init.md");

// Original: initCompletionReminder(). Whitespace-only AGENTS.md content is
// replaced for display, while nonempty content is retained verbatim.
pub fn init_completion_reminder(agents_md: &str) -> String {
    let latest = if agents_md.trim().is_empty() {
        "No AGENTS.md content was found after `/init` completed."
    } else {
        agents_md
    };
    [
        "The user just ran `/init` slash command.",
        "The system has analyzed the codebase and generated an `AGENTS.md` file.",
        "",
        "Latest AGENTS.md file content:",
        latest,
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_nonempty_agents_content_and_replaces_whitespace_only_content() {
        assert!(DEFAULT_INIT_PROMPT.contains("AGENTS.md"));
        assert_eq!(
            init_completion_reminder("\n  \t"),
            "The user just ran `/init` slash command.\nThe system has analyzed the codebase and generated an `AGENTS.md` file.\n\nLatest AGENTS.md file content:\nNo AGENTS.md content was found after `/init` completed."
        );
        assert!(
            init_completion_reminder("# Existing\n\nRules\n").ends_with("# Existing\n\nRules\n")
        );
    }
}
