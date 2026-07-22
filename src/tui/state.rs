use std::collections::BTreeMap;

use crate::{
    cli::options::CliOptions,
    sdk::types::{PermissionMode, ThinkingEffort},
    tui::{
        components::editor::InputMode,
        config::TuiConfig,
        types::{AppState, StreamingPhase},
    },
};

/// Inputs used before a TUI session or model has been selected.
#[derive(Debug, Clone, Copy)]
pub struct InitialAppStateInput<'a> {
    pub cli_options: &'a CliOptions,
    pub additional_dirs: &'a [String],
    pub tui_config: &'a TuiConfig,
    pub version: &'a str,
    pub work_dir: &'a str,
}

/// Builds the complete initial state presented while TUI startup is pending.
///
/// Original:
///   apps/kimi-code/src/tui/kimi-tui.ts
///   createInitialAppState()
pub fn create_initial_app_state(input: InitialAppStateInput<'_>) -> AppState {
    let permission_mode = if input.cli_options.auto {
        PermissionMode::Auto
    } else if input.cli_options.yolo {
        PermissionMode::Yolo
    } else {
        PermissionMode::Manual
    };

    AppState {
        model: String::new(),
        work_dir: input.work_dir.to_owned(),
        additional_dirs: input.additional_dirs.to_vec(),
        session_id: String::new(),
        permission_mode,
        plan_mode: input.cli_options.plan,
        input_mode: InputMode::Prompt,
        swarm_mode: false,
        thinking_effort: ThinkingEffort::from("off"),
        context_usage: 0.0,
        context_tokens: 0,
        max_context_tokens: 0,
        is_compacting: false,
        is_replaying: false,
        streaming_phase: StreamingPhase::Idle,
        streaming_start_time_ms: 0,
        theme: input.tui_config.theme.clone(),
        version: input.version.to_owned(),
        editor_command: input.tui_config.editor_command.clone(),
        disable_paste_burst: Some(input.tui_config.disable_paste_burst),
        notifications: input.tui_config.notifications.clone(),
        upgrade: input.tui_config.upgrade.clone(),
        available_models: BTreeMap::new(),
        available_providers: BTreeMap::new(),
        session_title: None,
        goal: None,
        mcp_servers_summary: None,
        banner: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_for(options: &CliOptions, config: &TuiConfig) -> AppState {
        create_initial_app_state(InitialAppStateInput {
            cli_options: options,
            additional_dirs: &["D:/shared".to_owned(), "D:/other".to_owned()],
            tui_config: config,
            version: "1.2.3",
            work_dir: "D:/project",
        })
    }

    #[test]
    fn creates_pending_session_values_and_copies_tui_configuration() {
        let options = CliOptions {
            plan: true,
            ..CliOptions::default()
        };
        let config = TuiConfig {
            theme: "dark".to_owned(),
            disable_paste_burst: true,
            editor_command: Some("code --wait".to_owned()),
            ..TuiConfig::default()
        };

        let state = state_for(&options, &config);

        assert_eq!(state.model, "");
        assert_eq!(state.work_dir, "D:/project");
        assert_eq!(state.additional_dirs, ["D:/shared", "D:/other"]);
        assert_eq!(state.session_id, "");
        assert_eq!(state.permission_mode, PermissionMode::Manual);
        assert!(state.plan_mode);
        assert_eq!(state.input_mode, InputMode::Prompt);
        assert!(!state.swarm_mode);
        assert_eq!(state.thinking_effort.as_str(), "off");
        assert_eq!(state.streaming_phase, StreamingPhase::Idle);
        assert_eq!(state.theme, "dark");
        assert_eq!(state.version, "1.2.3");
        assert_eq!(state.editor_command.as_deref(), Some("code --wait"));
        assert_eq!(state.disable_paste_burst, Some(true));
        assert_eq!(state.notifications, config.notifications);
        assert_eq!(state.upgrade, config.upgrade);
        assert!(state.available_models.is_empty());
        assert!(state.available_providers.is_empty());
        assert!(state.session_title.is_none());
        assert!(state.goal.is_none());
        assert!(state.mcp_servers_summary.is_none());
        assert!(state.banner.is_none());
    }

    #[test]
    fn auto_permission_takes_precedence_over_yolo() {
        let options = CliOptions {
            auto: true,
            yolo: true,
            ..CliOptions::default()
        };
        assert_eq!(
            state_for(&options, &TuiConfig::default()).permission_mode,
            PermissionMode::Auto
        );
    }

    #[test]
    fn yolo_is_used_when_auto_is_disabled() {
        let options = CliOptions {
            yolo: true,
            ..CliOptions::default()
        };
        assert_eq!(
            state_for(&options, &TuiConfig::default()).permission_mode,
            PermissionMode::Yolo
        );
    }
}
