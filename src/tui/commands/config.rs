use async_trait::async_trait;

use crate::sdk::types::PermissionMode;

pub const NO_ACTIVE_SESSION_MESSAGE: &str = "No active session. Send /login to login.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeCommandState {
    pub has_session: bool,
    pub plan_mode: bool,
    pub permission_mode: PermissionMode,
}

#[async_trait(?Send)]
pub trait ModeCommandHost {
    fn mode_command_state(&self) -> ModeCommandState;
    async fn clear_plan(&mut self) -> Result<(), String>;
    async fn set_plan_mode(&mut self, enabled: bool) -> Result<(), String>;
    async fn get_plan_path(&self) -> Result<Option<String>, String>;
    async fn set_permission(&mut self, mode: PermissionMode) -> Result<(), String>;
    async fn compact(&mut self, instruction: Option<&str>) -> Result<(), String>;
    fn update_plan_mode(&mut self, enabled: bool);
    fn update_permission_mode(&mut self, mode: PermissionMode);
    fn show_notice(&mut self, title: &str, detail: Option<&str>);
    fn show_error(&mut self, message: &str);
}

// Original: `src/tui/commands/config.ts`, `handlePlanCommand()`.
pub async fn handle_plan_command(host: &mut impl ModeCommandHost, args: &str) {
    let state = host.mode_command_state();
    if !state.has_session {
        host.show_error(NO_ACTIVE_SESSION_MESSAGE);
        return;
    }
    let subcommand = args.trim().to_lowercase();
    if subcommand == "clear" {
        match host.clear_plan().await {
            Ok(()) => host.show_notice("Plan cleared", None),
            Err(error) => host.show_error(&format!("Failed to clear plan: {error}")),
        }
        return;
    }
    let enabled = match subcommand.as_str() {
        "" => !state.plan_mode,
        "on" => true,
        "off" => false,
        _ => {
            host.show_error(&format!("Unknown plan subcommand: {subcommand}"));
            return;
        }
    };
    apply_plan_mode(host, enabled).await;
}

// Original: `applyPlanMode()`.
async fn apply_plan_mode(host: &mut impl ModeCommandHost, enabled: bool) {
    if let Err(error) = host.set_plan_mode(enabled).await {
        host.show_error(&format!("Failed to set plan mode: {error}"));
        return;
    }
    host.update_plan_mode(enabled);
    if enabled {
        let path = host.get_plan_path().await.ok().flatten();
        let detail = path
            .as_deref()
            .map(|path| format!("Plan will be created here: {path}"));
        host.show_notice("Plan mode: ON", detail.as_deref());
    } else {
        host.show_notice("Plan mode: OFF", None);
    }
}

// Original: `handleYoloCommand()`.
pub async fn handle_yolo_command(
    host: &mut impl ModeCommandHost,
    args: &str,
) -> Result<(), String> {
    handle_permission_toggle(
        host,
        args,
        PermissionMode::Yolo,
        "YOLO",
        "Tool actions auto-approved; the agent may still ask you questions.",
    )
    .await
}

// Original: `handleAutoCommand()`.
pub async fn handle_auto_command(
    host: &mut impl ModeCommandHost,
    args: &str,
) -> Result<(), String> {
    handle_permission_toggle(
        host,
        args,
        PermissionMode::Auto,
        "Auto",
        "All actions auto-approved; the agent will not ask you questions.",
    )
    .await
}

async fn handle_permission_toggle(
    host: &mut impl ModeCommandHost,
    args: &str,
    target: PermissionMode,
    label: &str,
    enabled_detail: &str,
) -> Result<(), String> {
    let state = host.mode_command_state();
    if !state.has_session {
        host.show_error(NO_ACTIVE_SESSION_MESSAGE);
        return Ok(());
    }
    let subcommand = args.trim().to_lowercase();
    let current = state.permission_mode;
    let enabled = match subcommand.as_str() {
        "on" if current == target => {
            host.show_notice(&format!("{label} mode is already on"), None);
            return Ok(());
        }
        "on" => true,
        "off" if current != target => {
            host.show_notice(&format!("{label} mode is already off"), None);
            return Ok(());
        }
        "off" => false,
        _ => current != target,
    };
    let next = if enabled {
        target
    } else {
        PermissionMode::Manual
    };
    host.set_permission(next).await?;
    host.update_permission_mode(next);
    host.show_notice(
        &format!("{label} mode: {}", if enabled { "ON" } else { "OFF" }),
        enabled.then_some(enabled_detail),
    );
    Ok(())
}

// Original: `handleCompactCommand()`.
pub async fn handle_compact_command(
    host: &mut impl ModeCommandHost,
    args: &str,
) -> Result<(), String> {
    if !host.mode_command_state().has_session {
        host.show_error(NO_ACTIVE_SESSION_MESSAGE);
        return Ok(());
    }
    let instruction = args.trim();
    host.compact((!instruction.is_empty()).then_some(instruction))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Host {
        state: ModeCommandState,
        operations: Vec<String>,
        notices: Vec<(String, Option<String>)>,
        errors: Vec<String>,
        plan_path: Option<String>,
        failure: Option<String>,
    }

    impl Default for Host {
        fn default() -> Self {
            Self {
                state: ModeCommandState {
                    has_session: true,
                    plan_mode: false,
                    permission_mode: PermissionMode::Manual,
                },
                operations: Vec::new(),
                notices: Vec::new(),
                errors: Vec::new(),
                plan_path: None,
                failure: None,
            }
        }
    }

    #[async_trait(?Send)]
    impl ModeCommandHost for Host {
        fn mode_command_state(&self) -> ModeCommandState {
            self.state
        }

        async fn clear_plan(&mut self) -> Result<(), String> {
            self.operations.push("clear_plan".to_owned());
            self.failure.clone().map_or(Ok(()), Err)
        }

        async fn set_plan_mode(&mut self, enabled: bool) -> Result<(), String> {
            self.operations.push(format!("set_plan:{enabled}"));
            self.failure.clone().map_or(Ok(()), Err)
        }

        async fn get_plan_path(&self) -> Result<Option<String>, String> {
            Ok(self.plan_path.clone())
        }

        async fn set_permission(&mut self, mode: PermissionMode) -> Result<(), String> {
            self.operations.push(format!("set_permission:{mode:?}"));
            self.failure.clone().map_or(Ok(()), Err)
        }

        async fn compact(&mut self, instruction: Option<&str>) -> Result<(), String> {
            self.operations
                .push(format!("compact:{}", instruction.unwrap_or("")));
            self.failure.clone().map_or(Ok(()), Err)
        }

        fn update_plan_mode(&mut self, enabled: bool) {
            self.state.plan_mode = enabled;
            self.operations.push(format!("update_plan:{enabled}"));
        }

        fn update_permission_mode(&mut self, mode: PermissionMode) {
            self.state.permission_mode = mode;
            self.operations.push(format!("update_permission:{mode:?}"));
        }

        fn show_notice(&mut self, title: &str, detail: Option<&str>) {
            self.notices
                .push((title.to_owned(), detail.map(str::to_owned)));
        }

        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    #[tokio::test]
    async fn plan_toggle_updates_state_before_notice_and_includes_path() {
        let mut host = Host {
            plan_path: Some("C:/repo/PLAN.md".to_owned()),
            ..Host::default()
        };
        handle_plan_command(&mut host, "").await;
        assert_eq!(host.operations, ["set_plan:true", "update_plan:true"]);
        assert!(host.state.plan_mode);
        assert_eq!(
            host.notices,
            [(
                "Plan mode: ON".to_owned(),
                Some("Plan will be created here: C:/repo/PLAN.md".to_owned())
            )]
        );
    }

    #[tokio::test]
    async fn plan_clear_and_unknown_subcommand_keep_distinct_paths() {
        let mut host = Host::default();
        handle_plan_command(&mut host, "clear").await;
        handle_plan_command(&mut host, "wat").await;
        assert_eq!(host.operations, ["clear_plan"]);
        assert_eq!(host.notices[0].0, "Plan cleared");
        assert_eq!(host.errors, ["Unknown plan subcommand: wat"]);
    }

    #[tokio::test]
    async fn yolo_and_auto_preserve_on_off_and_toggle_semantics() {
        let mut host = Host::default();
        handle_yolo_command(&mut host, "on").await.expect("yolo on");
        handle_yolo_command(&mut host, "on")
            .await
            .expect("already on");
        handle_auto_command(&mut host, "unexpected")
            .await
            .expect("auto toggle");
        assert_eq!(host.state.permission_mode, PermissionMode::Auto);
        assert!(
            host.notices
                .iter()
                .any(|(title, _)| title == "YOLO mode is already on")
        );
    }

    #[tokio::test]
    async fn compact_trims_instruction_and_requires_session() {
        let mut host = Host::default();
        handle_compact_command(&mut host, "  keep decisions  ")
            .await
            .expect("compact");
        assert_eq!(host.operations, ["compact:keep decisions"]);
        host.state.has_session = false;
        handle_compact_command(&mut host, "again")
            .await
            .expect("no session status");
        assert_eq!(host.errors, [NO_ACTIVE_SESSION_MESSAGE]);
    }

    #[tokio::test]
    async fn plan_failure_does_not_update_local_state() {
        let mut host = Host {
            failure: Some("backend down".to_owned()),
            ..Host::default()
        };
        handle_plan_command(&mut host, "on").await;
        assert!(!host.state.plan_mode);
        assert_eq!(host.errors, ["Failed to set plan mode: backend down"]);
    }
}
