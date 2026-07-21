use async_trait::async_trait;

use crate::{
    sdk::types::PermissionMode,
    tui::{
        config::TuiConfig,
        theme::{colors::ResolvedTheme, custom_theme_loader::load_custom_theme_merged},
    },
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigStatusTone {
    Normal,
    Error,
}

#[async_trait(?Send)]
pub trait EditorThemeCommandHost {
    fn current_tui_config(&self) -> TuiConfig;
    async fn pick_editor(&mut self, current_value: &str) -> Option<String>;
    async fn pick_theme(&mut self, current_value: &str) -> Option<String>;
    async fn save_tui_config(&mut self, config: &TuiConfig) -> Result<(), String>;
    async fn apply_theme(
        &mut self,
        theme: &str,
        resolved: Option<ResolvedTheme>,
    ) -> Result<(), String>;
    fn resolved_auto_theme(&self) -> ResolvedTheme;
    fn update_editor_command(&mut self, command: Option<String>);
    fn refresh_terminal_theme_tracking(&mut self);
    fn track_theme_switch(&mut self, theme: &str);
    fn show_config_status(&mut self, message: &str, tone: ConfigStatusTone);
    fn show_config_error(&mut self, message: &str);
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

// Original: `handleEditorCommand()` and `showEditorPicker()`.
pub async fn handle_editor_command(host: &mut impl EditorThemeCommandHost, args: &str) {
    let command = args.trim();
    if command.is_empty() {
        let current = host.current_tui_config().editor_command.unwrap_or_default();
        if let Some(value) = host.pick_editor(&current).await {
            apply_editor_choice(host, &value).await;
        }
    } else {
        apply_editor_choice(host, command).await;
    }
}

// Original: `applyEditorChoice()`.
async fn apply_editor_choice(host: &mut impl EditorThemeCommandHost, value: &str) {
    let mut config = host.current_tui_config();
    let previous = config.editor_command.as_deref().unwrap_or_default();
    if !value.is_empty() && value == previous {
        host.show_config_status(
            &format!("Editor unchanged: {value}"),
            ConfigStatusTone::Normal,
        );
        return;
    }
    let editor_command = (!value.is_empty()).then(|| value.to_owned());
    config.editor_command.clone_from(&editor_command);
    if let Err(error) = host.save_tui_config(&config).await {
        host.show_config_status(
            &format!("Failed to save editor: {error}"),
            ConfigStatusTone::Error,
        );
        return;
    }
    host.update_editor_command(editor_command);
    host.show_config_status(
        if value.is_empty() {
            "Editor set to auto-detect ($VISUAL / $EDITOR).".to_owned()
        } else {
            format!("Editor set to \"{value}\".")
        }
        .as_str(),
        ConfigStatusTone::Normal,
    );
}

// Original: `handleThemeCommand()` and `showThemePicker()`.
pub async fn handle_theme_command(host: &mut impl EditorThemeCommandHost, args: &str) {
    let theme = args.trim();
    if theme.is_empty() {
        let current = host.current_tui_config().theme;
        if let Some(value) = host.pick_theme(&current).await {
            apply_theme_choice(host, &value).await;
        }
        return;
    }
    if !is_built_in_theme(theme) && load_custom_theme_merged(theme).await.is_none() {
        host.show_config_error(&format!("Unknown theme: {theme}"));
        return;
    }
    apply_theme_choice(host, theme).await;
}

const fn is_built_in_theme(theme: &str) -> bool {
    matches!(theme.as_bytes(), b"auto" | b"dark" | b"light")
}

// Original: `applyThemeChoice()`.
async fn apply_theme_choice(host: &mut impl EditorThemeCommandHost, theme: &str) {
    let mut config = host.current_tui_config();
    if theme == config.theme {
        if theme == "auto" {
            host.refresh_terminal_theme_tracking();
        }
        host.show_config_status(
            &format!("Theme unchanged: \"{theme}\"."),
            ConfigStatusTone::Normal,
        );
        return;
    }
    if !is_built_in_theme(theme) && load_custom_theme_merged(theme).await.is_none() {
        host.show_config_status(
            &format!("Theme \"{theme}\" could not be loaded."),
            ConfigStatusTone::Error,
        );
        return;
    }
    config.theme = theme.to_owned();
    if let Err(error) = host.save_tui_config(&config).await {
        host.show_config_status(
            &format!("Failed to save theme: {error}"),
            ConfigStatusTone::Error,
        );
        return;
    }
    let resolved = (theme == "auto").then(|| host.resolved_auto_theme());
    if let Err(error) = host.apply_theme(theme, resolved).await {
        host.show_config_error(&error);
        return;
    }
    host.refresh_terminal_theme_tracking();
    host.track_theme_switch(theme);
    let detail = resolved.map_or_else(String::new, |resolved| {
        format!(
            " (tracking terminal; current: {})",
            match resolved {
                ResolvedTheme::Dark => "dark",
                ResolvedTheme::Light => "light",
            }
        )
    });
    host.show_config_status(
        &format!("Theme set to \"{theme}\"{detail}."),
        ConfigStatusTone::Normal,
    );
}

#[cfg(test)]
mod tests {
    use crate::tui::config::{NotificationCondition, NotificationsConfig, UpgradePreferences};

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

    struct EditorThemeHost {
        config: TuiConfig,
        editor_pick: Option<String>,
        theme_pick: Option<String>,
        saved: Vec<TuiConfig>,
        applied: Vec<(String, Option<ResolvedTheme>)>,
        statuses: Vec<(String, ConfigStatusTone)>,
        errors: Vec<String>,
        refreshes: usize,
        tracked: Vec<String>,
        save_error: Option<String>,
    }

    impl Default for EditorThemeHost {
        fn default() -> Self {
            Self {
                config: TuiConfig {
                    theme: "dark".to_owned(),
                    disable_paste_burst: false,
                    editor_command: None,
                    notifications: NotificationsConfig {
                        enabled: true,
                        condition: NotificationCondition::Unfocused,
                    },
                    upgrade: UpgradePreferences { auto_install: true },
                },
                editor_pick: None,
                theme_pick: None,
                saved: Vec::new(),
                applied: Vec::new(),
                statuses: Vec::new(),
                errors: Vec::new(),
                refreshes: 0,
                tracked: Vec::new(),
                save_error: None,
            }
        }
    }

    #[async_trait(?Send)]
    impl EditorThemeCommandHost for EditorThemeHost {
        fn current_tui_config(&self) -> TuiConfig {
            self.config.clone()
        }

        async fn pick_editor(&mut self, _: &str) -> Option<String> {
            self.editor_pick.take()
        }

        async fn pick_theme(&mut self, _: &str) -> Option<String> {
            self.theme_pick.take()
        }

        async fn save_tui_config(&mut self, config: &TuiConfig) -> Result<(), String> {
            if let Some(error) = &self.save_error {
                return Err(error.clone());
            }
            self.saved.push(config.clone());
            self.config = config.clone();
            Ok(())
        }

        async fn apply_theme(
            &mut self,
            theme: &str,
            resolved: Option<ResolvedTheme>,
        ) -> Result<(), String> {
            self.applied.push((theme.to_owned(), resolved));
            Ok(())
        }

        fn resolved_auto_theme(&self) -> ResolvedTheme {
            ResolvedTheme::Light
        }

        fn update_editor_command(&mut self, command: Option<String>) {
            self.config.editor_command = command;
        }

        fn refresh_terminal_theme_tracking(&mut self) {
            self.refreshes += 1;
        }

        fn track_theme_switch(&mut self, theme: &str) {
            self.tracked.push(theme.to_owned());
        }

        fn show_config_status(&mut self, message: &str, tone: ConfigStatusTone) {
            self.statuses.push((message.to_owned(), tone));
        }

        fn show_config_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    #[tokio::test]
    async fn editor_argument_persists_before_updating_state() {
        let mut host = EditorThemeHost::default();
        handle_editor_command(&mut host, "  nvim  ").await;
        assert_eq!(host.config.editor_command.as_deref(), Some("nvim"));
        assert_eq!(host.saved.len(), 1);
        assert_eq!(host.statuses[0].0, "Editor set to \"nvim\".");
    }

    #[tokio::test]
    async fn editor_picker_can_restore_auto_detection() {
        let mut host = EditorThemeHost::default();
        host.config.editor_command = Some("vim".to_owned());
        host.editor_pick = Some(String::new());
        handle_editor_command(&mut host, "").await;
        assert_eq!(host.config.editor_command, None);
        assert!(host.statuses[0].0.contains("auto-detect"));
    }

    #[tokio::test]
    async fn auto_theme_applies_resolved_palette_then_tracks_terminal() {
        let mut host = EditorThemeHost::default();
        handle_theme_command(&mut host, "auto").await;
        assert_eq!(
            host.applied,
            [("auto".to_owned(), Some(ResolvedTheme::Light))]
        );
        assert_eq!(host.refreshes, 1);
        assert_eq!(host.tracked, ["auto"]);
        assert!(host.statuses[0].0.contains("current: light"));
    }

    #[tokio::test]
    async fn unknown_custom_theme_is_rejected_before_save() {
        let mut host = EditorThemeHost::default();
        handle_theme_command(&mut host, "definitely-missing-theme").await;
        assert_eq!(host.errors, ["Unknown theme: definitely-missing-theme"]);
        assert!(host.saved.is_empty());
    }
}
