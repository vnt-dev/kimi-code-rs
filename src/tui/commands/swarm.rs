use std::{fmt::Display, future::Future};

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

pub trait SwarmCommandHost {
    type Error: Display + Send;

    fn set_permission(
        &mut self,
        mode: PermissionMode,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn set_swarm_mode(
        &mut self,
        enabled: bool,
        trigger: SwarmModeTrigger,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn update_permission_mode(&mut self, mode: PermissionMode);
    fn update_swarm_mode(&mut self, enabled: bool, entry: Option<SwarmModeTrigger>);
    fn render_swarm_mode_marker(&mut self, active: bool);
    fn send_normal_user_input(&mut self, prompt: &str);
    fn show_error(&mut self, message: &str);
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

/// Apply a choice from the Manual-mode permission prompt. Permission changes
/// finish before the swarm action starts, matching the original call order.
///
/// Original:
///   apps/kimi-code/src/tui/commands/swarm.ts
///   startSwarmWithPermission(), setPermissionForSwarm()
pub async fn execute_swarm_permission_plan<H: SwarmCommandHost>(
    host: &mut H,
    plan: SwarmPermissionPlan,
) {
    if let Some(mode) = plan.permission_change {
        if let Err(error) = host.set_permission(mode).await {
            host.show_error(&format!("Failed to set permission mode: {error}"));
            return;
        }
        host.update_permission_mode(mode);
    }
    execute_swarm_action(host, plan.action).await;
}

/// Original:
///   apps/kimi-code/src/tui/commands/swarm.ts
///   startSwarmTask(), setSwarmMode(), renderSwarmModeMarker()
pub async fn execute_swarm_action<H: SwarmCommandHost>(host: &mut H, action: SwarmAction) {
    match action {
        SwarmAction::SetMode { enabled, trigger } => {
            if !set_swarm_mode(host, enabled, trigger).await {
                return;
            }
            host.render_swarm_mode_marker(enabled);
        }
        SwarmAction::StartTask {
            prompt,
            enable_mode,
        } => {
            if enable_mode && !set_swarm_mode(host, true, SwarmModeTrigger::Task).await {
                return;
            }
            host.render_swarm_mode_marker(true);
            host.send_normal_user_input(&prompt);
        }
    }
}

async fn set_swarm_mode<H: SwarmCommandHost>(
    host: &mut H,
    enabled: bool,
    trigger: SwarmModeTrigger,
) -> bool {
    if let Err(error) = host.set_swarm_mode(enabled, trigger).await {
        let operation = if enabled { "enable" } else { "disable" };
        host.show_error(&format!("Failed to {operation} swarm mode: {error}"));
        return false;
    }
    host.update_swarm_mode(enabled, enabled.then_some(trigger));
    true
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

    #[derive(Default)]
    struct Host {
        operations: Vec<String>,
        permission_error: Option<&'static str>,
        swarm_error: Option<&'static str>,
    }

    impl SwarmCommandHost for Host {
        type Error = &'static str;

        async fn set_permission(&mut self, mode: PermissionMode) -> Result<(), Self::Error> {
            self.operations.push(format!("set_permission:{mode:?}"));
            self.permission_error.take().map_or(Ok(()), Err)
        }

        async fn set_swarm_mode(
            &mut self,
            enabled: bool,
            trigger: SwarmModeTrigger,
        ) -> Result<(), Self::Error> {
            self.operations
                .push(format!("set_swarm:{enabled}:{trigger:?}"));
            self.swarm_error.take().map_or(Ok(()), Err)
        }

        fn update_permission_mode(&mut self, mode: PermissionMode) {
            self.operations.push(format!("permission_state:{mode:?}"));
        }

        fn update_swarm_mode(&mut self, enabled: bool, entry: Option<SwarmModeTrigger>) {
            self.operations
                .push(format!("swarm_state:{enabled}:{entry:?}"));
        }

        fn render_swarm_mode_marker(&mut self, active: bool) {
            self.operations.push(format!("marker:{active}"));
        }

        fn send_normal_user_input(&mut self, prompt: &str) {
            self.operations.push(format!("send:{prompt}"));
        }

        fn show_error(&mut self, message: &str) {
            self.operations.push(format!("error:{message}"));
        }
    }

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

    #[tokio::test]
    async fn executes_permission_mode_marker_and_prompt_in_order() {
        let mut host = Host::default();
        execute_swarm_permission_plan(
            &mut host,
            SwarmPermissionPlan {
                permission_change: Some(PermissionMode::Auto),
                action: SwarmAction::StartTask {
                    prompt: "Ship feature X".to_owned(),
                    enable_mode: true,
                },
            },
        )
        .await;
        assert_eq!(
            host.operations,
            [
                "set_permission:Auto",
                "permission_state:Auto",
                "set_swarm:true:Task",
                "swarm_state:true:Some(Task)",
                "marker:true",
                "send:Ship feature X",
            ]
        );
    }

    #[tokio::test]
    async fn skips_reentering_mode_for_task_when_already_enabled() {
        let mut host = Host::default();
        execute_swarm_action(
            &mut host,
            SwarmAction::StartTask {
                prompt: "Continue".to_owned(),
                enable_mode: false,
            },
        )
        .await;
        assert_eq!(host.operations, ["marker:true", "send:Continue"]);
    }

    #[tokio::test]
    async fn permission_and_mode_failures_short_circuit_later_side_effects() {
        let mut permission_failure = Host {
            permission_error: Some("denied"),
            ..Host::default()
        };
        execute_swarm_permission_plan(
            &mut permission_failure,
            SwarmPermissionPlan {
                permission_change: Some(PermissionMode::Yolo),
                action: SwarmAction::StartTask {
                    prompt: "task".to_owned(),
                    enable_mode: true,
                },
            },
        )
        .await;
        assert_eq!(
            permission_failure.operations,
            [
                "set_permission:Yolo",
                "error:Failed to set permission mode: denied"
            ]
        );

        let mut mode_failure = Host {
            swarm_error: Some("denied"),
            ..Host::default()
        };
        execute_swarm_action(
            &mut mode_failure,
            SwarmAction::StartTask {
                prompt: "task".to_owned(),
                enable_mode: true,
            },
        )
        .await;
        assert_eq!(
            mode_failure.operations,
            [
                "set_swarm:true:Task",
                "error:Failed to enable swarm mode: denied"
            ]
        );
    }
}
