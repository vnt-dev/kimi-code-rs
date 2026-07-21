use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::{
    cli::sub::login_flow::LoginCancellation,
    oauth::{
        managed_auth::KIMI_CODE_PROVIDER_NAME,
        managed_models::ManagedKimiCodeModelInfo,
        open_platform::{
            OpenPlatformDefinition, OpenPlatformError, apply_open_platform_config,
            filter_models_by_prefix, get_open_platform_by_id,
        },
        toolkit::AuthStatus,
    },
    sdk::model_alias::ModelAlias,
    tui::{
        commands::prompts::{
            PromptHost, prompt_api_key, prompt_logout_provider_selection,
            prompt_model_selection_for_open_platform, prompt_platform_selection,
        },
        components::dialogs::ChoiceOption,
    },
};

pub const DEFAULT_OAUTH_PROVIDER_NAME: &str = KIMI_CODE_PROVIDER_NAME;
pub const PRODUCT_NAME: &str = "Kimi Code";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthAppState {
    pub model: String,
    pub available_models: BTreeMap<String, ModelAlias>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthConfig {
    pub value: Map<String, Value>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self { value: Map::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryEvent {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

#[async_trait(?Send)]
pub trait AuthCommandHost: PromptHost {
    fn auth_app_state(&self) -> AuthAppState;
    async fn oauth_status(&self, provider_name: &str) -> Result<AuthStatus, String>;

    /// Runs the OAuth device flow and owns its authorization spinner. The
    /// command retains cancellation and completion-label ordering.
    async fn oauth_login(
        &mut self,
        provider_name: &str,
        cancellation: LoginCancellation,
    ) -> Result<(), String>;
    async fn oauth_logout(&mut self, provider_name: &str) -> Result<(), String>;
    fn finish_oauth_login(&mut self, ok: bool, label: &str);
    fn set_login_cancellation(&mut self, cancellation: Option<LoginCancellation>);

    async fn fetch_open_platform_models(
        &self,
        platform: &OpenPlatformDefinition,
        api_key: &str,
        cancellation: LoginCancellation,
    ) -> Result<Vec<ManagedKimiCodeModelInfo>, OpenPlatformError>;
    async fn get_config(&self, reload: bool) -> Result<AuthConfig, String>;
    async fn set_config(&mut self, config: AuthConfig) -> Result<(), String>;
    async fn remove_provider(&mut self, provider_id: &str) -> Result<(), String>;
    async fn refresh_config_after_login(&mut self) -> Result<(), String>;
    async fn refresh_config_after_logout(&mut self) -> Result<(), String>;
    async fn clear_active_session_after_logout(&mut self) -> Result<(), String>;
    fn apply_reloaded_config(&mut self, config: &AuthConfig);

    fn track(&mut self, event: TelemetryEvent);
    fn show_status(&mut self, message: &str);
}

fn event(
    name: &str,
    properties: impl IntoIterator<Item = (&'static str, String)>,
) -> TelemetryEvent {
    TelemetryEvent {
        name: name.to_owned(),
        properties: properties
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    }
}

// Original: `src/tui/commands/auth.ts`, `handleLoginCommand()`.
pub async fn handle_login_command(host: &mut impl AuthCommandHost) {
    let Some(platform_id) = prompt_platform_selection(host).await else {
        return;
    };
    if platform_id == "kimi-code" {
        handle_kimi_code_oauth_login(host).await;
        return;
    }
    if let Some(platform) = get_open_platform_by_id(&platform_id) {
        handle_open_platform_login(host, platform).await;
    }
}

// Original: `handleKimiCodeOAuthLogin()`.
async fn handle_kimi_code_oauth_login(host: &mut impl AuthCommandHost) {
    let status = match host.oauth_status(DEFAULT_OAUTH_PROVIDER_NAME).await {
        Ok(status) => status,
        Err(error) => {
            host.show_error(&format!("Login failed: {error}"));
            return;
        }
    };
    let already_logged_in = status.providers.iter().any(|provider| {
        provider.provider_name == DEFAULT_OAUTH_PROVIDER_NAME && provider.has_token
    });

    let cancellation = LoginCancellation::default();
    host.set_login_cancellation(Some(cancellation.clone()));
    let login_result = host
        .oauth_login(DEFAULT_OAUTH_PROVIDER_NAME, cancellation.clone())
        .await;
    host.set_login_cancellation(None);
    match login_result {
        Ok(()) => {
            host.finish_oauth_login(true, "Logged in.");
            if let Err(error) = host.refresh_config_after_login().await {
                host.show_error(&format!(
                    "Authentication successful, but failed to refresh config: {error}"
                ));
                return;
            }
            host.track(event(
                "login",
                [
                    ("provider", DEFAULT_OAUTH_PROVIDER_NAME.to_owned()),
                    ("method", "oauth".to_owned()),
                    ("already_logged_in", already_logged_in.to_string()),
                ],
            ));
            if already_logged_in {
                host.show_status("Already logged in. Model configuration refreshed.");
            }
        }
        Err(_error) if cancellation.is_aborted() => {
            host.finish_oauth_login(false, "Login cancelled.");
        }
        Err(error) => {
            host.finish_oauth_login(false, "Login failed.");
            host.show_error(&format!("Login failed: {error}"));
        }
    }
}

// Original: `handleOpenPlatformLogin()`.
async fn handle_open_platform_login(
    host: &mut impl AuthCommandHost,
    platform: &OpenPlatformDefinition,
) {
    let console_host = platform
        .console_url
        .and_then(|url| {
            url.strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
        })
        .unwrap_or_default();
    let platform_name = if console_host.is_empty() {
        "Kimi Platform".to_owned()
    } else {
        format!("Kimi Platform ({console_host})")
    };
    let subtitle_lines = vec![
        format!("{:<12}{}", "base_url", platform.base_url),
        format!("{:<12}~/.kimi-code/config.toml", "saved to"),
    ];
    let Some(api_key) = prompt_api_key(host, &platform_name, Some(subtitle_lines)).await else {
        return;
    };

    let cancellation = LoginCancellation::default();
    host.set_login_cancellation(Some(cancellation.clone()));
    let models_result = host
        .fetch_open_platform_models(platform, &api_key, cancellation.clone())
        .await;
    host.set_login_cancellation(None);
    let models = match models_result {
        Ok(models) => filter_models_by_prefix(&models, platform),
        Err(_) if cancellation.is_aborted() => return,
        Err(error) => {
            let unauthorized = error.status() == Some(401);
            host.show_error(&format!("Failed to verify API key: {error}"));
            if unauthorized {
                host.show_status(
                    "Hint: If your API key was obtained from Kimi Code, please select \"Kimi Code\" instead.",
                );
            }
            return;
        }
    };
    if models.is_empty() {
        host.show_error("No models available for this platform.");
        return;
    }

    let Some(selection) = prompt_model_selection_for_open_platform(host, &models, platform).await
    else {
        return;
    };
    let existing = match host.get_config(false).await {
        Ok(config) => config,
        Err(error) => {
            host.show_error(&error);
            return;
        }
    };
    if existing
        .value
        .get("providers")
        .and_then(Value::as_object)
        .is_some_and(|providers| providers.contains_key(platform.id))
        && let Err(error) = host.remove_provider(platform.id).await
    {
        host.show_error(&error);
        return;
    }
    let mut config = match host.get_config(false).await {
        Ok(config) => config,
        Err(error) => {
            host.show_error(&error);
            return;
        }
    };
    let effort = selection.thinking.as_str();
    apply_open_platform_config(
        &mut config.value,
        platform,
        &models,
        &selection.model,
        effort != "off",
        (!matches!(effort, "off" | "on")).then_some(effort),
        &api_key,
    );
    if let Err(error) = host.set_config(config).await {
        host.show_error(&error);
        return;
    }
    if let Err(error) = host.refresh_config_after_login().await {
        host.show_error(&error);
        return;
    }
    host.track(event(
        "login",
        [
            ("provider", platform.id.to_owned()),
            ("method", "api_key".to_owned()),
        ],
    ));
    host.show_status(&format!(
        "Setup complete: {} · {}",
        platform.name, selection.model.id
    ));
}

// Original: `handleLogoutCommand()`.
pub async fn handle_logout_command(host: &mut impl AuthCommandHost) {
    let oauth_status = match host.oauth_status(DEFAULT_OAUTH_PROVIDER_NAME).await {
        Ok(status) => status,
        Err(error) => {
            host.show_error(&error);
            return;
        }
    };
    let has_oauth_token = oauth_status.providers.iter().any(|provider| {
        provider.provider_name == DEFAULT_OAUTH_PROVIDER_NAME && provider.has_token
    });
    let config = match host.get_config(false).await {
        Ok(config) => config,
        Err(error) => {
            host.show_error(&error);
            return;
        }
    };
    let providers = config
        .value
        .get("providers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut options = Vec::new();
    if has_oauth_token || providers.contains_key(DEFAULT_OAUTH_PROVIDER_NAME) {
        options.push(
            ChoiceOption::new(DEFAULT_OAUTH_PROVIDER_NAME, PRODUCT_NAME)
                .with_description("OAuth login"),
        );
    }
    let mut api_key_provider_ids = providers
        .keys()
        .filter(|id| id.as_str() != DEFAULT_OAUTH_PROVIDER_NAME)
        .cloned()
        .collect::<Vec<_>>();
    api_key_provider_ids.sort();
    for id in api_key_provider_ids {
        let mut option = ChoiceOption::new(&id, &id);
        if let Some(base_url) = providers
            .get(&id)
            .and_then(|provider| provider.get("baseUrl"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            option = option.with_description(base_url);
        }
        options.push(option);
    }
    if options.is_empty() {
        host.show_status("Nothing to logout.");
        return;
    }

    let state = host.auth_app_state();
    let current_provider = state
        .available_models
        .get(state.model.trim())
        .map(|model| model.provider.clone());
    let Some(target) =
        prompt_logout_provider_selection(host, options, current_provider.clone()).await
    else {
        return;
    };
    let operation = if target == DEFAULT_OAUTH_PROVIDER_NAME {
        host.oauth_logout(DEFAULT_OAUTH_PROVIDER_NAME).await
    } else {
        host.remove_provider(&target).await
    };
    if let Err(error) = operation {
        host.show_error(&error);
        return;
    }

    if current_provider.as_deref() == Some(&target) {
        if let Err(error) = host.refresh_config_after_logout().await {
            host.show_error(&error);
            return;
        }
        if let Err(error) = host.clear_active_session_after_logout().await {
            host.show_error(&error);
            return;
        }
    } else {
        match host.get_config(true).await {
            Ok(updated) => host.apply_reloaded_config(&updated),
            Err(error) => {
                host.show_error(&error);
                return;
            }
        }
    }
    host.track(event("logout", [("provider", target.clone())]));
    let label = if target == DEFAULT_OAUTH_PROVIDER_NAME {
        PRODUCT_NAME
    } else {
        &target
    };
    host.show_status(&format!("Logged out from {label}."));
}

#[cfg(test)]
mod tests {
    use crate::{
        oauth::{managed_models::SupportsThinkingType, toolkit::AuthProviderStatus},
        sdk::model_alias::ModelProtocol,
        tui::components::Component,
    };

    use super::*;

    struct Host {
        scripts: Vec<Vec<String>>,
        mount_index: usize,
        state: AuthAppState,
        status: AuthStatus,
        config: AuthConfig,
        models: Result<Vec<ManagedKimiCodeModelInfo>, String>,
        oauth_login_error: Option<String>,
        cancelled_login: bool,
        events: Vec<TelemetryEvent>,
        statuses: Vec<String>,
        errors: Vec<String>,
        cancellation_states: Vec<bool>,
        oauth_logouts: Vec<String>,
        removed: Vec<String>,
        refreshed_login: usize,
        refreshed_logout: usize,
        cleared_session: usize,
        applied_reload: usize,
    }

    impl PromptHost for Host {
        fn mount_editor_replacement(&mut self, mut component: Box<dyn Component>) {
            let script = self
                .scripts
                .get(self.mount_index)
                .cloned()
                .unwrap_or_default();
            self.mount_index += 1;
            for input in script {
                component.handle_input(&input);
            }
        }

        fn restore_editor(&mut self) {}

        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    #[async_trait(?Send)]
    impl AuthCommandHost for Host {
        fn auth_app_state(&self) -> AuthAppState {
            self.state.clone()
        }

        async fn oauth_status(&self, _: &str) -> Result<AuthStatus, String> {
            Ok(self.status.clone())
        }

        async fn oauth_login(
            &mut self,
            _: &str,
            cancellation: LoginCancellation,
        ) -> Result<(), String> {
            if self.cancelled_login {
                cancellation.abort();
            }
            self.oauth_login_error.clone().map_or(Ok(()), Err)
        }

        async fn oauth_logout(&mut self, provider_name: &str) -> Result<(), String> {
            self.oauth_logouts.push(provider_name.to_owned());
            Ok(())
        }

        fn finish_oauth_login(&mut self, ok: bool, label: &str) {
            self.statuses.push(format!("spinner:{ok}:{label}"));
        }

        fn set_login_cancellation(&mut self, cancellation: Option<LoginCancellation>) {
            self.cancellation_states.push(cancellation.is_some());
        }

        async fn fetch_open_platform_models(
            &self,
            _: &OpenPlatformDefinition,
            _: &str,
            _: LoginCancellation,
        ) -> Result<Vec<ManagedKimiCodeModelInfo>, OpenPlatformError> {
            self.models
                .clone()
                .map_err(OpenPlatformError::InvalidHeader)
        }

        async fn get_config(&self, _: bool) -> Result<AuthConfig, String> {
            Ok(self.config.clone())
        }

        async fn set_config(&mut self, config: AuthConfig) -> Result<(), String> {
            self.config = config;
            Ok(())
        }

        async fn remove_provider(&mut self, provider_id: &str) -> Result<(), String> {
            self.removed.push(provider_id.to_owned());
            if let Some(providers) = self
                .config
                .value
                .get_mut("providers")
                .and_then(Value::as_object_mut)
            {
                providers.remove(provider_id);
            }
            Ok(())
        }

        async fn refresh_config_after_login(&mut self) -> Result<(), String> {
            self.refreshed_login += 1;
            Ok(())
        }

        async fn refresh_config_after_logout(&mut self) -> Result<(), String> {
            self.refreshed_logout += 1;
            Ok(())
        }

        async fn clear_active_session_after_logout(&mut self) -> Result<(), String> {
            self.cleared_session += 1;
            Ok(())
        }

        fn apply_reloaded_config(&mut self, _: &AuthConfig) {
            self.applied_reload += 1;
        }

        fn track(&mut self, event: TelemetryEvent) {
            self.events.push(event);
        }

        fn show_status(&mut self, message: &str) {
            self.statuses.push(message.to_owned());
        }
    }

    fn alias(provider: &str) -> ModelAlias {
        ModelAlias {
            provider: provider.to_owned(),
            model: "model".to_owned(),
            max_context_size: 128_000,
            max_output_size: None,
            capabilities: None,
            display_name: None,
            reasoning_key: None,
            protocol: Some(ModelProtocol::Anthropic),
            adaptive_thinking: None,
            support_efforts: None,
            default_effort: None,
            beta_api: None,
            overrides: None,
        }
    }

    fn model() -> ManagedKimiCodeModelInfo {
        ManagedKimiCodeModelInfo {
            id: "kimi-k2".to_owned(),
            context_length: 128_000,
            supports_reasoning: true,
            supports_image_in: true,
            supports_video_in: false,
            supports_tool_use: true,
            supports_thinking_type: Some(SupportsThinkingType::Both),
            support_efforts: None,
            default_effort: None,
            display_name: Some("Kimi K2".to_owned()),
            protocol: Some(crate::oauth::managed_models::ManagedKimiCodeProtocol::Anthropic),
        }
    }

    fn host() -> Host {
        Host {
            scripts: Vec::new(),
            mount_index: 0,
            state: AuthAppState {
                model: "current".to_owned(),
                available_models: BTreeMap::from([(
                    "current".to_owned(),
                    alias(DEFAULT_OAUTH_PROVIDER_NAME),
                )]),
            },
            status: AuthStatus {
                providers: vec![AuthProviderStatus {
                    provider_name: DEFAULT_OAUTH_PROVIDER_NAME.to_owned(),
                    has_token: false,
                }],
            },
            config: AuthConfig::default(),
            models: Ok(vec![model()]),
            oauth_login_error: None,
            cancelled_login: false,
            events: Vec::new(),
            statuses: Vec::new(),
            errors: Vec::new(),
            cancellation_states: Vec::new(),
            oauth_logouts: Vec::new(),
            removed: Vec::new(),
            refreshed_login: 0,
            refreshed_logout: 0,
            cleared_session: 0,
            applied_reload: 0,
        }
    }

    #[tokio::test]
    async fn oauth_login_tracks_existing_session_and_clears_cancellation() {
        let mut host = host();
        host.scripts = vec![vec!["\r".to_owned()]];
        host.status.providers[0].has_token = true;
        handle_login_command(&mut host).await;
        assert_eq!(host.cancellation_states, [true, false]);
        assert_eq!(host.refreshed_login, 1);
        assert_eq!(host.events[0].properties["already_logged_in"], "true");
        assert!(
            host.statuses
                .iter()
                .any(|value| value.contains("Already logged in"))
        );
    }

    #[tokio::test]
    async fn cancelled_oauth_login_stops_without_error_or_tracking() {
        let mut host = host();
        host.scripts = vec![vec!["\r".to_owned()]];
        host.cancelled_login = true;
        host.oauth_login_error = Some("aborted".to_owned());
        handle_login_command(&mut host).await;
        assert!(host.errors.is_empty());
        assert!(host.events.is_empty());
        assert!(
            host.statuses
                .iter()
                .any(|value| value.contains("Login cancelled"))
        );
    }

    #[tokio::test]
    async fn logout_current_managed_provider_refreshes_and_clears_session() {
        let mut host = host();
        host.status.providers[0].has_token = true;
        host.scripts = vec![vec!["\r".to_owned()]];
        handle_logout_command(&mut host).await;
        assert_eq!(host.oauth_logouts, [DEFAULT_OAUTH_PROVIDER_NAME]);
        assert_eq!((host.refreshed_logout, host.cleared_session), (1, 1));
        assert_eq!(host.events[0].name, "logout");
        assert!(
            host.statuses
                .iter()
                .any(|value| value == "Logged out from Kimi Code.")
        );
    }

    #[tokio::test]
    async fn logout_with_no_configured_provider_is_a_status_only() {
        let mut host = host();
        host.state.available_models.clear();
        handle_logout_command(&mut host).await;
        assert_eq!(host.statuses, ["Nothing to logout."]);
        assert_eq!(host.mount_index, 0);
    }
}
