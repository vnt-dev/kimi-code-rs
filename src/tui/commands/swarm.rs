use crate::sdk::types::PermissionMode;

pub const LLM_NOT_SET_MESSAGE: &str = "LLM not set, send \"/login\" to login";
pub const NO_ACTIVE_SESSION_MESSAGE: &str = "No active session. Send /login to login.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwarmModeTrigger {
    Manual,
    Task,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwarmAction {
    SetMode {
        enabled: bool,
        trigger: SwarmModeTrigger,
    },
    StartTask {
        prompt: String,
        enable_mode: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmPermissionPrompt {
    pub command_text: String,
    pub cancel_status: &'static str,
    pub continuation: SwarmAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwarmCommandPlan {
    Error(&'static str),
    Status(&'static str),
    PermissionPrompt(SwarmPermissionPrompt),
    Execute(SwarmAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmPermissionPlan {
    pub permission_change: Option<PermissionMode>,
    pub action: SwarmAction,
}

#[derive(Debug, Clone, Copy)]
pub struct SwarmCommandState<'a> {
    pub has_session: bool,
    pub model: &'a str,
    pub permission_mode: PermissionMode,
    pub swarm_mode: bool,
}

/// Resolve the synchronous branches of `handleSwarmCommand`. The UI adapter
/// renders `PermissionPrompt`; the async executor applies `Execute` in order
/// (permission, mode change, marker, then user input).
///
/// Original:
///   apps/kimi-code/src/tui/commands/swarm.ts
///   handleSwarmCommand(), applySwarmMode(), startSwarmTask()
pub fn plan_swarm_command(state: SwarmCommandState<'_>, args: &str) -> SwarmCommandPlan {
    if !state.has_session {
        return SwarmCommandPlan::Error(NO_ACTIVE_SESSION_MESSAGE);
    }

    let prompt = args.trim();
    if let Some(enabled) = swarm_mode_subcommand(prompt) {
        return plan_mode_change(state, enabled, format!("/swarm {prompt}"));
    }
    if prompt.is_empty() {
        return plan_mode_change(state, !state.swarm_mode, "/swarm".to_owned());
    }
    if state.model.trim().is_empty() {
        return SwarmCommandPlan::Error(LLM_NOT_SET_MESSAGE);
    }

    let action = SwarmAction::StartTask {
        prompt: prompt.to_owned(),
        enable_mode: !state.swarm_mode,
    };
    if state.permission_mode == PermissionMode::Manual {
        SwarmCommandPlan::PermissionPrompt(SwarmPermissionPrompt {
            command_text: format!("/swarm {prompt}"),
            cancel_status: "Swarm task not started.",
            continuation: action,
        })
    } else {
        SwarmCommandPlan::Execute(action)
    }
}

fn plan_mode_change(
    state: SwarmCommandState<'_>,
    enabled: bool,
    command_text: String,
) -> SwarmCommandPlan {
    if enabled == state.swarm_mode {
        return SwarmCommandPlan::Status(if enabled {
            "Swarm mode is already on."
        } else {
            "Swarm mode is already off."
        });
    }
    let action = SwarmAction::SetMode {
        enabled,
        trigger: SwarmModeTrigger::Manual,
    };
    if enabled && state.permission_mode == PermissionMode::Manual {
        SwarmCommandPlan::PermissionPrompt(SwarmPermissionPrompt {
            command_text,
            cancel_status: "Swarm mode not enabled.",
            continuation: action,
        })
    } else {
        SwarmCommandPlan::Execute(action)
    }
}

/// Original: swarm.ts startSwarmWithPermission()
pub fn plan_swarm_permission_choice(
    prompt: &SwarmPermissionPrompt,
    choice: PermissionMode,
) -> SwarmPermissionPlan {
    SwarmPermissionPlan {
        permission_change: matches!(choice, PermissionMode::Auto | PermissionMode::Yolo)
            .then_some(choice),
        action: prompt.continuation.clone(),
    }
}

/// Original: swarm.ts swarmModeSubcommand()
pub fn swarm_mode_subcommand(input: &str) -> Option<bool> {
    match input.to_lowercase().as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(permission_mode: PermissionMode, swarm_mode: bool) -> SwarmCommandState<'static> {
        SwarmCommandState {
            has_session: true,
            model: "kimi-model",
            permission_mode,
            swarm_mode,
        }
    }

    #[test]
    fn validates_session_and_model_in_original_branch_order() {
        let mut value = state(PermissionMode::Auto, false);
        value.has_session = false;
        assert_eq!(
            plan_swarm_command(value, "on"),
            SwarmCommandPlan::Error(NO_ACTIVE_SESSION_MESSAGE)
        );

        value.has_session = true;
        value.model = "  ";
        assert!(matches!(
            plan_swarm_command(value, "task"),
            SwarmCommandPlan::Error(LLM_NOT_SET_MESSAGE)
        ));
        assert!(matches!(
            plan_swarm_command(value, "on"),
            SwarmCommandPlan::Execute(SwarmAction::SetMode { enabled: true, .. })
        ));
    }

    #[test]
    fn toggles_bare_command_and_reports_idempotent_explicit_modes() {
        assert_eq!(
            plan_swarm_command(state(PermissionMode::Auto, false), ""),
            SwarmCommandPlan::Execute(SwarmAction::SetMode {
                enabled: true,
                trigger: SwarmModeTrigger::Manual
            })
        );
        assert_eq!(
            plan_swarm_command(state(PermissionMode::Auto, true), "ON"),
            SwarmCommandPlan::Status("Swarm mode is already on.")
        );
        assert_eq!(
            plan_swarm_command(state(PermissionMode::Auto, false), "off"),
            SwarmCommandPlan::Status("Swarm mode is already off.")
        );
    }

    #[test]
    fn starts_task_and_only_enters_swarm_when_needed() {
        assert_eq!(
            plan_swarm_command(state(PermissionMode::Auto, false), " Ship feature X "),
            SwarmCommandPlan::Execute(SwarmAction::StartTask {
                prompt: "Ship feature X".to_owned(),
                enable_mode: true
            })
        );
        assert_eq!(
            plan_swarm_command(state(PermissionMode::Yolo, true), "Ship feature X"),
            SwarmCommandPlan::Execute(SwarmAction::StartTask {
                prompt: "Ship feature X".to_owned(),
                enable_mode: false
            })
        );
    }

    #[test]
    fn manual_mode_returns_prompt_with_exact_cancel_context() {
        let task = plan_swarm_command(state(PermissionMode::Manual, false), "Ship feature X");
        let SwarmCommandPlan::PermissionPrompt(task_prompt) = task else {
            panic!("expected task permission prompt");
        };
        assert_eq!(task_prompt.command_text, "/swarm Ship feature X");
        assert_eq!(task_prompt.cancel_status, "Swarm task not started.");
        let permission = plan_swarm_permission_choice(&task_prompt, PermissionMode::Yolo);
        assert_eq!(permission.permission_change, Some(PermissionMode::Yolo));

        let mode = plan_swarm_command(state(PermissionMode::Manual, false), "on");
        let SwarmCommandPlan::PermissionPrompt(mode_prompt) = mode else {
            panic!("expected mode permission prompt");
        };
        assert_eq!(mode_prompt.cancel_status, "Swarm mode not enabled.");
        assert_eq!(
            plan_swarm_permission_choice(&mode_prompt, PermissionMode::Manual).permission_change,
            None
        );
    }
}
