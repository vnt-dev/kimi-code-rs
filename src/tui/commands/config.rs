use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use indexmap::IndexMap;
use tokio::sync::oneshot;

use crate::{
    sdk::{
        model_alias::{ModelAlias, ModelProtocol, ProviderType, effective_model_alias},
        types::{PermissionMode, ThinkingEffort},
    },
    tui::{
        commands::experimental_flags::{ExperimentalFeatureState, set_experimental_features},
        components::dialogs::{
            ExperimentalFeatureDraftChange, SettingsSelection,
            model_selector::{ModelSelection, model_display_name, segments_for},
        },
        config::TuiConfig,
        theme::{colors::ResolvedTheme, custom_theme_loader::load_custom_theme_merged},
        utils::thinking_config::{ThinkingConfig, ThinkingConfigPatch, thinking_effort_to_config},
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

const MODEL_PICKER_REFRESH_TIMEOUT: Duration = Duration::from_millis(2_000);
const MODEL_SWITCH_CACHE_WARNING: &str = "Note: Switching models invalidates the existing prompt cache. Use /new to avoid extra token costs.";
const EFFORT_SWITCH_CACHE_WARNING: &str = "Note: Switching effort invalidates the existing prompt cache. Use /new to avoid extra token costs.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCommandState {
    pub model: String,
    pub thinking_effort: ThinkingEffort,
    pub available_models: IndexMap<String, ModelAlias>,
    pub provider_types: BTreeMap<String, ProviderType>,
    pub streaming: bool,
    pub has_session: bool,
    pub has_conversation_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RefreshModelsResult {
    pub failed: Vec<RefreshModelFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshModelFailure {
    pub provider: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPickerRequest {
    pub models: IndexMap<String, ModelAlias>,
    pub current_value: String,
    pub selected_value: String,
    pub current_thinking_effort: ThinkingEffort,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortPickerRequest {
    pub efforts: Vec<ThinkingEffort>,
    pub current_value: ThinkingEffort,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedModelConfig {
    pub default_model: Option<String>,
    pub thinking: Option<ThinkingConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModelStatus {
    pub model: Option<String>,
    pub thinking_effort: ThinkingEffort,
}

#[async_trait(?Send)]
pub trait ModelCommandHost {
    fn model_command_state(&self) -> ModelCommandState;
    fn start_model_refresh(&mut self) -> oneshot::Receiver<Result<RefreshModelsResult, String>>;
    async fn pick_model(&mut self, request: ModelPickerRequest) -> Option<(ModelSelection, bool)>;
    async fn pick_effort(&mut self, request: EffortPickerRequest)
    -> Option<(ThinkingEffort, bool)>;
    async fn activate_model_after_login(
        &mut self,
        alias: &str,
        effort: &ThinkingEffort,
    ) -> Result<(), String>;
    async fn set_session_model(&mut self, alias: &str) -> Result<(), String>;
    async fn set_session_thinking(&mut self, effort: &ThinkingEffort) -> Result<(), String>;
    async fn get_runtime_model_status(&self) -> Result<RuntimeModelStatus, String>;
    async fn get_persisted_model_config(&self) -> Result<PersistedModelConfig, String>;
    async fn persist_model_config(
        &mut self,
        alias: &str,
        thinking: ThinkingConfigPatch,
    ) -> Result<(), String>;
    fn update_model_state(&mut self, alias: &str, effort: &ThinkingEffort);
    fn track_model_switch(&mut self, alias: &str);
    fn track_thinking_toggle(
        &mut self,
        enabled: bool,
        effort: &ThinkingEffort,
        previous: &ThinkingEffort,
    );
    fn show_model_notice(&mut self, title: &str, detail: Option<&str>);
    fn show_model_status(&mut self, message: &str, warning: bool);
    fn show_model_error(&mut self, message: &str);
}

#[async_trait(?Send)]
pub trait SettingsCommandHost {
    fn settings_tui_config(&self) -> TuiConfig;
    fn current_permission_mode(&self) -> PermissionMode;
    fn has_active_session(&self) -> bool;
    async fn pick_permission(&mut self, current: PermissionMode) -> Option<PermissionMode>;
    async fn pick_update_preference(&mut self, current: bool) -> Option<bool>;
    async fn pick_setting(&mut self) -> Option<SettingsSelection>;
    async fn get_experimental_features(&self) -> Result<Vec<ExperimentalFeatureState>, String>;
    async fn pick_experimental_changes(
        &mut self,
        features: Vec<ExperimentalFeatureState>,
    ) -> Option<Vec<ExperimentalFeatureDraftChange>>;
    async fn set_experimental_config(
        &mut self,
        changes: &BTreeMap<String, bool>,
    ) -> Result<(), String>;
    async fn set_settings_permission(&mut self, mode: PermissionMode) -> Result<(), String>;
    async fn save_settings_tui_config(&mut self, config: &TuiConfig) -> Result<(), String>;
    async fn reload_session_after_experiments(&mut self, message: &str) -> Result<(), String>;
    fn update_settings_permission(&mut self, mode: PermissionMode);
    fn update_upgrade_preference(&mut self, auto_install: bool);
    fn refresh_slash_command_autocomplete(&mut self);
    fn restore_settings_editor(&mut self);
    fn track_settings(&mut self, event: &str, value: &str);
    fn show_settings_status(&mut self, message: &str, muted: bool);
    fn show_settings_notice(&mut self, message: &str);
    fn show_settings_error(&mut self, message: &str);

    async fn open_model_setting(&mut self);
    async fn open_theme_setting(&mut self);
    async fn open_editor_setting(&mut self);
    async fn open_experiments_setting(&mut self);
    async fn open_usage_setting(&mut self);
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

pub fn effective_model_for_state(state: &ModelCommandState, model: &ModelAlias) -> ModelAlias {
    effective_model_alias(model, state.provider_types.get(&model.provider).copied())
}

// Original: `handleModelCommand()`.
pub async fn handle_model_command(host: &mut impl ModelCommandHost, args: &str) {
    refresh_models_for_picker(host).await;
    let alias = args.trim();
    let state = host.model_command_state();
    if !alias.is_empty() && !state.available_models.contains_key(alias) {
        host.show_model_error(&format!("Unknown model alias: {alias}"));
        return;
    }
    show_model_picker(
        host,
        if alias.is_empty() {
            state.model
        } else {
            alias.to_owned()
        },
    )
    .await;
}

// Original: `refreshModelsForPicker()` and `withTimeout()`.
async fn refresh_models_for_picker(host: &mut impl ModelCommandHost) {
    let receiver = host.start_model_refresh();
    match tokio::time::timeout(MODEL_PICKER_REFRESH_TIMEOUT, receiver).await {
        Ok(Ok(Ok(result))) => {
            for failure in result.failed {
                host.show_model_status(
                    &format!(
                        "Skipped refreshing {}: {}",
                        failure.provider, failure.reason
                    ),
                    true,
                );
            }
        }
        Ok(Ok(Err(error))) => {
            host.show_model_status(&format!("Skipped refreshing models: {error}"), true);
        }
        Ok(Err(_)) | Err(_) => {}
    }
}

// Original: `showModelPicker()`.
pub async fn show_model_picker(host: &mut impl ModelCommandHost, selected_value: String) {
    let state = host.model_command_state();
    let models = state
        .available_models
        .iter()
        .map(|(alias, model)| (alias.clone(), effective_model_for_state(&state, model)))
        .collect::<IndexMap<_, _>>();
    if models.is_empty() {
        host.show_model_notice(
            "No models configured",
            Some(
                "Run /login to sign in to Kimi, or /provider to add another provider from a model catalog.",
            ),
        );
        return;
    }
    let request = ModelPickerRequest {
        models,
        current_value: state.model,
        selected_value,
        current_thinking_effort: state.thinking_effort,
        warning: state
            .has_conversation_history
            .then(|| MODEL_SWITCH_CACHE_WARNING.to_owned()),
    };
    if let Some((selection, persist)) = host.pick_model(request).await {
        perform_model_switch(host, &selection.alias, &selection.thinking, persist).await;
    }
}

// Original: `handleEffortCommand()` and `showEffortPicker()`.
pub async fn handle_effort_command(host: &mut impl ModelCommandHost, args: &str) {
    let state = host.model_command_state();
    let Some(model) = state.available_models.get(&state.model) else {
        host.show_model_error("No model selected. Run /model to select one first.");
        return;
    };
    let effective = effective_model_for_state(&state, model);
    let segments = segments_for(&effective);
    let argument = args.trim().to_lowercase();
    if argument.is_empty() {
        let current = if segments
            .iter()
            .any(|effort| effort == state.thinking_effort.as_str())
        {
            state.thinking_effort.clone()
        } else {
            ThinkingEffort::new(
                segments
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "off".to_owned()),
            )
        };
        let request = EffortPickerRequest {
            efforts: segments.iter().map(ThinkingEffort::new).collect(),
            current_value: current,
            warning: state
                .has_conversation_history
                .then(|| EFFORT_SWITCH_CACHE_WARNING.to_owned()),
        };
        if let Some((effort, persist)) = host.pick_effort(request).await {
            perform_model_switch(host, &state.model, &effort, persist).await;
        }
        return;
    }
    if !segments.iter().any(|effort| effort == &argument) {
        if effective.protocol != Some(ModelProtocol::Anthropic) {
            host.show_model_error(&format!(
                "Unsupported thinking effort \"{argument}\" for {}. Available: {}",
                state.model,
                segments.join(", ")
            ));
            return;
        }
        host.show_model_status(
            &format!(
                "Thinking effort \"{argument}\" is not listed for {} (known: {}). Sending \"{argument}\" unchanged; the configured provider will validate it.",
                state.model,
                effective
                    .support_efforts
                    .as_ref()
                    .map(|efforts| efforts.join(", "))
                    .unwrap_or_else(|| "none declared".to_owned())
            ),
            true,
        );
    }
    perform_model_switch(host, &state.model, &ThinkingEffort::new(argument), true).await;
}

// Original: `performModelSwitch()`.
pub async fn perform_model_switch(
    host: &mut impl ModelCommandHost,
    alias: &str,
    effort: &ThinkingEffort,
    persist: bool,
) {
    let before = host.model_command_state();
    if before.streaming {
        host.show_model_error("Cannot switch models while streaming — press Esc or Ctrl-C first.");
        return;
    }
    let model_changed = alias != before.model;
    let effort_changed = effort != &before.thinking_effort;
    let runtime_changed = model_changed || effort_changed;
    let mut effective_alias = alias.to_owned();
    let mut effective_effort = effort.clone();

    let runtime_result = if !before.has_session && runtime_changed {
        host.activate_model_after_login(alias, effort).await
    } else if before.has_session {
        if model_changed && let Err(error) = host.set_session_model(alias).await {
            Err(error)
        } else if effort_changed && let Err(error) = host.set_session_thinking(effort).await {
            Err(error)
        } else {
            match host.get_runtime_model_status().await {
                Ok(status) => {
                    effective_alias = status.model.unwrap_or_else(|| alias.to_owned());
                    effective_effort = status.thinking_effort;
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
    } else {
        Ok(())
    };
    if let Err(error) = runtime_result {
        host.show_model_error(&format!("Failed to switch model: {error}"));
        return;
    }
    if !before.has_session {
        let after_activation = host.model_command_state();
        effective_alias = after_activation.model;
        effective_effort = after_activation.thinking_effort;
    }

    let effective_model_changed = effective_alias != before.model;
    let effective_effort_changed = effective_effort != before.thinking_effort;
    let display_name = model_display_name(
        &effective_alias,
        before.available_models.get(&effective_alias),
    );
    host.update_model_state(&effective_alias, &effective_effort);
    if !before.has_session && runtime_changed {
        if effective_model_changed {
            host.track_model_switch(&effective_alias);
        }
        if effective_effort_changed {
            host.track_thinking_toggle(
                effective_effort.as_str() != "off",
                &effective_effort,
                &before.thinking_effort,
            );
        }
    }

    let persisted = if persist {
        match persist_model_selection(
            host,
            &before,
            &effective_alias,
            &effective_effort,
            effective_effort_changed,
        )
        .await
        {
            Ok(persisted) => persisted,
            Err(error) => {
                host.show_model_error(&format!(
                    "Switched to {display_name}, but failed to save default: {error}"
                ));
                return;
            }
        }
    } else {
        false
    };
    let message = if effective_model_changed {
        if persist {
            format!(
                "Switched to {display_name} with thinking {}.",
                effective_effort.as_str()
            )
        } else {
            format!(
                "Switched to {display_name} with thinking {} for this session only.",
                effective_effort.as_str()
            )
        }
    } else if effective_effort_changed {
        if persist {
            format!("Thinking set to {}.", effective_effort.as_str())
        } else {
            format!(
                "Thinking set to {} for this session only.",
                effective_effort.as_str()
            )
        }
    } else if persist && persisted {
        format!(
            "Saved {display_name} with thinking {} as default.",
            effective_effort.as_str()
        )
    } else {
        format!(
            "Already using {display_name} with thinking {}.",
            effective_effort.as_str()
        )
    };
    host.show_model_status(&message, false);
}

// Original: `persistModelSelection()`.
async fn persist_model_selection(
    host: &mut impl ModelCommandHost,
    state: &ModelCommandState,
    alias: &str,
    effort: &ThinkingEffort,
    effort_changed: bool,
) -> Result<bool, String> {
    let config = host.get_persisted_model_config().await?;
    let supported = state
        .available_models
        .get(alias)
        .map(|model| effective_model_for_state(state, model))
        .and_then(|model| model.support_efforts);
    let full = thinking_effort_to_config(effort, supported.as_deref());
    let patch = if effort_changed {
        full
    } else {
        ThinkingConfigPatch {
            enabled: full.enabled,
            effort: None,
        }
    };
    let same = config.default_model.as_deref() == Some(alias)
        && config.thinking.as_ref().and_then(|value| value.enabled) == Some(patch.enabled)
        && (!effort_changed
            || config
                .thinking
                .as_ref()
                .and_then(|value| value.effort.as_deref())
                == patch.effort.as_deref());
    if same {
        return Ok(false);
    }
    host.persist_model_config(alias, patch).await?;
    Ok(true)
}

// Original: `showPermissionPicker()` and `applyPermissionChoice()`.
pub async fn show_permission_picker(host: &mut impl SettingsCommandHost) {
    let current = host.current_permission_mode();
    let Some(mode) = host.pick_permission(current).await else {
        return;
    };
    if mode == current {
        host.show_settings_status(
            &format!("Permission mode unchanged: {}.", permission_label(mode)),
            false,
        );
        return;
    }
    if let Err(error) = host.set_settings_permission(mode).await {
        host.show_settings_error(&format!("Failed to set permission mode: {error}"));
        return;
    }
    host.update_settings_permission(mode);
    host.show_settings_notice(&format!("Permission mode: {}", permission_label(mode)));
}

const fn permission_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Manual => "manual",
        PermissionMode::Yolo => "yolo",
        PermissionMode::Auto => "auto",
    }
}

// Original: `showUpdatePreferencePicker()` and
// `applyUpdatePreferenceChoice()`.
pub async fn show_update_preference_picker(host: &mut impl SettingsCommandHost) {
    let current = host.settings_tui_config().upgrade.auto_install;
    if let Some(value) = host.pick_update_preference(current).await {
        apply_update_preference_choice(host, value).await;
    }
}

pub async fn apply_update_preference_choice(
    host: &mut impl SettingsCommandHost,
    auto_install: bool,
) {
    let mut config = host.settings_tui_config();
    if auto_install == config.upgrade.auto_install {
        host.show_settings_status(
            &format!(
                "Automatic updates already {}.",
                if auto_install { "enabled" } else { "disabled" }
            ),
            false,
        );
        return;
    }
    config.upgrade.auto_install = auto_install;
    if let Err(error) = host.save_settings_tui_config(&config).await {
        host.show_settings_error(&format!("Failed to save automatic update setting: {error}"));
        return;
    }
    host.update_upgrade_preference(auto_install);
    host.track_settings("upgrade_preference_changed", &auto_install.to_string());
    host.show_settings_status(
        &format!(
            "Automatic updates {}.",
            if auto_install { "enabled" } else { "disabled" }
        ),
        false,
    );
}

// Original: `showExperimentsPanel()`.
pub async fn show_experiments_panel(host: &mut impl SettingsCommandHost) {
    let features = match host.get_experimental_features().await {
        Ok(features) => features,
        Err(error) => {
            host.show_settings_error(&format!("Failed to load experimental features: {error}"));
            return;
        }
    };
    if let Some(changes) = host.pick_experimental_changes(features).await {
        apply_experimental_feature_changes(host, &changes).await;
    }
}

// Original: `applyExperimentalFeatureChanges()`.
pub async fn apply_experimental_feature_changes(
    host: &mut impl SettingsCommandHost,
    changes: &[ExperimentalFeatureDraftChange],
) {
    if changes.is_empty() {
        host.show_settings_status("No experimental feature changes to apply.", true);
        return;
    }
    let patch = changes
        .iter()
        .map(|change| (change.id.clone(), change.enabled))
        .collect::<BTreeMap<_, _>>();
    if let Err(error) = host.set_experimental_config(&patch).await {
        host.show_settings_error(&format!("Failed to update experimental features: {error}"));
        return;
    }
    let features = match host.get_experimental_features().await {
        Ok(features) => features,
        Err(error) => {
            host.show_settings_error(&format!("Failed to update experimental features: {error}"));
            return;
        }
    };
    set_experimental_features(&features);
    host.refresh_slash_command_autocomplete();
    host.restore_settings_editor();
    if host.has_active_session() {
        if let Err(error) = host
            .reload_session_after_experiments("Experimental features updated. Session reloaded.")
            .await
        {
            host.show_settings_error(&format!("Failed to update experimental features: {error}"));
            return;
        }
    } else {
        host.show_settings_status("Experimental features updated.", false);
    }
    host.track_settings("experimental_features_apply", &changes.len().to_string());
}

// Original: `showSettingsSelector()` and `handleSettingsSelection()`.
pub async fn show_settings_selector(host: &mut impl SettingsCommandHost) {
    let Some(selection) = host.pick_setting().await else {
        return;
    };
    host.restore_settings_editor();
    match selection {
        SettingsSelection::Model => host.open_model_setting().await,
        SettingsSelection::Permission => show_permission_picker(host).await,
        SettingsSelection::Theme => host.open_theme_setting().await,
        SettingsSelection::Editor => host.open_editor_setting().await,
        SettingsSelection::Experiments => host.open_experiments_setting().await,
        SettingsSelection::Upgrade => show_update_preference_picker(host).await,
        SettingsSelection::Usage => host.open_usage_setting().await,
    }
}

#[cfg(test)]
mod tests {
    use crate::tui::commands::experimental_flags::{ExperimentalFlagSource, FlagSurface};
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

    struct ModelHost {
        state: ModelCommandState,
        refresh: Option<Result<RefreshModelsResult, String>>,
        model_pick: Option<(ModelSelection, bool)>,
        effort_pick: Option<(ThinkingEffort, bool)>,
        runtime_status: RuntimeModelStatus,
        persisted: PersistedModelConfig,
        operations: Vec<String>,
        notices: Vec<(String, Option<String>)>,
        statuses: Vec<(String, bool)>,
        errors: Vec<String>,
        persisted_patches: Vec<(String, ThinkingConfigPatch)>,
    }

    fn model_alias(provider: &str, model: &str, protocol: Option<ModelProtocol>) -> ModelAlias {
        ModelAlias {
            provider: provider.to_owned(),
            model: model.to_owned(),
            max_context_size: 128_000,
            max_output_size: None,
            capabilities: None,
            display_name: Some(model.to_owned()),
            reasoning_key: None,
            protocol,
            adaptive_thinking: None,
            support_efforts: None,
            default_effort: None,
            beta_api: None,
            overrides: None,
        }
    }

    impl Default for ModelHost {
        fn default() -> Self {
            let alias = model_alias("provider", "first", None);
            Self {
                state: ModelCommandState {
                    model: "provider/first".to_owned(),
                    thinking_effort: ThinkingEffort::from("off"),
                    available_models: IndexMap::from([("provider/first".to_owned(), alias)]),
                    provider_types: BTreeMap::from([("provider".to_owned(), ProviderType::Openai)]),
                    streaming: false,
                    has_session: true,
                    has_conversation_history: false,
                },
                refresh: Some(Ok(RefreshModelsResult::default())),
                model_pick: None,
                effort_pick: None,
                runtime_status: RuntimeModelStatus {
                    model: Some("provider/first".to_owned()),
                    thinking_effort: ThinkingEffort::from("off"),
                },
                persisted: PersistedModelConfig {
                    default_model: None,
                    thinking: None,
                },
                operations: Vec::new(),
                notices: Vec::new(),
                statuses: Vec::new(),
                errors: Vec::new(),
                persisted_patches: Vec::new(),
            }
        }
    }

    #[async_trait(?Send)]
    impl ModelCommandHost for ModelHost {
        fn model_command_state(&self) -> ModelCommandState {
            self.state.clone()
        }

        fn start_model_refresh(
            &mut self,
        ) -> oneshot::Receiver<Result<RefreshModelsResult, String>> {
            let (sender, receiver) = oneshot::channel();
            if let Some(result) = self.refresh.take() {
                let _ = sender.send(result);
            } else {
                std::mem::forget(sender);
            }
            receiver
        }

        async fn pick_model(
            &mut self,
            request: ModelPickerRequest,
        ) -> Option<(ModelSelection, bool)> {
            self.operations
                .push(format!("pick_model:{}", request.selected_value));
            self.model_pick.take()
        }

        async fn pick_effort(
            &mut self,
            request: EffortPickerRequest,
        ) -> Option<(ThinkingEffort, bool)> {
            self.operations
                .push(format!("pick_effort:{}", request.current_value.as_str()));
            self.effort_pick.take()
        }

        async fn activate_model_after_login(
            &mut self,
            alias: &str,
            effort: &ThinkingEffort,
        ) -> Result<(), String> {
            self.operations
                .push(format!("activate:{alias}:{}", effort.as_str()));
            self.state.model = alias.to_owned();
            self.state.thinking_effort = effort.clone();
            Ok(())
        }

        async fn set_session_model(&mut self, alias: &str) -> Result<(), String> {
            self.operations.push(format!("set_model:{alias}"));
            Ok(())
        }

        async fn set_session_thinking(&mut self, effort: &ThinkingEffort) -> Result<(), String> {
            self.operations
                .push(format!("set_thinking:{}", effort.as_str()));
            Ok(())
        }

        async fn get_runtime_model_status(&self) -> Result<RuntimeModelStatus, String> {
            Ok(self.runtime_status.clone())
        }

        async fn get_persisted_model_config(&self) -> Result<PersistedModelConfig, String> {
            Ok(self.persisted.clone())
        }

        async fn persist_model_config(
            &mut self,
            alias: &str,
            thinking: ThinkingConfigPatch,
        ) -> Result<(), String> {
            self.persisted_patches.push((alias.to_owned(), thinking));
            Ok(())
        }

        fn update_model_state(&mut self, alias: &str, effort: &ThinkingEffort) {
            self.state.model = alias.to_owned();
            self.state.thinking_effort = effort.clone();
            self.operations
                .push(format!("update:{alias}:{}", effort.as_str()));
        }

        fn track_model_switch(&mut self, alias: &str) {
            self.operations.push(format!("track_model:{alias}"));
        }

        fn track_thinking_toggle(
            &mut self,
            enabled: bool,
            effort: &ThinkingEffort,
            previous: &ThinkingEffort,
        ) {
            self.operations.push(format!(
                "track_thinking:{enabled}:{}:{}",
                effort.as_str(),
                previous.as_str()
            ));
        }

        fn show_model_notice(&mut self, title: &str, detail: Option<&str>) {
            self.notices
                .push((title.to_owned(), detail.map(str::to_owned)));
        }

        fn show_model_status(&mut self, message: &str, warning: bool) {
            self.statuses.push((message.to_owned(), warning));
        }

        fn show_model_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    #[tokio::test]
    async fn model_command_reports_refresh_failures_before_opening_selected_alias() {
        let mut host = ModelHost {
            refresh: Some(Ok(RefreshModelsResult {
                failed: vec![RefreshModelFailure {
                    provider: "slow".to_owned(),
                    reason: "timeout".to_owned(),
                }],
            })),
            ..ModelHost::default()
        };
        handle_model_command(&mut host, "provider/first").await;
        assert_eq!(
            host.statuses[0],
            ("Skipped refreshing slow: timeout".to_owned(), true)
        );
        assert_eq!(host.operations, ["pick_model:provider/first"]);
    }

    #[tokio::test]
    async fn model_picker_empty_state_shows_login_provider_guidance() {
        let mut host = ModelHost::default();
        host.state.available_models.clear();
        show_model_picker(&mut host, String::new()).await;
        assert_eq!(host.notices[0].0, "No models configured");
        assert!(
            host.notices[0]
                .1
                .as_deref()
                .is_some_and(|text| text.contains("/provider"))
        );
    }

    #[tokio::test]
    async fn non_anthropic_unknown_effort_is_rejected() {
        let mut host = ModelHost::default();
        handle_effort_command(&mut host, "ultra").await;
        assert!(host.errors[0].contains("Unsupported thinking effort \"ultra\""));
        assert!(host.persisted_patches.is_empty());
    }

    #[tokio::test]
    async fn anthropic_unknown_effort_is_forwarded_and_persisted() {
        let mut host = ModelHost::default();
        host.state
            .provider_types
            .insert("provider".to_owned(), ProviderType::Anthropic);
        host.state.available_models.insert(
            "provider/first".to_owned(),
            model_alias("provider", "custom-model", Some(ModelProtocol::Anthropic)),
        );
        host.runtime_status.thinking_effort = ThinkingEffort::from("ultra");
        handle_effort_command(&mut host, "ultra").await;
        assert!(
            host.statuses
                .iter()
                .any(|(message, warning)| *warning
                    && message.contains("Sending \"ultra\" unchanged"))
        );
        assert_eq!(host.persisted_patches[0].1.effort.as_deref(), Some("ultra"));
    }

    #[tokio::test]
    async fn streaming_blocks_switch_before_runtime_or_persistence() {
        let mut host = ModelHost::default();
        host.state.streaming = true;
        perform_model_switch(
            &mut host,
            "provider/other",
            &ThinkingEffort::from("on"),
            true,
        )
        .await;
        assert!(host.errors[0].contains("Cannot switch models while streaming"));
        assert!(host.operations.is_empty());
    }

    struct SettingsHost {
        config: TuiConfig,
        permission: PermissionMode,
        permission_pick: Option<PermissionMode>,
        setting_pick: Option<SettingsSelection>,
        features: Vec<ExperimentalFeatureState>,
        active_session: bool,
        operations: Vec<String>,
        statuses: Vec<(String, bool)>,
        notices: Vec<String>,
        errors: Vec<String>,
    }

    impl Default for SettingsHost {
        fn default() -> Self {
            Self {
                config: EditorThemeHost::default().config,
                permission: PermissionMode::Manual,
                permission_pick: None,
                setting_pick: None,
                features: vec![ExperimentalFeatureState {
                    id: "feature".to_owned(),
                    title: "Feature".to_owned(),
                    description: String::new(),
                    surface: FlagSurface::Tui,
                    env: "KIMI_CODE_FEATURE".to_owned(),
                    default_enabled: false,
                    enabled: true,
                    source: ExperimentalFlagSource::Config,
                    config_value: Some(true),
                }],
                active_session: false,
                operations: Vec::new(),
                statuses: Vec::new(),
                notices: Vec::new(),
                errors: Vec::new(),
            }
        }
    }

    #[async_trait(?Send)]
    impl SettingsCommandHost for SettingsHost {
        fn settings_tui_config(&self) -> TuiConfig {
            self.config.clone()
        }
        fn current_permission_mode(&self) -> PermissionMode {
            self.permission
        }
        fn has_active_session(&self) -> bool {
            self.active_session
        }
        async fn pick_permission(&mut self, _: PermissionMode) -> Option<PermissionMode> {
            self.permission_pick.take()
        }
        async fn pick_update_preference(&mut self, _: bool) -> Option<bool> {
            None
        }
        async fn pick_setting(&mut self) -> Option<SettingsSelection> {
            self.setting_pick.take()
        }
        async fn get_experimental_features(&self) -> Result<Vec<ExperimentalFeatureState>, String> {
            Ok(self.features.clone())
        }
        async fn pick_experimental_changes(
            &mut self,
            _: Vec<ExperimentalFeatureState>,
        ) -> Option<Vec<ExperimentalFeatureDraftChange>> {
            None
        }
        async fn set_experimental_config(
            &mut self,
            changes: &BTreeMap<String, bool>,
        ) -> Result<(), String> {
            self.operations.push(format!("set_experiments:{changes:?}"));
            Ok(())
        }
        async fn set_settings_permission(&mut self, mode: PermissionMode) -> Result<(), String> {
            self.operations.push(format!("set_permission:{mode:?}"));
            Ok(())
        }
        async fn save_settings_tui_config(&mut self, config: &TuiConfig) -> Result<(), String> {
            self.config = config.clone();
            self.operations.push("save_tui".to_owned());
            Ok(())
        }
        async fn reload_session_after_experiments(&mut self, message: &str) -> Result<(), String> {
            self.operations.push(format!("reload:{message}"));
            Ok(())
        }
        fn update_settings_permission(&mut self, mode: PermissionMode) {
            self.permission = mode;
            self.operations.push(format!("update_permission:{mode:?}"));
        }
        fn update_upgrade_preference(&mut self, auto_install: bool) {
            self.config.upgrade.auto_install = auto_install;
            self.operations
                .push(format!("update_upgrade:{auto_install}"));
        }
        fn refresh_slash_command_autocomplete(&mut self) {
            self.operations.push("refresh_autocomplete".to_owned());
        }
        fn restore_settings_editor(&mut self) {
            self.operations.push("restore_editor".to_owned());
        }
        fn track_settings(&mut self, event: &str, value: &str) {
            self.operations.push(format!("track:{event}:{value}"));
        }
        fn show_settings_status(&mut self, message: &str, muted: bool) {
            self.statuses.push((message.to_owned(), muted));
        }
        fn show_settings_notice(&mut self, message: &str) {
            self.notices.push(message.to_owned());
        }
        fn show_settings_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
        async fn open_model_setting(&mut self) {
            self.operations.push("open_model".to_owned());
        }
        async fn open_theme_setting(&mut self) {
            self.operations.push("open_theme".to_owned());
        }
        async fn open_editor_setting(&mut self) {
            self.operations.push("open_editor".to_owned());
        }
        async fn open_experiments_setting(&mut self) {
            self.operations.push("open_experiments".to_owned());
        }
        async fn open_usage_setting(&mut self) {
            self.operations.push("open_usage".to_owned());
        }
    }

    #[tokio::test]
    async fn permission_picker_applies_backend_before_local_state() {
        let mut host = SettingsHost {
            permission_pick: Some(PermissionMode::Yolo),
            ..SettingsHost::default()
        };
        show_permission_picker(&mut host).await;
        assert_eq!(
            host.operations,
            ["set_permission:Yolo", "update_permission:Yolo"]
        );
        assert_eq!(host.permission, PermissionMode::Yolo);
        assert_eq!(host.notices, ["Permission mode: yolo"]);
    }

    #[tokio::test]
    async fn update_preference_saves_tracks_and_updates_state() {
        let mut host = SettingsHost::default();
        apply_update_preference_choice(&mut host, false).await;
        assert!(!host.config.upgrade.auto_install);
        assert_eq!(
            host.operations,
            [
                "save_tui",
                "update_upgrade:false",
                "track:upgrade_preference_changed:false"
            ]
        );
        assert_eq!(host.statuses[0].0, "Automatic updates disabled.");
    }

    #[tokio::test]
    async fn experiments_refresh_flags_and_reload_active_session_in_order() {
        let mut host = SettingsHost {
            active_session: true,
            ..SettingsHost::default()
        };
        apply_experimental_feature_changes(
            &mut host,
            &[ExperimentalFeatureDraftChange {
                id: "feature".to_owned(),
                enabled: true,
            }],
        )
        .await;
        assert_eq!(
            host.operations,
            [
                "set_experiments:{\"feature\": true}",
                "refresh_autocomplete",
                "restore_editor",
                "reload:Experimental features updated. Session reloaded.",
                "track:experimental_features_apply:1"
            ]
        );
    }

    #[tokio::test]
    async fn settings_selector_restores_then_routes_selection() {
        let mut host = SettingsHost {
            setting_pick: Some(SettingsSelection::Usage),
            ..SettingsHost::default()
        };
        show_settings_selector(&mut host).await;
        assert_eq!(host.operations, ["restore_editor", "open_usage"]);
    }
}
