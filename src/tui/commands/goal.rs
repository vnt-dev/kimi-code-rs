pub const MAX_GOAL_OBJECTIVE_LENGTH: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalCommandErrorSeverity {
    Error,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedGoalCommand {
    Status,
    Pause,
    Resume,
    Cancel,
    Create {
        objective: String,
        replace: bool,
    },
    NextAdd {
        objective: String,
    },
    NextManage,
    Error {
        message: String,
        severity: GoalCommandErrorSeverity,
    },
}

// Original:
//   apps/kimi-code/src/tui/commands/goal.ts
//   parseGoalCommand()
pub fn parse_goal_command(raw_args: &str) -> ParsedGoalCommand {
    let args = raw_args.trim();
    if args.is_empty() || args == "status" {
        return ParsedGoalCommand::Status;
    }

    let tokens = args.split_whitespace().collect::<Vec<_>>();
    match tokens.as_slice() {
        ["next", ..] => return parse_next_goal_command(&tokens),
        ["pause"] => return ParsedGoalCommand::Pause,
        ["resume"] => return ParsedGoalCommand::Resume,
        ["cancel"] => return ParsedGoalCommand::Cancel,
        _ => {}
    }

    let mut index = 0;
    let replace = tokens.get(index) == Some(&"replace");
    if replace {
        index += 1;
    }
    if tokens.get(index) == Some(&"--") {
        index += 1;
    }

    parse_objective(&tokens[index..], replace)
}

fn parse_next_goal_command(tokens: &[&str]) -> ParsedGoalCommand {
    if tokens == ["next", "manage"] {
        return ParsedGoalCommand::NextManage;
    }
    let mut index = 1;
    if tokens.get(index) == Some(&"--") {
        index += 1;
    }
    let objective = tokens[index..].join(" ").trim().to_owned();
    if objective.is_empty() {
        return ParsedGoalCommand::Error {
            message: "Provide an upcoming goal objective, e.g. `/goal next Ship feature X`, or use `/goal next manage`.".to_owned(),
            severity: GoalCommandErrorSeverity::Hint,
        };
    }
    if javascript_string_len(&objective) > MAX_GOAL_OBJECTIVE_LENGTH {
        return objective_too_long_error();
    }
    ParsedGoalCommand::NextAdd { objective }
}

fn parse_objective(tokens: &[&str], replace: bool) -> ParsedGoalCommand {
    let objective = tokens.join(" ").trim().to_owned();
    if objective.is_empty() {
        return ParsedGoalCommand::Error {
            message: "Provide a goal objective, e.g. `/goal Ship feature X`.".to_owned(),
            severity: GoalCommandErrorSeverity::Hint,
        };
    }
    if javascript_string_len(&objective) > MAX_GOAL_OBJECTIVE_LENGTH {
        return objective_too_long_error();
    }
    ParsedGoalCommand::Create { objective, replace }
}

fn objective_too_long_error() -> ParsedGoalCommand {
    ParsedGoalCommand::Error {
        message: format!(
            "Goal objective is too long (max {MAX_GOAL_OBJECTIVE_LENGTH} characters). Reference long details by file path."
        ),
        severity: GoalCommandErrorSeverity::Error,
    }
}

fn javascript_string_len(value: &str) -> usize {
    value.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_and_control_commands() {
        assert_eq!(parse_goal_command(""), ParsedGoalCommand::Status);
        assert_eq!(parse_goal_command("status"), ParsedGoalCommand::Status);
        assert_eq!(parse_goal_command("pause"), ParsedGoalCommand::Pause);
        assert_eq!(parse_goal_command("resume"), ParsedGoalCommand::Resume);
        assert_eq!(parse_goal_command("cancel"), ParsedGoalCommand::Cancel);
    }

    #[test]
    fn parses_plain_replace_and_escaped_objectives() {
        assert_eq!(
            parse_goal_command("Ship   feature X"),
            ParsedGoalCommand::Create {
                objective: "Ship feature X".to_owned(),
                replace: false,
            }
        );
        assert_eq!(
            parse_goal_command("replace Ship feature Y"),
            ParsedGoalCommand::Create {
                objective: "Ship feature Y".to_owned(),
                replace: true,
            }
        );
        assert_eq!(
            parse_goal_command("-- cancel"),
            ParsedGoalCommand::Create {
                objective: "cancel".to_owned(),
                replace: false,
            }
        );
        assert_eq!(
            parse_goal_command("clear"),
            ParsedGoalCommand::Create {
                objective: "clear".to_owned(),
                replace: false,
            }
        );
    }

    #[test]
    fn parses_upcoming_goal_commands() {
        assert_eq!(
            parse_goal_command("next Ship release notes"),
            ParsedGoalCommand::NextAdd {
                objective: "Ship release notes".to_owned(),
            }
        );
        assert_eq!(
            parse_goal_command("next manage"),
            ParsedGoalCommand::NextManage
        );
        assert_eq!(
            parse_goal_command("next -- manage release notes"),
            ParsedGoalCommand::NextAdd {
                objective: "manage release notes".to_owned(),
            }
        );
    }

    #[test]
    fn returns_hints_for_missing_objectives() {
        assert!(matches!(
            parse_goal_command("replace"),
            ParsedGoalCommand::Error {
                severity: GoalCommandErrorSeverity::Hint,
                ..
            }
        ));
        assert!(matches!(
            parse_goal_command("next"),
            ParsedGoalCommand::Error {
                severity: GoalCommandErrorSeverity::Hint,
                ..
            }
        ));
    }

    #[test]
    fn counts_the_limit_like_javascript_utf16_strings() {
        assert!(matches!(
            parse_goal_command(&"x".repeat(4_001)),
            ParsedGoalCommand::Error {
                severity: GoalCommandErrorSeverity::Error,
                ..
            }
        ));
        assert!(matches!(
            parse_goal_command(&"😀".repeat(2_001)),
            ParsedGoalCommand::Error {
                severity: GoalCommandErrorSeverity::Error,
                ..
            }
        ));
        assert!(matches!(
            parse_goal_command(&"😀".repeat(2_000)),
            ParsedGoalCommand::Create { .. }
        ));
    }
}
