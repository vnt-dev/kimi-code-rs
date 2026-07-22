use std::{error::Error, fmt, path::PathBuf};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::{
    cli::version::{IdentityError, create_kimi_code_user_agent},
    oauth::{
        managed_auth::ManagedKimiOAuthRef,
        refresh_provider_models::{RefreshHostError, RefreshProviderHost, RefreshProviderOptions},
    },
    sdk::{
        model_alias::ModelAlias,
        types::{PermissionMode, ThinkingEffort},
    },
    tui::{
        constant::kimi_tui::OAUTH_LOGIN_REQUIRED_STARTUP_NOTICE,
        utils::{
            refresh_providers::{RefreshProviderScope, RefreshResult, refresh_all_provider_models},
            thinking_config::{ThinkingConfig, thinking_effort_from_config},
        },
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct AuthFlowConfig {
    pub models: IndexMap<String, ModelAlias>,
    pub providers: IndexMap<String, Value>,
    pub default_model: Option<String>,
    pub thinking: Option<ThinkingConfig>,
}

impl Default for AuthFlowConfig {
    fn default() -> Self {
        Self {
            models: IndexMap::new(),
            providers: IndexMap::new(),
            default_model: None,
            thinking: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFlowState {
    pub work_dir: PathBuf,
    pub additional_dirs: Vec<PathBuf>,
    pub plan_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthFlowStartupOptions {
    pub model: Option<String>,
    pub auto: bool,
    pub yolo: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuthFlowAppPatch {
    pub available_models: Option<IndexMap<String, ModelAlias>>,
    pub available_providers: Option<IndexMap<String, Value>>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub context_tokens: Option<u64>,
    pub max_context_tokens: Option<u64>,
    pub context_usage: Option<f64>,
    pub session_title: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTuiSessionOptions {
    pub work_dir: PathBuf,
    pub model: String,
    pub thinking: Option<ThinkingEffort>,
    pub permission: Option<PermissionMode>,
    pub plan_mode: Option<bool>,
    pub additional_dirs: Option<Vec<PathBuf>>,
}

#[derive(Debug)]
pub struct AuthFlowHostError(Box<dyn Error + Send + Sync>);

impl AuthFlowHostError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for AuthFlowHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for AuthFlowHostError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait AuthFlowSession: Send + Sync {
    fn id(&self) -> &str;
    fn title(&self) -> Option<&str>;
    async fn set_model(&self, model: &str) -> Result<(), AuthFlowHostError>;
    async fn set_thinking(&self, effort: &ThinkingEffort) -> Result<(), AuthFlowHostError>;
}

#[async_trait]
pub trait AuthFlowHost: RefreshProviderHost {
    fn auth_flow_state(&self) -> AuthFlowState;
    fn startup_options(&self) -> AuthFlowStartupOptions;
    fn version(&self) -> &str;
    fn active_session(&self) -> Option<&dyn AuthFlowSession>;
    fn set_app_state(&mut self, patch: AuthFlowAppPatch);
    fn set_startup_ready(&mut self);
    fn reset_session_runtime(&mut self);
    fn append_startup_notice(&mut self, notice: &str);

    async fn load_config(&self, reload: bool) -> Result<AuthFlowConfig, AuthFlowHostError>;
    async fn create_session(
        &self,
        options: CreateTuiSessionOptions,
    ) -> Result<Box<dyn AuthFlowSession>, AuthFlowHostError>;
    async fn set_session(
        &mut self,
        session: Box<dyn AuthFlowSession>,
    ) -> Result<(), AuthFlowHostError>;
    async fn sync_runtime_state(&mut self) -> Result<(), AuthFlowHostError>;
    async fn close_session(&mut self, reason: &str) -> Result<(), AuthFlowHostError>;
    async fn refresh_skill_commands(&mut self) -> Result<(), AuthFlowHostError>;
    async fn refresh_plugin_commands(&mut self) -> Result<(), AuthFlowHostError>;

    fn start_session_subscription(&mut self);
    fn schedule_fetch_sessions(&mut self);
    fn update_terminal_title(&mut self);
    fn schedule_refresh_skill_commands(&mut self);
    fn schedule_refresh_plugin_commands(&mut self);
}

#[derive(Debug)]
pub enum AuthFlowError {
    Host(AuthFlowHostError),
    Refresh(RefreshHostError),
    Identity(IdentityError),
}

impl fmt::Display for AuthFlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(error) => error.fmt(formatter),
            Self::Refresh(error) => error.fmt(formatter),
            Self::Identity(error) => error.fmt(formatter),
        }
    }
}

impl Error for AuthFlowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Host(error) => Some(error),
            Self::Refresh(error) => Some(error),
            Self::Identity(error) => Some(error),
        }
    }
}

impl From<AuthFlowHostError> for AuthFlowError {
    fn from(error: AuthFlowHostError) -> Self {
        Self::Host(error)
    }
}

#[derive(Debug, Default)]
pub struct AuthFlowController;

impl AuthFlowController {
    // Original: `src/tui/controllers/auth-flow.ts`, `refreshAvailableModels()`.
    pub async fn refresh_available_models(
        &self,
        host: &mut impl AuthFlowHost,
    ) -> Result<(), AuthFlowError> {
        let config = host.load_config(true).await?;
        host.set_app_state(AuthFlowAppPatch {
            available_models: Some(config.models),
            available_providers: Some(config.providers),
            ..AuthFlowAppPatch::default()
        });
        Ok(())
    }

    // Original: `enterLoginRequiredStartupState()`.
    pub fn enter_login_required_startup_state(&self, host: &mut impl AuthFlowHost) {
        host.reset_session_runtime();
        host.set_app_state(AuthFlowAppPatch {
            session_id: Some(String::new()),
            model: Some(String::new()),
            thinking_effort: Some(ThinkingEffort::from("off")),
            context_tokens: Some(0),
            max_context_tokens: Some(0),
            context_usage: Some(0.0),
            session_title: Some(None),
            ..AuthFlowAppPatch::default()
        });
        host.append_startup_notice(OAUTH_LOGIN_REQUIRED_STARTUP_NOTICE);
        host.set_startup_ready();
    }

    // Original: `activateModelAfterLogin()`.
    pub async fn activate_model_after_login(
        &self,
        host: &mut impl AuthFlowHost,
        model: &str,
        effort: Option<ThinkingEffort>,
    ) -> Result<(), AuthFlowError> {
        if let Some(session) = host.active_session() {
            session.set_model(model).await?;
            if let Some(effort) = effort.as_ref() {
                session.set_thinking(effort).await?;
            }
            return Ok(());
        }

        let state = host.auth_flow_state();
        let startup = host.startup_options();
        let permission = if startup.auto {
            Some(PermissionMode::Auto)
        } else if startup.yolo {
            Some(PermissionMode::Yolo)
        } else {
            None
        };
        let session = host
            .create_session(CreateTuiSessionOptions {
                work_dir: state.work_dir,
                model: model.to_owned(),
                thinking: effort,
                permission,
                plan_mode: state.plan_mode.then_some(true),
                additional_dirs: (!state.additional_dirs.is_empty())
                    .then_some(state.additional_dirs),
            })
            .await?;
        let session_id = session.id().to_owned();
        let session_title = session.title().map(str::to_owned);
        host.set_session(session).await?;
        host.set_app_state(AuthFlowAppPatch {
            session_id: Some(session_id),
            session_title: Some(session_title),
            ..AuthFlowAppPatch::default()
        });
        host.sync_runtime_state().await?;
        host.start_session_subscription();
        host.schedule_fetch_sessions();
        host.update_terminal_title();
        host.schedule_refresh_skill_commands();
        host.schedule_refresh_plugin_commands();
        Ok(())
    }

    // Original: `clearActiveSessionAfterLogout()`.
    pub async fn clear_active_session_after_logout(
        &self,
        host: &mut impl AuthFlowHost,
    ) -> Result<(), AuthFlowError> {
        host.close_session("logged out").await?;
        host.reset_session_runtime();
        host.set_app_state(AuthFlowAppPatch {
            session_id: Some(String::new()),
            model: Some(String::new()),
            session_title: Some(None),
            ..AuthFlowAppPatch::default()
        });
        host.refresh_skill_commands().await?;
        host.refresh_plugin_commands().await?;
        Ok(())
    }

    // Original: `refreshConfigAfterLogin()`.
    pub async fn refresh_config_after_login(
        &self,
        host: &mut impl AuthFlowHost,
    ) -> Result<(), AuthFlowError> {
        let config = host.load_config(true).await?;
        let default_model = host
            .startup_options()
            .model
            .or(config.default_model.clone());
        let Some(default_model) = default_model else {
            host.set_app_state(AuthFlowAppPatch {
                available_models: Some(config.models),
                available_providers: Some(config.providers),
                ..AuthFlowAppPatch::default()
            });
            return Ok(());
        };
        let Some(selected) = config.models.get(&default_model).cloned() else {
            host.set_app_state(AuthFlowAppPatch {
                available_models: Some(config.models),
                available_providers: Some(config.providers),
                ..AuthFlowAppPatch::default()
            });
            return Ok(());
        };
        let effort = thinking_effort_from_config(config.thinking.as_ref());
        self.activate_model_after_login(host, &default_model, effort)
            .await?;
        host.set_app_state(AuthFlowAppPatch {
            available_models: Some(config.models),
            available_providers: Some(config.providers),
            model: Some(default_model),
            max_context_tokens: Some(selected.max_context_size),
            ..AuthFlowAppPatch::default()
        });
        Ok(())
    }

    // Original: `refreshConfigAfterLogout()`.
    pub async fn refresh_config_after_logout(
        &self,
        host: &mut impl AuthFlowHost,
    ) -> Result<(), AuthFlowError> {
        let config = host.load_config(true).await?;
        host.set_app_state(AuthFlowAppPatch {
            available_models: Some(config.models),
            available_providers: Some(config.providers),
            model: Some(String::new()),
            thinking_effort: Some(ThinkingEffort::from("off")),
            max_context_tokens: Some(0),
            context_usage: Some(0.0),
            context_tokens: Some(0),
            ..AuthFlowAppPatch::default()
        });
        Ok(())
    }

    // Original: `refreshProviderModels()`.
    pub async fn refresh_provider_models(
        &self,
        host: &mut impl AuthFlowHost,
    ) -> Result<RefreshResult, AuthFlowError> {
        self.refresh_provider_models_with_scope(host, RefreshProviderScope::All)
            .await
    }

    // Original: `refreshOAuthProviderModels()`.
    pub async fn refresh_oauth_provider_models(
        &self,
        host: &mut impl AuthFlowHost,
    ) -> Result<RefreshResult, AuthFlowError> {
        self.refresh_provider_models_with_scope(host, RefreshProviderScope::OAuth)
            .await
    }

    async fn refresh_provider_models_with_scope(
        &self,
        host: &mut impl AuthFlowHost,
        scope: RefreshProviderScope,
    ) -> Result<RefreshResult, AuthFlowError> {
        let user_agent =
            create_kimi_code_user_agent(host.version()).map_err(AuthFlowError::Identity)?;
        let adapter = RefreshHostWithUserAgent { host, user_agent };
        let result = refresh_all_provider_models(
            &adapter,
            RefreshProviderOptions {
                scope,
                provider_id: None,
            },
        )
        .await
        .map_err(AuthFlowError::Refresh)?;
        if !result.changed.is_empty() {
            self.refresh_available_models(host).await?;
        }
        Ok(result)
    }
}

struct RefreshHostWithUserAgent<'a, H> {
    host: &'a H,
    user_agent: String,
}

#[async_trait]
impl<H: AuthFlowHost> RefreshProviderHost for RefreshHostWithUserAgent<'_, H> {
    async fn get_config(&self) -> Result<Map<String, Value>, RefreshHostError> {
        self.host.get_config().await
    }

    async fn remove_provider(
        &self,
        provider_id: &str,
    ) -> Result<Map<String, Value>, RefreshHostError> {
        self.host.remove_provider(provider_id).await
    }

    async fn set_config(
        &self,
        patch: Map<String, Value>,
    ) -> Result<Map<String, Value>, RefreshHostError> {
        self.host.set_config(patch).await
    }

    async fn resolve_oauth_token(
        &self,
        provider_name: &str,
        oauth_ref: Option<&ManagedKimiOAuthRef>,
    ) -> Result<String, RefreshHostError> {
        self.host
            .resolve_oauth_token(provider_name, oauth_ref)
            .await
    }

    fn user_agent(&self) -> Option<&str> {
        Some(&self.user_agent)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::sdk::model_alias::ModelProtocol;

    #[derive(Default)]
    struct SessionCalls {
        models: Vec<String>,
        thinking: Vec<ThinkingEffort>,
    }

    struct SessionMock {
        id: String,
        title: Option<String>,
        calls: Arc<Mutex<SessionCalls>>,
    }

    #[async_trait]
    impl AuthFlowSession for SessionMock {
        fn id(&self) -> &str {
            &self.id
        }

        fn title(&self) -> Option<&str> {
            self.title.as_deref()
        }

        async fn set_model(&self, model: &str) -> Result<(), AuthFlowHostError> {
            self.calls
                .lock()
                .expect("session calls")
                .models
                .push(model.to_owned());
            Ok(())
        }

        async fn set_thinking(&self, effort: &ThinkingEffort) -> Result<(), AuthFlowHostError> {
            self.calls
                .lock()
                .expect("session calls")
                .thinking
                .push(effort.clone());
            Ok(())
        }
    }

    struct HostMock {
        state: AuthFlowState,
        startup: AuthFlowStartupOptions,
        config: AuthFlowConfig,
        active: Option<Box<dyn AuthFlowSession>>,
        created_calls: Arc<Mutex<SessionCalls>>,
        create_options: Mutex<Vec<CreateTuiSessionOptions>>,
        patches: Vec<AuthFlowAppPatch>,
        events: Mutex<Vec<String>>,
        notices: Vec<String>,
        startup_ready: usize,
    }

    impl HostMock {
        fn new() -> Self {
            Self {
                state: AuthFlowState {
                    work_dir: PathBuf::from("/work"),
                    additional_dirs: Vec::new(),
                    plan_mode: false,
                },
                startup: AuthFlowStartupOptions::default(),
                config: AuthFlowConfig::default(),
                active: None,
                created_calls: Arc::new(Mutex::new(SessionCalls::default())),
                create_options: Mutex::new(Vec::new()),
                patches: Vec::new(),
                events: Mutex::new(Vec::new()),
                notices: Vec::new(),
                startup_ready: 0,
            }
        }

        fn event(&self, value: &str) {
            self.events.expect_lock().push(value.to_owned());
        }
    }

    trait ExpectLock<T> {
        fn expect_lock(&self) -> std::sync::MutexGuard<'_, T>;
    }

    impl<T> ExpectLock<T> for Mutex<T> {
        fn expect_lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.lock().expect("test mutex")
        }
    }

    #[async_trait]
    impl RefreshProviderHost for HostMock {
        async fn get_config(&self) -> Result<Map<String, Value>, RefreshHostError> {
            self.event("refresh:get-config");
            Ok(Map::new())
        }

        async fn remove_provider(&self, _: &str) -> Result<Map<String, Value>, RefreshHostError> {
            panic!("no provider")
        }

        async fn set_config(
            &self,
            _: Map<String, Value>,
        ) -> Result<Map<String, Value>, RefreshHostError> {
            panic!("no config changes")
        }

        async fn resolve_oauth_token(
            &self,
            _: &str,
            _: Option<&ManagedKimiOAuthRef>,
        ) -> Result<String, RefreshHostError> {
            panic!("no oauth provider")
        }
    }

    #[async_trait]
    impl AuthFlowHost for HostMock {
        fn auth_flow_state(&self) -> AuthFlowState {
            self.state.clone()
        }

        fn startup_options(&self) -> AuthFlowStartupOptions {
            self.startup.clone()
        }

        fn version(&self) -> &str {
            "1.2.3"
        }

        fn active_session(&self) -> Option<&dyn AuthFlowSession> {
            self.active.as_deref()
        }

        fn set_app_state(&mut self, patch: AuthFlowAppPatch) {
            self.event("patch");
            self.patches.push(patch);
        }

        fn set_startup_ready(&mut self) {
            self.event("startup-ready");
            self.startup_ready += 1;
        }

        fn reset_session_runtime(&mut self) {
            self.event("reset-runtime");
        }

        fn append_startup_notice(&mut self, notice: &str) {
            self.event("startup-notice");
            self.notices.push(notice.to_owned());
        }

        async fn load_config(&self, reload: bool) -> Result<AuthFlowConfig, AuthFlowHostError> {
            self.event(&format!("load-config:{reload}"));
            Ok(self.config.clone())
        }

        async fn create_session(
            &self,
            options: CreateTuiSessionOptions,
        ) -> Result<Box<dyn AuthFlowSession>, AuthFlowHostError> {
            self.event("create-session");
            self.create_options.expect_lock().push(options);
            Ok(Box::new(SessionMock {
                id: "ses-new".to_owned(),
                title: Some("New session".to_owned()),
                calls: Arc::clone(&self.created_calls),
            }))
        }

        async fn set_session(
            &mut self,
            session: Box<dyn AuthFlowSession>,
        ) -> Result<(), AuthFlowHostError> {
            self.event("set-session");
            self.active = Some(session);
            Ok(())
        }

        async fn sync_runtime_state(&mut self) -> Result<(), AuthFlowHostError> {
            self.event("sync-runtime");
            Ok(())
        }

        async fn close_session(&mut self, reason: &str) -> Result<(), AuthFlowHostError> {
            self.event(&format!("close:{reason}"));
            self.active = None;
            Ok(())
        }

        async fn refresh_skill_commands(&mut self) -> Result<(), AuthFlowHostError> {
            self.event("refresh-skills");
            Ok(())
        }

        async fn refresh_plugin_commands(&mut self) -> Result<(), AuthFlowHostError> {
            self.event("refresh-plugins");
            Ok(())
        }

        fn start_session_subscription(&mut self) {
            self.event("subscribe");
        }

        fn schedule_fetch_sessions(&mut self) {
            self.event("schedule-sessions");
        }

        fn update_terminal_title(&mut self) {
            self.event("terminal-title");
        }

        fn schedule_refresh_skill_commands(&mut self) {
            self.event("schedule-skills");
        }

        fn schedule_refresh_plugin_commands(&mut self) {
            self.event("schedule-plugins");
        }
    }

    fn alias(context: u64) -> ModelAlias {
        ModelAlias {
            provider: "managed:kimi-code".to_owned(),
            model: "model".to_owned(),
            max_context_size: context,
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

    #[test]
    fn enters_login_required_state_in_source_order() {
        let controller = AuthFlowController;
        let mut host = HostMock::new();

        controller.enter_login_required_startup_state(&mut host);

        assert_eq!(
            *host.events.expect_lock(),
            ["reset-runtime", "patch", "startup-notice", "startup-ready"]
        );
        assert_eq!(host.notices, [OAUTH_LOGIN_REQUIRED_STARTUP_NOTICE]);
        assert_eq!(host.startup_ready, 1);
        let patch = &host.patches[0];
        assert_eq!(patch.session_id.as_deref(), Some(""));
        assert_eq!(patch.model.as_deref(), Some(""));
        assert_eq!(
            patch.thinking_effort.as_ref().map(ThinkingEffort::as_str),
            Some("off")
        );
        assert_eq!(patch.session_title, Some(None));
    }

    #[tokio::test]
    async fn existing_session_switches_model_and_optional_thinking_only() {
        let controller = AuthFlowController;
        let mut host = HostMock::new();
        let calls = Arc::new(Mutex::new(SessionCalls::default()));
        host.active = Some(Box::new(SessionMock {
            id: "ses-old".to_owned(),
            title: None,
            calls: Arc::clone(&calls),
        }));

        controller
            .activate_model_after_login(&mut host, "new-model", Some(ThinkingEffort::from("high")))
            .await
            .expect("activate");

        let calls = calls.expect_lock();
        assert_eq!(calls.models, ["new-model"]);
        assert_eq!(
            calls
                .thinking
                .iter()
                .map(ThinkingEffort::as_str)
                .collect::<Vec<_>>(),
            ["high"]
        );
        assert!(host.create_options.expect_lock().is_empty());
        assert!(host.patches.is_empty());
    }

    #[tokio::test]
    async fn creates_session_with_startup_modes_then_runs_post_setup_hooks_in_order() {
        let controller = AuthFlowController;
        let mut host = HostMock::new();
        host.state.additional_dirs = vec![PathBuf::from("/extra")];
        host.state.plan_mode = true;
        host.startup.auto = true;
        host.startup.yolo = true;

        controller
            .activate_model_after_login(&mut host, "model", Some(ThinkingEffort::from("on")))
            .await
            .expect("activate");

        assert_eq!(
            host.create_options.expect_lock().as_slice(),
            [CreateTuiSessionOptions {
                work_dir: PathBuf::from("/work"),
                model: "model".to_owned(),
                thinking: Some(ThinkingEffort::from("on")),
                permission: Some(PermissionMode::Auto),
                plan_mode: Some(true),
                additional_dirs: Some(vec![PathBuf::from("/extra")]),
            }]
        );
        assert_eq!(
            *host.events.expect_lock(),
            [
                "create-session",
                "set-session",
                "patch",
                "sync-runtime",
                "subscribe",
                "schedule-sessions",
                "terminal-title",
                "schedule-skills",
                "schedule-plugins",
            ]
        );
        assert_eq!(host.patches[0].session_id.as_deref(), Some("ses-new"));
        assert_eq!(
            host.patches[0].session_title,
            Some(Some("New session".to_owned()))
        );
    }

    #[tokio::test]
    async fn logout_clears_session_then_awaits_command_refreshes() {
        let controller = AuthFlowController;
        let mut host = HostMock::new();
        controller
            .clear_active_session_after_logout(&mut host)
            .await
            .expect("logout");
        assert_eq!(
            *host.events.expect_lock(),
            [
                "close:logged out",
                "reset-runtime",
                "patch",
                "refresh-skills",
                "refresh-plugins",
            ]
        );
        assert_eq!(host.patches[0].session_title, Some(None));
    }

    #[tokio::test]
    async fn login_config_activates_selected_model_and_applies_model_metadata() {
        let controller = AuthFlowController;
        let mut host = HostMock::new();
        host.config
            .models
            .insert("default".to_owned(), alias(200_000));
        host.config.default_model = Some("default".to_owned());
        host.config.thinking = Some(ThinkingConfig {
            enabled: Some(true),
            effort: Some("high".to_owned()),
        });

        controller
            .refresh_config_after_login(&mut host)
            .await
            .expect("config");

        assert_eq!(host.create_options.expect_lock()[0].model, "default");
        assert_eq!(
            host.create_options.expect_lock()[0]
                .thinking
                .as_ref()
                .map(ThinkingEffort::as_str),
            Some("high")
        );
        let patch = host.patches.last().expect("model patch");
        assert_eq!(patch.model.as_deref(), Some("default"));
        assert_eq!(patch.max_context_tokens, Some(200_000));
        assert!(
            patch
                .available_models
                .as_ref()
                .is_some_and(|models| models.contains_key("default"))
        );
    }

    #[tokio::test]
    async fn missing_default_only_refreshes_available_config_and_logout_zeros_usage() {
        let controller = AuthFlowController;
        let mut host = HostMock::new();
        host.config
            .models
            .insert("other".to_owned(), alias(128_000));
        host.config.default_model = Some("missing".to_owned());

        controller
            .refresh_config_after_login(&mut host)
            .await
            .expect("missing default");
        assert!(host.create_options.expect_lock().is_empty());
        assert!(host.patches[0].available_models.is_some());
        assert_eq!(host.patches[0].model, None);

        host.patches.clear();
        controller
            .refresh_config_after_logout(&mut host)
            .await
            .expect("logout config");
        let patch = &host.patches[0];
        assert_eq!(patch.model.as_deref(), Some(""));
        assert_eq!(
            patch.thinking_effort.as_ref().map(ThinkingEffort::as_str),
            Some("off")
        );
        assert_eq!(
            (patch.context_tokens, patch.max_context_tokens),
            (Some(0), Some(0))
        );
        assert_eq!(patch.context_usage, Some(0.0));
    }

    #[tokio::test]
    async fn provider_refresh_uses_shared_orchestrator_without_reloading_when_unchanged() {
        let controller = AuthFlowController;
        let mut host = HostMock::new();

        let result = controller
            .refresh_oauth_provider_models(&mut host)
            .await
            .expect("refresh");

        assert_eq!(result, RefreshResult::default());
        assert_eq!(*host.events.expect_lock(), ["refresh:get-config"]);
        assert!(host.patches.is_empty());
    }
}
