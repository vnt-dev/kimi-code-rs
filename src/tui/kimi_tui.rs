use std::{any::Any, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use kimi_code_agent_core_v2::app::auth::{
    AuthOperationError, OAuthToolkitContract, OAuthToolkitService,
};
use kimi_code_oauth::{
    DeviceAuthorization, DeviceCodeObserver, KimiOAuthLoginOptions, KimiOAuthLoginResult,
    LoginAbortSignal, OAuthManagerError,
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    cli::sub::{
        login_flow::LoginCancellation,
        provider::{ModelDefinition, ProviderConfig, ProviderConfigPatch},
        provider_config::ProviderConfigStore,
    },
    sdk::{
        model_alias::{ModelAlias, ProviderType, effective_model_alias},
        types::ThinkingEffort,
    },
    tui::{
        agent_core::TuiAgentCore,
        components::{
            Component, ComponentRole,
            dialogs::model_selector::{ModelSelection, model_display_name, segments_for},
            editor::{CustomEditor, EditorAction, InputMode},
            render::truncate_to_width,
        },
        controllers::{
            dialog_focus::{
                DialogOutcome, MountedDialog, help_dialog, migration_notice_dialog, model_dialog,
                permission_dialog, settings_dialog, theme_dialog,
            },
            slash_autocomplete::{SlashAutocompleteSurface, build_builtin_slash_autocomplete},
            slash_command_surface::{SlashCommandSurfaceAction, resolve_slash_command_surface},
        },
        runtime::{TuiApp, TuiControl},
        theme::{ColorToken, current_theme},
        utils::thinking_config::thinking_effort_to_config,
    },
};

const DEFAULT_PENDING_RESPONSE: &str =
    "Agent runtime is not connected yet. Your input was received.";
const MODEL_SWITCH_CACHE_WARNING: &str = "Note: Switching models invalidates the existing prompt cache. Use /new to avoid extra token costs.";

#[derive(Debug, Clone, PartialEq, Eq)]
enum TranscriptLine {
    User(String),
    Assistant(String),
    System(String),
}

enum LoginUpdate {
    DeviceCode(DeviceAuthorization),
    Finished(Result<KimiOAuthLoginResult, AuthOperationError>),
}

struct ActiveLogin {
    cancellation: LoginCancellation,
    updates: mpsc::UnboundedReceiver<LoginUpdate>,
    task: JoinHandle<()>,
}

impl Drop for ActiveLogin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// Original:
//   apps/kimi-code/src/tui/kimi-tui.ts
//   KimiTUI coordinator
//
// Rust adaptation:
//   This first interactive coordinator owns only layout, editor input, and
//   visible default responses. Session creation, v2 event routing, dialogs,
//   and command side effects remain explicit MIGRATION-TODO boundaries. The
//   defaults keep every accepted user action inside the live TUI instead of
//   panicking at an unimplemented backend call.
pub struct KimiTui {
    version: String,
    editor: CustomEditor,
    transcript: Vec<TranscriptLine>,
    status: Option<String>,
    terminal_rows: usize,
    startup_warning: Option<String>,
    slash_autocomplete: SlashAutocompleteSurface,
    active_dialog: Option<MountedDialog>,
    agent_core: Option<TuiAgentCore>,
    model_config_store: Option<ProviderConfigStore>,
    model: String,
    thinking_effort: ThinkingEffort,
    active_login: Option<ActiveLogin>,
}

impl KimiTui {
    pub fn new(version: impl Into<String>, startup_warning: Option<String>) -> Self {
        Self::with_work_dir(
            version,
            startup_warning,
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    }

    fn with_work_dir(
        version: impl Into<String>,
        startup_warning: Option<String>,
        work_dir: PathBuf,
    ) -> Self {
        let mut editor = CustomEditor::new();
        editor.set_focused(true);
        let slash_autocomplete = build_builtin_slash_autocomplete(work_dir);
        editor.set_autocomplete_provider(Arc::clone(&slash_autocomplete.provider));
        editor.set_argument_hints(slash_autocomplete.argument_hints.clone());
        Self {
            version: version.into(),
            editor,
            transcript: Vec::new(),
            status: Some("Ready".to_owned()),
            terminal_rows: 24,
            startup_warning,
            slash_autocomplete,
            active_dialog: None,
            agent_core: None,
            model_config_store: None,
            model: String::new(),
            thinking_effort: ThinkingEffort::from("off"),
            active_login: None,
        }
    }

    pub fn with_agent_core(
        version: impl Into<String>,
        startup_warning: Option<String>,
        agent_core: TuiAgentCore,
    ) -> Self {
        let mut tui = Self::new(version, startup_warning);
        tui.model_config_store = Some(ProviderConfigStore::new(
            agent_core.bootstrap_options().config_path.clone(),
        ));
        tui.agent_core = Some(agent_core);
        tui
    }

    pub fn editor_text(&self) -> String {
        self.editor.text()
    }

    async fn handle_editor_action(&mut self, action: EditorAction) -> TuiControl {
        match action {
            EditorAction::Submit(text) => self.submit(text).await,
            EditorAction::CtrlC => {
                if self.editor.text().is_empty() {
                    TuiControl::Exit
                } else {
                    self.editor.set_text("");
                    self.status = Some("Input cleared. Press Ctrl+C again to exit.".to_owned());
                    TuiControl::Continue
                }
            }
            EditorAction::CtrlD => TuiControl::Exit,
            EditorAction::UpArrowEmptyWithHistoryFallback => {
                self.editor.apply_up_arrow_history_fallback();
                TuiControl::Continue
            }
            EditorAction::DownArrowEmptyWithHistoryFallback => {
                self.editor.apply_down_arrow_history_fallback();
                TuiControl::Continue
            }
            EditorAction::CtrlBWithCursorLeftFallback => {
                self.editor.apply_ctrl_b_fallback();
                TuiControl::Continue
            }
            EditorAction::Escape => {
                self.status = Some("No dialog is open.".to_owned());
                TuiControl::Continue
            }
            EditorAction::OpenExternalEditor => self.pending_action("External editor"),
            EditorAction::ToggleToolExpand => self.pending_action("Tool output expansion"),
            EditorAction::CtrlS => self.pending_action("Session picker"),
            EditorAction::ToggleTodoWithDefaultFallback => self.pending_action("Todo panel"),
            EditorAction::ShiftTab => self.pending_action("Permission mode switch"),
            EditorAction::UndoShortcut => {
                self.status = Some("Editor undo applied.".to_owned());
                TuiControl::Continue
            }
            EditorAction::PasteImage => self.pending_action("Clipboard image paste"),
            EditorAction::InputModeChanged(InputMode::Prompt) => {
                self.slash_autocomplete.set_input_mode(InputMode::Prompt);
                self.status = Some("Prompt mode".to_owned());
                TuiControl::Continue
            }
            EditorAction::InputModeChanged(InputMode::Bash) => {
                self.slash_autocomplete.set_input_mode(InputMode::Bash);
                self.status = Some("Shell mode (execution not connected)".to_owned());
                TuiControl::Continue
            }
            EditorAction::AutocompleteCancelled => {
                self.status = Some("Autocomplete cancelled.".to_owned());
                TuiControl::Continue
            }
            EditorAction::RequestAutocomplete { force } => {
                if self.editor.text().trim_start().starts_with('/')
                    && let Some(request) = self.editor.begin_autocomplete_request(force, false)
                {
                    let response = request.run().await;
                    self.editor.finish_autocomplete_request(response);
                }
                TuiControl::Continue
            }
            EditorAction::NonEscapeInput => TuiControl::Continue,
        }
    }

    fn pending_action(&mut self, action: &str) -> TuiControl {
        // MIGRATION-TODO:
        // Original: apps/kimi-code/src/tui/kimi-tui.ts dispatches this action
        // to a controller or session service.
        // Temporary behavior: keep the UI alive and show a deterministic
        // acknowledgement.
        self.status = Some(format!("{action} is not connected yet."));
        TuiControl::Continue
    }

    fn mount_dialog(&mut self, dialog: MountedDialog) {
        // Original: KimiTUI.mountEditorReplacement().
        self.editor.cancel_autocomplete();
        self.editor.set_focused(false);
        self.active_dialog = Some(dialog);
        self.status = Some("Dialog open · Esc to cancel".to_owned());
    }

    fn restore_editor(&mut self) {
        // Original: KimiTUI.restoreEditor().
        self.active_dialog = None;
        self.editor.set_focused(true);
    }

    async fn handle_dialog_input(&mut self, data: &str) -> TuiControl {
        let Some(dialog) = &mut self.active_dialog else {
            return TuiControl::Continue;
        };
        dialog.handle_input(data);
        let Some(outcome) = dialog.take_outcome() else {
            return TuiControl::Continue;
        };

        self.restore_editor();
        match outcome {
            DialogOutcome::Cancelled => {
                self.status = Some("Dialog closed.".to_owned());
            }
            DialogOutcome::Selected(message) => {
                // MIGRATION-TODO:
                // Original selector callbacks update KimiHarness session or
                // persisted TUI configuration. Until those v2 services are
                // composed, preserve the selected value as a visible result.
                self.transcript.push(TranscriptLine::System(message));
                self.status = Some("Selection acknowledged.".to_owned());
            }
            DialogOutcome::ModelSelected(selection) => {
                self.apply_model_selection(selection).await;
            }
        }
        TuiControl::Continue
    }

    async fn show_model_picker(&mut self, requested_alias: &str) -> TuiControl {
        let Some(store) = self.model_config_store.as_ref() else {
            self.show_command_error(
                "Model configuration is unavailable because agent-core-v2 did not start.",
            );
            return TuiControl::Continue;
        };
        let config = match store.get_config().await {
            Ok(config) => config,
            Err(error) => {
                self.show_command_error(&format!("Failed to load models: {error}"));
                return TuiControl::Continue;
            }
        };
        let models = match model_aliases_from_config(&config) {
            Ok(models) => models,
            Err(error) => {
                self.show_command_error(&format!("Failed to load models: {error}"));
                return TuiControl::Continue;
            }
        };
        if models.is_empty() {
            self.transcript.push(TranscriptLine::System(
                "No models configured. Run /login to sign in to Kimi, or /provider to add another provider from a model catalog."
                    .to_owned(),
            ));
            self.status = Some("No models configured".to_owned());
            return TuiControl::Continue;
        }
        if !requested_alias.is_empty() && !models.contains_key(requested_alias) {
            self.show_command_error(&format!("Unknown model alias: {requested_alias}"));
            return TuiControl::Continue;
        }

        if self.model.is_empty() {
            self.model = config
                .default_model
                .as_deref()
                .filter(|alias| models.contains_key(*alias))
                .unwrap_or_default()
                .to_owned();
            self.thinking_effort = configured_thinking_effort(&config, &self.model, &models);
        }
        let selected_value = if requested_alias.is_empty() {
            (!self.model.is_empty()).then(|| self.model.clone())
        } else {
            Some(requested_alias.to_owned())
        };
        let warning = self
            .transcript
            .iter()
            .any(|line| matches!(line, TranscriptLine::User(_) | TranscriptLine::Assistant(_)))
            .then(|| MODEL_SWITCH_CACHE_WARNING.to_owned());
        self.mount_dialog(model_dialog(
            models,
            self.model.clone(),
            selected_value,
            self.thinking_effort.clone(),
            warning,
        ));
        TuiControl::Continue
    }

    async fn apply_model_selection(&mut self, selection: ModelSelection) {
        let Some(store) = self.model_config_store.as_ref() else {
            self.show_command_error("Model configuration is unavailable.");
            return;
        };
        let config = match store.get_config().await {
            Ok(config) => config,
            Err(error) => {
                self.show_command_error(&format!("Failed to save model: {error}"));
                return;
            }
        };
        let models = match model_aliases_from_config(&config) {
            Ok(models) => models,
            Err(error) => {
                self.show_command_error(&format!("Failed to save model: {error}"));
                return;
            }
        };
        let Some(model) = models.get(&selection.alias) else {
            self.show_command_error(&format!("Unknown model alias: {}", selection.alias));
            return;
        };
        let thinking =
            thinking_effort_to_config(&selection.thinking, model.support_efforts.as_deref());
        let mut thinking_value = serde_json::Map::from_iter([(
            "enabled".to_owned(),
            serde_json::Value::Bool(thinking.enabled),
        )]);
        if let Some(effort) = thinking.effort {
            thinking_value.insert("effort".to_owned(), serde_json::Value::String(effort));
        }
        let previous_model = self.model.clone();
        let previous_effort = self.thinking_effort.clone();
        if let Err(error) = store
            .set_config(&ProviderConfigPatch {
                default_model: Some(selection.alias.clone()),
                thinking: Some(serde_json::Value::Object(thinking_value)),
                ..ProviderConfigPatch::default()
            })
            .await
        {
            self.show_command_error(&format!("Failed to save model: {error}"));
            return;
        }

        self.model.clone_from(&selection.alias);
        self.thinking_effort.clone_from(&selection.thinking);
        let display_name = model_display_name(&selection.alias, Some(model));
        self.status = Some(if previous_model != selection.alias {
            format!(
                "Switched to {display_name} with thinking {}.",
                selection.thinking.as_str()
            )
        } else if previous_effort != selection.thinking {
            format!("Thinking set to {}.", selection.thinking.as_str())
        } else {
            format!(
                "Already using {display_name} with thinking {}.",
                selection.thinking.as_str()
            )
        });
    }

    fn show_command_error(&mut self, message: &str) {
        self.transcript
            .push(TranscriptLine::System(format!("Error: {message}")));
        self.status = Some(message.to_owned());
    }

    async fn submit(&mut self, text: String) -> TuiControl {
        let text = text.trim().to_owned();
        if text.is_empty() {
            self.status = Some("Enter a message or /help.".to_owned());
            return TuiControl::Continue;
        }
        self.editor.add_to_history(&text);
        if text.starts_with('/') {
            return self.handle_slash_command(&text).await;
        }

        self.transcript.push(TranscriptLine::User(text));
        // MIGRATION-TODO:
        // Original: KimiTUI sends the prompt through KimiHarness.Session.
        // Completion condition: create/resume a v2 session, enqueue the turn,
        // and route DomainEvent values into transcript components.
        self.transcript.push(TranscriptLine::Assistant(
            DEFAULT_PENDING_RESPONSE.to_owned(),
        ));
        self.status = Some("Default response shown; v2 turn execution is pending.".to_owned());
        TuiControl::Continue
    }

    async fn handle_slash_command(&mut self, input: &str) -> TuiControl {
        match resolve_slash_command_surface(input, &self.version) {
            SlashCommandSurfaceAction::Exit => TuiControl::Exit,
            SlashCommandSurfaceAction::ClearTranscript => {
                self.transcript.clear();
                // MIGRATION-TODO:
                // Original: /new (and its /clear alias) creates a fresh v2
                // session in the current workspace.
                // Temporary behavior: clear the local transcript because no
                // v2 session has been composed yet.
                self.status =
                    Some("Local transcript cleared; v2 session creation is pending.".to_owned());
                TuiControl::Continue
            }
            SlashCommandSurfaceAction::ShowHelp => {
                self.mount_dialog(help_dialog());
                TuiControl::Continue
            }
            SlashCommandSurfaceAction::ShowVersion(version) => {
                self.transcript.push(TranscriptLine::System(version));
                self.status = Some("Version".to_owned());
                TuiControl::Continue
            }
            SlashCommandSurfaceAction::Empty => {
                self.status = Some("Enter a slash command.".to_owned());
                TuiControl::Continue
            }
            SlashCommandSurfaceAction::Pending { command_name, args } => {
                if command_name == "login" {
                    return self.login();
                }
                if command_name == "model" {
                    return self.show_model_picker(args.trim()).await;
                }
                // MIGRATION-TODO:
                // Original: commands/dispatch.ts and KimiTUI route the full
                // slash-command registry into controllers and session
                // services. The registered operation and its arguments have
                // been accepted, but its v2-backed behavior is not composed.
                let dialog = match command_name {
                    "settings" => settings_dialog(),
                    "permission" => permission_dialog(),
                    "theme" => theme_dialog(),
                    _ => migration_notice_dialog(command_name, &args),
                };
                self.mount_dialog(dialog);
                TuiControl::Continue
            }
            SlashCommandSurfaceAction::Unknown(name) => {
                self.transcript.push(TranscriptLine::System(format!(
                    "Unknown command: /{}. Type /help to list commands.",
                    name.trim_start_matches('/')
                )));
                self.status = Some("Unknown command.".to_owned());
                TuiControl::Continue
            }
        }
    }

    // Original: apps/kimi-code/src/tui/commands/auth.ts,
    // handleKimiCodeOAuthLogin(). The v2 toolkit owns device-code polling and
    // credential persistence while the TUI keeps rendering and accepting the
    // source command's Ctrl-C cancellation input.
    fn login(&mut self) -> TuiControl {
        if self.active_login.is_some() {
            self.status = Some("Kimi login is already in progress.".to_owned());
            return TuiControl::Continue;
        }
        let Some(agent_core) = self.agent_core.as_ref() else {
            self.status = Some("agent-core-v2 OAuth is not connected.".to_owned());
            return TuiControl::Continue;
        };

        self.status = Some("Opening browser for Kimi login…".to_owned());
        let oauth_toolkit = Arc::clone(agent_core.oauth_toolkit());
        let cancellation = LoginCancellation::default();
        let task_cancellation = cancellation.clone();
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let task = tokio::spawn(run_login(oauth_toolkit, task_cancellation, updates_tx));
        self.active_login = Some(ActiveLogin {
            cancellation,
            updates,
            task,
        });
        TuiControl::Continue
    }

    fn cancel_login(&mut self) {
        let Some(login) = self.active_login.take() else {
            return;
        };
        login.cancellation.abort();
        self.status = Some("Login cancelled.".to_owned());
    }

    fn poll_login(&mut self) -> bool {
        let Some(login) = self.active_login.as_mut() else {
            return false;
        };
        let mut updates = Vec::new();
        let disconnected = loop {
            match login.updates.try_recv() {
                Ok(update) => updates.push(update),
                Err(mpsc::error::TryRecvError::Empty) => break false,
                Err(mpsc::error::TryRecvError::Disconnected) => break true,
            }
        };
        let changed = disconnected || !updates.is_empty();
        let mut finished = false;
        for update in updates {
            match update {
                LoginUpdate::DeviceCode(authorization) => {
                    self.transcript
                        .push(TranscriptLine::System("Sign in to Kimi Code".to_owned()));
                    self.transcript.push(TranscriptLine::System(format!(
                        "Open: {}",
                        authorization.verification_uri_complete
                    )));
                    self.transcript.push(TranscriptLine::System(format!(
                        "Code: {} · Press Ctrl-C to cancel",
                        authorization.user_code
                    )));
                    self.status = Some("Waiting for authorization…".to_owned());
                }
                LoginUpdate::Finished(result) => {
                    finished = true;
                    match result {
                        Ok(result) if result.ok => {
                            self.status = Some(
                                "Kimi Code credentials saved; v2 model provisioning is not connected yet."
                                    .to_owned(),
                            );
                        }
                        Ok(_) => {
                            self.status = Some("Kimi login did not complete.".to_owned());
                        }
                        Err(error) => {
                            self.transcript
                                .push(TranscriptLine::System(format!("Login failed: {error}")));
                            self.status = Some("Login failed.".to_owned());
                        }
                    }
                }
            }
        }
        if disconnected && !finished {
            finished = true;
            self.transcript.push(TranscriptLine::System(
                "Login failed: background login task stopped unexpectedly".to_owned(),
            ));
            self.status = Some("Login failed.".to_owned());
        }
        if finished {
            self.active_login.take();
        }
        changed
    }

    fn render_transcript_line(line: &TranscriptLine) -> String {
        match line {
            TranscriptLine::User(text) => format!(
                "{} {text}",
                current_theme().bold_fg(ColorToken::RoleUser, "You:")
            ),
            TranscriptLine::Assistant(text) => format!(
                "{} {text}",
                current_theme().bold_fg(ColorToken::Primary, "Kimi:")
            ),
            TranscriptLine::System(text) => current_theme().fg(ColorToken::TextMuted, text),
        }
    }
}

fn model_aliases_from_config(
    config: &ProviderConfig,
) -> Result<IndexMap<String, ModelAlias>, String> {
    config
        .models
        .iter()
        .map(|(alias, definition)| {
            let model = model_alias_from_definition(alias, definition)?;
            let provider_type = config
                .providers
                .get(&model.provider)
                .and_then(|provider| provider_type(&provider.provider_type));
            Ok((alias.clone(), effective_model_alias(&model, provider_type)))
        })
        .collect()
}

fn model_alias_from_definition(
    alias: &str,
    definition: &ModelDefinition,
) -> Result<ModelAlias, String> {
    let value = serde_json::to_value(definition)
        .map_err(|error| format!("invalid model alias {alias:?}: {error}"))?;
    serde_json::from_value(value).map_err(|error| format!("invalid model alias {alias:?}: {error}"))
}

fn provider_type(value: &str) -> Option<ProviderType> {
    match value {
        "anthropic" => Some(ProviderType::Anthropic),
        "openai" => Some(ProviderType::Openai),
        "kimi" => Some(ProviderType::Kimi),
        "google-genai" => Some(ProviderType::GoogleGenai),
        "openai_responses" => Some(ProviderType::OpenaiResponses),
        "vertexai" => Some(ProviderType::Vertexai),
        _ => None,
    }
}

fn configured_thinking_effort(
    config: &ProviderConfig,
    alias: &str,
    models: &IndexMap<String, ModelAlias>,
) -> ThinkingEffort {
    let thinking = config
        .thinking
        .as_ref()
        .and_then(serde_json::Value::as_object);
    if thinking
        .and_then(|thinking| thinking.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        return ThinkingEffort::from("off");
    }
    if let Some(effort) = thinking
        .and_then(|thinking| thinking.get("effort"))
        .and_then(serde_json::Value::as_str)
    {
        return ThinkingEffort::new(effort);
    }
    let Some(model) = models.get(alias) else {
        return ThinkingEffort::from("off");
    };
    if let Some(efforts) = model
        .support_efforts
        .as_ref()
        .filter(|efforts| !efforts.is_empty())
    {
        return ThinkingEffort::new(
            model
                .default_effort
                .as_ref()
                .filter(|effort| efforts.contains(*effort))
                .unwrap_or(&efforts[efforts.len() / 2]),
        );
    }
    let segments = segments_for(model);
    if segments.iter().any(|effort| effort == "on") {
        ThinkingEffort::from("on")
    } else {
        ThinkingEffort::new(
            segments
                .first()
                .cloned()
                .unwrap_or_else(|| "off".to_owned()),
        )
    }
}

struct TuiLoginAbortSignal(LoginCancellation);

impl LoginAbortSignal for TuiLoginAbortSignal {
    fn is_aborted(&self) -> bool {
        self.0.is_aborted()
    }
}

struct TuiDeviceCodeObserver {
    updates: mpsc::UnboundedSender<LoginUpdate>,
}

#[async_trait]
impl DeviceCodeObserver for TuiDeviceCodeObserver {
    async fn on_device_code(
        &self,
        authorization: &DeviceAuthorization,
    ) -> Result<(), OAuthManagerError> {
        let _ = self
            .updates
            .send(LoginUpdate::DeviceCode(authorization.clone()));
        crate::utils::open_url::open_url(&authorization.verification_uri_complete);
        Ok(())
    }
}

async fn run_login(
    oauth_toolkit: Arc<OAuthToolkitService>,
    cancellation: LoginCancellation,
    updates: mpsc::UnboundedSender<LoginUpdate>,
) {
    let observer = TuiDeviceCodeObserver {
        updates: updates.clone(),
    };
    let signal = TuiLoginAbortSignal(cancellation);
    let result = oauth_toolkit
        .login(
            Some("kimi-code"),
            KimiOAuthLoginOptions {
                on_device_code: Some(&observer),
                signal: Some(&signal),
                ..KimiOAuthLoginOptions::default()
            },
        )
        .await;
    let _ = updates.send(LoginUpdate::Finished(result));
}

impl Component for KimiTui {
    fn render(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let title = current_theme().bold_fg(ColorToken::Primary, "Kimi Code");
        let subtitle = current_theme().fg(
            ColorToken::TextMuted,
            &format!(
                "Rust interactive migration shell v{} · /help for commands",
                self.version
            ),
        );
        let mut lines = vec![
            truncate_to_width(&title, width, "…", false),
            truncate_to_width(&subtitle, width, "…", false),
            String::new(),
        ];
        if let Some(warning) = self.startup_warning.take() {
            self.transcript.push(TranscriptLine::System(warning));
        }

        let input_lines = if let Some(dialog) = &mut self.active_dialog {
            dialog.render(width)
        } else {
            self.editor.render_editor(width)
        };
        let reserved = input_lines.len().saturating_add(5);
        let transcript_capacity = self.terminal_rows.saturating_sub(reserved).max(1);
        let transcript_start = self.transcript.len().saturating_sub(transcript_capacity);
        lines.extend(
            self.transcript[transcript_start..]
                .iter()
                .map(Self::render_transcript_line)
                .map(|line| truncate_to_width(&line, width, "…", false)),
        );
        lines.push(String::new());
        if let Some(status) = &self.status {
            let status = current_theme().fg(ColorToken::TextMuted, status);
            lines.push(truncate_to_width(&status, width, "…", false));
        }
        lines.extend(input_lines);
        lines
    }

    fn handle_input(&mut self, _data: &str) {
        // KimiTui input is routed through the async TuiApp boundary so
        // autocomplete requests can complete without blocking the runtime.
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[async_trait]
impl TuiApp for KimiTui {
    async fn handle_terminal_input(&mut self, data: &str) -> TuiControl {
        if data == "\u{1b}[I" || data == "\u{1b}[O" {
            return TuiControl::Continue;
        }
        if data == "\u{3}" && self.active_login.is_some() {
            self.cancel_login();
            return TuiControl::Continue;
        }
        if self.active_dialog.is_some() {
            return self.handle_dialog_input(data).await;
        }
        let outcome = self.editor.handle_input_event(data);
        for action in outcome.actions {
            if self.handle_editor_action(action).await == TuiControl::Exit {
                return TuiControl::Exit;
            }
        }
        TuiControl::Continue
    }

    fn handle_terminal_resize(&mut self, _columns: u16, rows: u16) {
        self.terminal_rows = usize::from(rows).max(1);
        self.editor.set_terminal_rows(self.terminal_rows);
    }

    fn poll_background(&mut self) -> bool {
        self.poll_login()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::{Map, Value};

    use super::*;
    use crate::cli::sub::provider::ProviderDefinition;
    use crate::tui::components::render::visible_width;

    fn temporary_model_config_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("kimi-tui-model-{unique}"))
            .join("config.toml")
    }

    fn configured_model(provider: &str, model: &str, display_name: &str) -> ModelDefinition {
        ModelDefinition {
            provider: provider.to_owned(),
            model: model.to_owned(),
            additional_fields: Map::from_iter([
                ("maxContextSize".to_owned(), Value::from(128_000)),
                ("capabilities".to_owned(), serde_json::json!(["thinking"])),
                (
                    "supportEfforts".to_owned(),
                    serde_json::json!(["low", "high"]),
                ),
                ("defaultEffort".to_owned(), Value::String("low".to_owned())),
                (
                    "displayName".to_owned(),
                    Value::String(display_name.to_owned()),
                ),
            ]),
        }
    }

    async fn tui_with_models() -> (KimiTui, ProviderConfigStore, PathBuf) {
        let path = temporary_model_config_path();
        let store = ProviderConfigStore::new(&path);
        store
            .set_config(&ProviderConfigPatch {
                providers: Some(BTreeMap::from([(
                    "test".to_owned(),
                    ProviderDefinition {
                        provider_type: "anthropic".to_owned(),
                        base_url: Some("https://example.test".to_owned()),
                        api_key: Some("test-key".to_owned()),
                        oauth: None,
                        source: None,
                        additional_fields: Map::new(),
                    },
                )])),
                models: Some(BTreeMap::from([
                    (
                        "test/first".to_owned(),
                        configured_model("test", "first", "First"),
                    ),
                    (
                        "test/second".to_owned(),
                        configured_model("test", "second", "Second"),
                    ),
                ])),
                default_model: Some("test/first".to_owned()),
                thinking: Some(serde_json::json!({"enabled": true, "effort": "low"})),
            })
            .await
            .expect("seed config");
        let mut tui = KimiTui::new("0.1.0", None);
        tui.model_config_store = Some(store.clone());
        tui.handle_terminal_resize(120, 40);
        (tui, store, path)
    }

    #[tokio::test]
    async fn accepts_text_and_returns_a_visible_default_response() {
        let mut tui = KimiTui::new("0.1.0", None);
        for input in ["h", "i", "\r"] {
            assert_eq!(tui.handle_terminal_input(input).await, TuiControl::Continue);
        }

        let rendered = tui.render(80).join("\n");
        assert!(rendered.contains("You:"));
        assert!(rendered.contains("hi"));
        assert!(rendered.contains(DEFAULT_PENDING_RESPONSE));
        assert_eq!(tui.editor_text(), "");
    }

    #[tokio::test]
    async fn routes_builtin_and_pending_slash_commands_without_panicking() {
        let mut tui = KimiTui::new("0.1.0", None);
        for input in ["/", "h", "e", "l", "p", "\r"] {
            assert_eq!(tui.handle_terminal_input(input).await, TuiControl::Continue);
        }
        assert!(tui.render(80).join("\n").contains("Keyboard shortcuts"));
        assert_eq!(
            tui.handle_terminal_input("\u{1b}").await,
            TuiControl::Continue
        );

        for input in ["/", "m", "o", "d", "e", "l", "\r"] {
            assert_eq!(tui.handle_terminal_input(input).await, TuiControl::Continue);
        }
        assert!(
            tui.render(80)
                .join("\n")
                .contains("v2 backend is not connected")
        );
    }

    #[tokio::test]
    async fn login_reports_when_agent_core_is_not_composed() {
        let mut tui = KimiTui::new("0.1.0", None);
        for input in ["/", "l", "o", "g", "i", "n", "\r"] {
            assert_eq!(tui.handle_terminal_input(input).await, TuiControl::Continue);
        }
        assert!(
            tui.render(80)
                .join("\n")
                .contains("agent-core-v2 OAuth is not connected")
        );
    }

    #[tokio::test]
    async fn background_login_updates_show_device_code_and_completion() {
        let mut tui = KimiTui::new("0.1.0", None);
        let cancellation = LoginCancellation::default();
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let task = tokio::spawn(std::future::pending::<()>());
        tui.active_login = Some(ActiveLogin {
            cancellation,
            updates,
            task,
        });

        updates_tx
            .send(LoginUpdate::DeviceCode(DeviceAuthorization {
                user_code: "ABCD-EFGH".to_owned(),
                device_code: "device".to_owned(),
                verification_uri: "https://example.com/device".to_owned(),
                verification_uri_complete: "https://example.com/device?code=ABCD-EFGH".to_owned(),
                expires_in: Some(900.0),
                interval: 5.0,
            }))
            .expect("device update");

        assert!(tui.poll_background());
        let waiting = tui.render(160).join("\n");
        assert!(waiting.contains("https://example.com/device?code=ABCD-EFGH"));
        assert!(waiting.contains("ABCD-EFGH"));
        assert!(waiting.contains("Waiting for authorization"));

        updates_tx
            .send(LoginUpdate::Finished(Ok(KimiOAuthLoginResult {
                provider_name: "kimi-code".to_owned(),
                ok: true,
                provision: None,
            })))
            .expect("completion update");

        assert!(tui.poll_background());
        assert!(tui.active_login.is_none());
        assert!(tui.render(160).join("\n").contains("credentials saved"));
    }

    #[tokio::test]
    async fn ctrl_c_cancels_an_active_login_without_exiting() {
        let mut tui = KimiTui::new("0.1.0", None);
        let cancellation = LoginCancellation::default();
        let cancellation_probe = cancellation.clone();
        let (_updates_tx, updates) = mpsc::unbounded_channel();
        let task = tokio::spawn(std::future::pending::<()>());
        tui.active_login = Some(ActiveLogin {
            cancellation,
            updates,
            task,
        });

        assert_eq!(
            tui.handle_terminal_input("\u{3}").await,
            TuiControl::Continue
        );
        assert!(cancellation_probe.is_aborted());
        assert!(tui.active_login.is_none());
        assert!(tui.render(80).join("\n").contains("Login cancelled"));
    }

    #[tokio::test]
    async fn slash_aliases_version_arguments_and_unknown_commands_are_visible() {
        let mut tui = KimiTui::new("1.2.3", None);
        tui.handle_terminal_resize(80, 40);

        for input in ["/version", "/goal ship it", "/missing"] {
            tui.submit(input.to_owned()).await;
            if tui.active_dialog.is_some() {
                tui.handle_dialog_input("\u{1b}");
            }
        }
        let rendered = tui.render(100).join("\n");
        assert!(rendered.contains("Kimi Code v1.2.3"));
        assert!(rendered.contains("Unknown command: /missing"));

        assert_eq!(tui.submit("/q".to_owned()).await, TuiControl::Exit);
    }

    #[tokio::test]
    async fn ctrl_c_clears_input_then_exits_and_render_respects_width() {
        let mut tui = KimiTui::new("0.1.0", Some("config warning".to_owned()));
        tui.handle_terminal_resize(24, 10);
        assert_eq!(tui.handle_terminal_input("x").await, TuiControl::Continue);
        assert_eq!(
            tui.handle_terminal_input("\u{3}").await,
            TuiControl::Continue
        );
        assert_eq!(tui.editor_text(), "");
        assert_eq!(tui.handle_terminal_input("\u{3}").await, TuiControl::Exit);
        for line in tui.render(24) {
            assert!(
                visible_width(&line) <= 24,
                "line exceeds terminal width: {line:?}"
            );
        }
    }

    #[tokio::test]
    async fn renders_and_accepts_builtin_slash_autocomplete() {
        let mut tui = KimiTui::with_work_dir("0.1.0", None, PathBuf::from("."));
        for input in ["/", "m", "o"] {
            assert_eq!(tui.handle_terminal_input(input).await, TuiControl::Continue);
        }

        let rendered = tui.render(100).join("\n");
        assert!(rendered.contains("model"));
        assert!(tui.editor.is_showing_autocomplete());

        assert_eq!(tui.handle_terminal_input("\r").await, TuiControl::Continue);
        assert!(
            tui.render(100)
                .join("\n")
                .contains("v2 backend is not connected")
        );
    }

    #[tokio::test]
    async fn dialog_replaces_editor_focus_and_restores_existing_input() {
        use crate::tui::components::core::CURSOR_MARKER;

        let mut tui = KimiTui::with_work_dir("0.1.0", None, PathBuf::from("."));
        tui.editor.set_text("draft");
        assert!(tui.render(100).join("\n").contains(CURSOR_MARKER));

        tui.submit("/settings".to_owned()).await;
        let dialog = tui.render(100).join("\n");
        assert!(dialog.contains("Settings"));
        assert!(!dialog.contains(CURSOR_MARKER));
        assert_eq!(tui.editor_text(), "draft");

        assert_eq!(
            tui.handle_terminal_input("\u{1b}").await,
            TuiControl::Continue
        );
        let restored = tui.render(100).join("\n");
        assert!(restored.contains(CURSOR_MARKER));
        assert_eq!(tui.editor_text(), "draft");
    }

    #[tokio::test]
    async fn model_command_opens_requested_alias_and_persists_selection() {
        let (mut tui, store, path) = tui_with_models().await;

        assert_eq!(
            tui.submit("/model test/second".to_owned()).await,
            TuiControl::Continue
        );
        assert_eq!(
            tui.active_dialog.as_ref().map(|dialog| dialog.kind),
            Some(crate::tui::controllers::dialog_focus::DialogKind::Model)
        );
        let picker = tui.render(120).join("\n");
        assert!(picker.contains("Select a model"));
        assert!(picker.contains("Second"));

        assert_eq!(tui.handle_terminal_input("\r").await, TuiControl::Continue);
        assert!(tui.active_dialog.is_none());
        assert_eq!(tui.model, "test/second");
        assert_eq!(tui.thinking_effort.as_str(), "low");
        assert!(
            tui.render(120)
                .join("\n")
                .contains("Switched to Second with thinking low.")
        );

        let persisted = store.get_config().await.expect("persisted config");
        assert_eq!(persisted.default_model.as_deref(), Some("test/second"));
        assert_eq!(
            persisted.thinking,
            Some(serde_json::json!({"enabled": true, "effort": "low"}))
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("config parent"));
    }

    #[tokio::test]
    async fn model_command_reports_unknown_alias_and_empty_configuration() {
        let (mut tui, _, path) = tui_with_models().await;
        tui.submit("/model missing".to_owned()).await;
        assert!(
            tui.render(120)
                .join("\n")
                .contains("Unknown model alias: missing")
        );

        let empty_path = temporary_model_config_path();
        let mut empty = KimiTui::new("0.1.0", None);
        empty.model_config_store = Some(ProviderConfigStore::new(&empty_path));
        empty.submit("/model".to_owned()).await;
        assert!(
            empty
                .render(120)
                .join("\n")
                .contains("No models configured")
        );

        let _ = std::fs::remove_dir_all(path.parent().expect("config parent"));
    }
}
