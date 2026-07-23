use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, ErrorInfo, register_error_domain};

pub const GOAL_ALREADY_EXISTS: &str = "goal.already_exists";
pub const GOAL_NOT_FOUND: &str = "goal.not_found";
pub const GOAL_OBJECTIVE_EMPTY: &str = "goal.objective_empty";
pub const GOAL_OBJECTIVE_TOO_LONG: &str = "goal.objective_too_long";
pub const GOAL_STATUS_INVALID: &str = "goal.status_invalid";
pub const GOAL_METADATA_RESERVED: &str = "goal.metadata_reserved";
pub const GOAL_NOT_RESUMABLE: &str = "goal.not_resumable";
pub const GOAL_UNSUPPORTED_AGENT: &str = "goal.unsupported_agent";

pub static GOAL_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("GOAL_ALREADY_EXISTS", GOAL_ALREADY_EXISTS),
        ("GOAL_NOT_FOUND", GOAL_NOT_FOUND),
        ("GOAL_OBJECTIVE_EMPTY", GOAL_OBJECTIVE_EMPTY),
        ("GOAL_OBJECTIVE_TOO_LONG", GOAL_OBJECTIVE_TOO_LONG),
        ("GOAL_STATUS_INVALID", GOAL_STATUS_INVALID),
        ("GOAL_METADATA_RESERVED", GOAL_METADATA_RESERVED),
        ("GOAL_NOT_RESUMABLE", GOAL_NOT_RESUMABLE),
        ("GOAL_UNSUPPORTED_AGENT", GOAL_UNSUPPORTED_AGENT),
    ],
    retryable: &[],
    info: &[
        (
            GOAL_ALREADY_EXISTS,
            ErrorInfo {
                title: "A goal is already active",
                retryable: false,
                public: true,
                action: Some("Use `/goal replace <objective>` to replace the current goal."),
            },
        ),
        (
            GOAL_NOT_FOUND,
            ErrorInfo {
                title: "No goal found",
                retryable: false,
                public: true,
                action: Some("Start a goal with `/goal <objective>` first."),
            },
        ),
        (
            GOAL_OBJECTIVE_EMPTY,
            ErrorInfo {
                title: "Goal objective is empty",
                retryable: false,
                public: true,
                action: Some("Provide a non-empty objective."),
            },
        ),
        (
            GOAL_OBJECTIVE_TOO_LONG,
            ErrorInfo {
                title: "Goal objective is too long",
                retryable: false,
                public: true,
                action: Some(
                    "Keep the objective under 4000 characters; reference long details by file path.",
                ),
            },
        ),
        (
            GOAL_STATUS_INVALID,
            ErrorInfo {
                title: "Invalid goal status transition",
                retryable: false,
                public: true,
                action: Some(
                    "Only an active goal can be paused; resume a blocked goal with `/goal resume`.",
                ),
            },
        ),
        (
            GOAL_METADATA_RESERVED,
            ErrorInfo {
                title: "Goal metadata is reserved",
                retryable: false,
                public: true,
                action: Some(
                    "Do not write metadata.custom.goal directly; use the goal lifecycle methods.",
                ),
            },
        ),
        (
            GOAL_NOT_RESUMABLE,
            ErrorInfo {
                title: "Goal is not resumable",
                retryable: false,
                public: true,
                action: Some("Only paused or blocked goals can be resumed."),
            },
        ),
        (
            GOAL_UNSUPPORTED_AGENT,
            ErrorInfo {
                title: "Goals are unavailable for subagents",
                retryable: false,
                public: true,
                action: Some("Run goal lifecycle commands on the main agent."),
            },
        ),
    ],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&GOAL_ERRORS).expect("goal error codes are unique");
});

// Original: goal/errors.ts, registerErrorDomain(GoalErrors).
pub fn ensure_goal_errors_registered() {
    LazyLock::force(&REGISTERED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn registers_all_public_non_retryable_errors_with_exact_guidance() {
        ensure_goal_errors_registered();
        for (_, code) in GOAL_ERRORS.codes {
            assert!(is_error_code(code));
            let info = error_info(code);
            assert!(!info.retryable);
            assert!(info.public);
            assert!(info.action.is_some());
        }
        assert_eq!(
            error_info(GOAL_OBJECTIVE_TOO_LONG).action.as_deref(),
            Some("Keep the objective under 4000 characters; reference long details by file path.")
        );
        assert_eq!(
            error_info(GOAL_UNSUPPORTED_AGENT).title,
            "Goals are unavailable for subagents"
        );
    }
}
