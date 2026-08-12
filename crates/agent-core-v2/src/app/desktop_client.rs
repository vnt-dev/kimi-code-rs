//! High-level Kimi Code client facade for desktop and other graphical hosts.
//!
//! The facade owns application composition, session lifecycle, managed model
//! configuration, streamed output, and host-mediated interactions.

use std::{
    collections::HashSet,
    num::NonZeroU64,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use indexmap::IndexMap;
use kimi_code_oauth::{
    AuthManagedUsageResult, AuthManagedUserInfoResult, AuthenticatedServiceOptions,
    BoosterWalletInfo, CredentialKind, DeviceAuthorization, DeviceCodeObserver,
    KIMI_CODE_PROVIDER_NAME, KimiHostIdentity, KimiIdentityOptions, KimiOAuthLoginOptions,
    ManagedKimiCodeApplyOptions, ManagedKimiCodeModelInfo, ManagedUserInfo, ManagedUserInfoPhone,
    OAuthManagerError, UsageRow, apply_managed_kimi_code_config, create_kimi_default_headers,
    fetch_managed_kimi_code_models, managed_usage::DEFAULT_KIMI_CODE_BASE_URL,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use tokio::sync::Mutex as AsyncMutex;

pub use super::usage_statistics::{DesktopDailyTokenUsage, DesktopUsageStatistics};

use crate::{
    _base::{
        di::{
            lifecycle::{DisposableHandle, DisposableStore},
            scope::{Scope, ScopeHandle},
        },
        errors::errors::Error2,
    },
    agent::{
        context_memory::protocol_message::ProtocolMessage,
        context_size::{AGENT_CONTEXT_SIZE_SERVICE_ID, AgentContextSizeServiceHandle, ContextSize},
        loop_::{
            AssistantContentEvent, AssistantDeltaEvent, ThinkingDeltaEvent, ToolCallDeltaEvent,
            TurnEndedEvent, TurnStartedEvent, TurnStepCompletedEvent, TurnStepInterruptedEvent,
            TurnStepStartedEvent,
        },
        permission_mode::{
            AGENT_PERMISSION_MODE_SERVICE_ID, config_section::DEFAULT_PERMISSION_MODE_SECTION,
        },
        permission_policy::PermissionMode,
        plan::config_section::DEFAULT_PLAN_MODE_SECTION,
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle, BindAgentInput},
        rpc::{AGENT_RPC_SERVICE_ID, AgentRpcServiceHandle, EmptyPayload, SetPermissionPayload},
        tool_executor::{ToolCallStartedEvent, ToolProgressEvent, ToolResultEvent},
    },
    app::{
        agent_app_runtime::bootstrap_agent_app,
        agent_file_catalog::{
            AgentFileSource, ManagedAgentFile, delete_managed_agent_file, list_managed_agent_files,
            resolve_agent_project_root, save_managed_agent_file,
        },
        auth::{OAuthToolkitContract, OAuthToolkitService, config_section::SERVICES_SECTION},
        bootstrap::{BootstrapInput, ensure_kimi_home, resolve_bootstrap_options},
        capability::{CAPABILITY_SERVICE_ID, CapabilityStatus},
        config::{CONFIG_SERVICE_ID, ConfigServiceContract, ConfigServiceHandle, ConfigTarget},
        cron::{
            CRON_ID_REGEX, CronTask, CronTaskInit, compute_next_cron_run, cron_to_human,
            format_local_iso_with_offset, has_fire_within_years, parse_cron_expression,
        },
        event::event_bus::EVENT_BUS_SERVICE_ID,
        file::{FILE_SERVICE_ID, FileByteStream, FileMeta, FileServiceError, SaveOptions},
        host_folder_browser::{FS_HOST_FOLDER_BROWSER_ID, FsBrowseResponse, FsHomeResponse},
        message_legacy::MESSAGE_LEGACY_SERVICE_ID,
        plugin::{
            GetPluginInfoInput, InstallPluginInput, PLUGIN_SERVICE_ID, PluginInfo,
            PluginInstallOperation, PluginSummary, PluginUpdateStatus, ReloadSummary,
            RemovePluginInput, SetPluginEnabledInput, SetPluginMcpServerEnabledInput,
        },
        session_index::{SESSION_INDEX_SERVICE_ID, SessionListQuery, SessionSummary},
        session_lifecycle::{
            CreateSessionOptions, ForkSessionOptions, SESSION_LIFECYCLE_SERVICE_ID,
        },
        skill_catalog::{SkillSource as CatalogSkillSource, is_user_activatable_skill_type},
        workspace_registry::{
            WORKSPACE_QUERY_SERVICE_ID, WORKSPACE_REGISTRY_SERVICE_ID, Workspace,
        },
    },
    kosong::{
        model::{
            MODEL_CATALOG_SERVICE_ID, Model, ModelCatalogItem,
            contract::{DEFAULT_MODEL_SECTION, MODELS_SECTION, ModelRecord, ModelsSection},
            thinking::THINKING_SECTION,
        },
        protocol::identity::Protocol,
        provider::{
            ENV_MODEL_PROVIDER_KEY, ProviderConfig, ProviderType, ProvidersSection,
            config::PROVIDERS_SECTION,
        },
    },
    os::interface::host_file_system::HOST_FILE_SYSTEM_SERVICE_ID,
    session::{
        agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, MAIN_AGENT_ID, ensure_main_agent},
        agent_profile_catalog::SESSION_AGENT_PROFILE_CATALOG_ID,
        cron::{
            MAX_CRON_JOBS_PER_SESSION, MAX_CRON_PROMPT_BYTES, ONE_SHOT_MAX_FUTURE_MS,
            SESSION_CRON_SERVICE_ID, SessionCronServiceHandle,
        },
        interaction::{Interaction, InteractionKind, SESSION_INTERACTION_SERVICE_ID},
        session_context::SESSION_CONTEXT_ID,
        skill_catalog::SESSION_SKILL_CATALOG_ID,
        todo::SESSION_TODO_SERVICE_ID,
    },
};

pub struct KimiCodeDesktopClient {
    home_dir: PathBuf,
    client_version: String,
    oauth: Arc<OAuthToolkitService>,
    app: Scope,
    models_configured: AtomicBool,
    config_gate: AsyncMutex<()>,
    usage_statistics_gate: AsyncMutex<()>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAuthStatus {
    pub logged_in: bool,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopManagedUsage {
    pub summary: Option<DesktopManagedUsageRow>,
    pub limits: Vec<DesktopManagedUsageRow>,
    pub extra_usage: Option<DesktopBoosterWalletInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopManagedUsageRow {
    pub label: String,
    pub used: f64,
    pub limit: f64,
    pub reset_hint: Option<String>,
}

impl From<UsageRow> for DesktopManagedUsageRow {
    fn from(value: UsageRow) -> Self {
        Self {
            label: value.label,
            used: value.used,
            limit: value.limit,
            reset_hint: value.reset_hint,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopBoosterWalletInfo {
    pub balance_cents: f64,
    pub total_cents: f64,
    pub monthly_charge_limit_enabled: bool,
    pub monthly_charge_limit_cents: f64,
    pub monthly_used_cents: f64,
    pub currency: String,
}

impl From<BoosterWalletInfo> for DesktopBoosterWalletInfo {
    fn from(value: BoosterWalletInfo) -> Self {
        Self {
            balance_cents: value.balance_cents,
            total_cents: value.total_cents,
            monthly_charge_limit_enabled: value.monthly_charge_limit_enabled,
            monthly_charge_limit_cents: value.monthly_charge_limit_cents,
            monthly_used_cents: value.monthly_used_cents,
            currency: value.currency,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopManagedUserInfo {
    pub user_id: String,
    pub nickname: String,
    pub status: String,
    pub region: String,
    pub user_level: i64,
    pub user_level_name: String,
    pub domain: i64,
    pub domain_name: String,
    pub global_id: Option<String>,
    pub bio: Option<String>,
    pub avatar: Option<String>,
    pub username: Option<String>,
    pub email: Option<String>,
    pub phone: Option<DesktopManagedUserInfoPhone>,
    pub created_time: Option<String>,
    pub last_login_time: Option<String>,
}

impl From<ManagedUserInfo> for DesktopManagedUserInfo {
    fn from(value: ManagedUserInfo) -> Self {
        Self {
            user_id: value.user_id,
            nickname: value.nickname,
            status: value.status,
            region: value.region,
            user_level: value.user_level,
            user_level_name: value.user_level_name,
            domain: value.domain,
            domain_name: value.domain_name,
            global_id: value.global_id,
            bio: value.bio,
            avatar: value.avatar,
            username: value.username,
            email: value.email,
            phone: value.phone.map(Into::into),
            created_time: value.created_time,
            last_login_time: value.last_login_time,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopManagedUserInfoPhone {
    pub country_code: String,
    pub number: String,
}

impl From<ManagedUserInfoPhone> for DesktopManagedUserInfoPhone {
    fn from(value: ManagedUserInfoPhone) -> Self {
        Self {
            country_code: value.country_code,
            number: value.number,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopModel {
    pub id: String,
    pub model: String,
    pub provider_id: String,
    pub is_default: bool,
    pub display_name: String,
    pub context_length: u64,
    pub supports_reasoning: bool,
    pub supports_image: bool,
    pub supports_video: bool,
    pub supports_tools: bool,
    pub protocol: String,
    pub support_efforts: Vec<String>,
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAgentSettings {
    pub default_model: Option<String>,
    pub default_permission: PermissionMode,
    pub default_thinking: bool,
    pub default_plan_mode: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopAgentSettingsPatch {
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub default_permission: Option<PermissionMode>,
    #[serde(default)]
    pub default_thinking: Option<bool>,
    #[serde(default)]
    pub default_plan_mode: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopProviderModel {
    pub model: String,
    pub display_name: Option<String>,
    pub max_context_size: u64,
    pub capabilities: Vec<String>,
    pub support_efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub adaptive_thinking: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopProvider {
    pub id: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub has_api_key: bool,
    pub managed: bool,
    pub models: Vec<DesktopProviderModel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopProviderModelInput {
    pub model: String,
    pub display_name: Option<String>,
    pub max_context_size: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub support_efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub adaptive_thinking: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopSaveProviderInput {
    pub original_id: Option<String>,
    pub id: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub replace_api_key: bool,
    pub base_url: String,
    pub default_model: Option<String>,
    pub models: Vec<DesktopProviderModelInput>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSkill {
    pub name: String,
    pub description: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSkillContent {
    pub name: String,
    pub description: String,
    pub source: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopCustomAgentScope {
    App,
    Project,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCustomAgent {
    pub scope: DesktopCustomAgentScope,
    pub relative_path: String,
    pub path: String,
    pub content: String,
    pub name: String,
    pub description: Option<String>,
    pub when_to_use: Option<String>,
    pub is_override: bool,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub subagents: Option<Vec<String>>,
    pub model: Option<String>,
    pub valid: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopSaveCustomAgentInput {
    pub workspace_id: String,
    pub scope: DesktopCustomAgentScope,
    #[serde(default)]
    pub relative_path: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopDeleteCustomAgentInput {
    pub workspace_id: String,
    pub scope: DesktopCustomAgentScope,
    pub relative_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCronTask {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub created_at: f64,
    pub recurring: bool,
    pub last_fired_at: Option<f64>,
    pub human_schedule: String,
    pub next_fire_at: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopCreateCronTaskInput {
    pub session_id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopDeleteCronTaskInput {
    pub session_id: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWorkspace {
    pub id: String,
    pub root: String,
    pub name: String,
    pub created_at: i64,
    pub last_opened_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopPrepareSessionRequest {
    #[serde(default)]
    pub session_id: Option<String>,
    pub work_dir: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub permission: Option<PermissionMode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreparedSession {
    pub session_id: String,
    pub agent_id: String,
    pub model: String,
    pub thinking_level: String,
    pub permission_mode: PermissionMode,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopChatRequest {
    pub prompt: String,
    pub model: String,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopChatEvent {
    TurnStarted(TurnStartedEvent),
    TurnEnded(TurnEndedEvent),
    StepStarted(TurnStepStartedEvent),
    StepCompleted(TurnStepCompletedEvent),
    StepInterrupted(TurnStepInterruptedEvent),
    AssistantDelta(AssistantDeltaEvent),
    AssistantContent(AssistantContentEvent),
    ThinkingDelta(ThinkingDeltaEvent),
    ToolCallDelta(ToolCallDeltaEvent),
    ToolCallStarted(ToolCallStartedEvent),
    ToolProgress(ToolProgressEvent),
    ToolResult(ToolResultEvent),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopChatResult {
    pub content: String,
    pub thinking: String,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesktopMessagePage {
    pub items: Vec<ProtocolMessage>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInteraction {
    pub id: String,
    pub kind: String,
    pub payload: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCompactionEvent {
    pub phase: String,
    pub trigger: Option<String>,
    pub compacted_count: Option<f64>,
    pub tokens_before: Option<f64>,
    pub tokens_after: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopContextUsage {
    pub context_tokens: f64,
    pub measured_tokens: f64,
    pub estimated_tokens: f64,
    pub max_context_tokens: u64,
    pub usage_ratio: f64,
}

struct CallbackObserver {
    callback: Arc<dyn Fn(DesktopDeviceCode) + Send + Sync>,
}

#[async_trait]
impl DeviceCodeObserver for CallbackObserver {
    async fn on_device_code(
        &self,
        authorization: &DeviceAuthorization,
    ) -> Result<(), OAuthManagerError> {
        (self.callback)(DesktopDeviceCode {
            user_code: authorization.user_code.clone(),
            verification_uri: authorization.verification_uri.clone(),
            verification_uri_complete: authorization.verification_uri_complete.clone(),
            expires_in: authorization.expires_in,
        });
        Ok(())
    }
}

impl KimiCodeDesktopClient {
    pub fn bootstrap(client_version: impl Into<String>) -> Result<Self, String> {
        Self::from_bootstrap_input(BootstrapInput {
            client_version: Some(client_version.into()),
            ..BootstrapInput::default()
        })
    }

    pub fn new(
        home_dir: impl Into<PathBuf>,
        client_version: impl Into<String>,
    ) -> Result<Self, String> {
        Self::from_bootstrap_input(BootstrapInput {
            home_dir: Some(home_dir.into()),
            client_version: Some(client_version.into()),
            ..BootstrapInput::default()
        })
    }

    fn from_bootstrap_input(input: BootstrapInput) -> Result<Self, String> {
        let options =
            resolve_bootstrap_options(input.clone()).map_err(|error| error.to_string())?;
        ensure_kimi_home(&options.home_dir).map_err(|error| error.to_string())?;
        let oauth =
            OAuthToolkitService::new(&options.home_dir).map_err(|error| error.to_string())?;
        let app = bootstrap_agent_app(input)?;

        Ok(Self {
            home_dir: options.home_dir,
            client_version: options.client_version,
            oauth: Arc::new(oauth),
            app,
            models_configured: AtomicBool::new(false),
            config_gate: AsyncMutex::new(()),
            usage_statistics_gate: AsyncMutex::new(()),
        })
    }

    pub async fn usage_statistics(&self) -> Result<DesktopUsageStatistics, String> {
        let _guard = self.usage_statistics_gate.lock().await;
        super::usage_statistics::collect_usage_statistics(self.home_dir.clone()).await
    }

    pub async fn auth_status(&self) -> Result<DesktopAuthStatus, String> {
        let token = self
            .oauth
            .get_cached_access_token(Some(KIMI_CODE_PROVIDER_NAME), None)
            .await
            .map_err(|error| error.to_string())?;
        Ok(DesktopAuthStatus {
            logged_in: token.is_some(),
            provider: KIMI_CODE_PROVIDER_NAME.to_owned(),
        })
    }

    pub async fn managed_usage(&self) -> Result<DesktopManagedUsage, String> {
        match self
            .oauth
            .get_managed_usage(
                Some(KIMI_CODE_PROVIDER_NAME),
                AuthenticatedServiceOptions::default(),
            )
            .await
        {
            AuthManagedUsageResult::Ok {
                summary,
                limits,
                extra_usage,
            } => Ok(DesktopManagedUsage {
                summary: summary.map(Into::into),
                limits: limits.into_iter().map(Into::into).collect(),
                extra_usage: extra_usage.map(Into::into),
            }),
            AuthManagedUsageResult::Error { message, .. } => Err(message),
        }
    }

    pub async fn managed_user_info(&self) -> Result<DesktopManagedUserInfo, String> {
        match self
            .oauth
            .get_managed_user_info(
                Some(KIMI_CODE_PROVIDER_NAME),
                AuthenticatedServiceOptions::default(),
            )
            .await
        {
            AuthManagedUserInfoResult::Ok { user_info } => Ok((*user_info).into()),
            AuthManagedUserInfoResult::Error { message, .. } => Err(message),
        }
    }

    pub async fn login(
        &self,
        on_device_code: Arc<dyn Fn(DesktopDeviceCode) + Send + Sync>,
    ) -> Result<DesktopAuthStatus, String> {
        let observer = CallbackObserver {
            callback: on_device_code,
        };
        self.oauth
            .login(
                Some(KIMI_CODE_PROVIDER_NAME),
                KimiOAuthLoginOptions {
                    on_device_code: Some(&observer),
                    ..KimiOAuthLoginOptions::default()
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        self.models_configured.store(false, Ordering::Release);
        self.auth_status().await
    }

    pub async fn logout(&self) -> Result<DesktopAuthStatus, String> {
        self.oauth
            .logout(Some(KIMI_CODE_PROVIDER_NAME), None)
            .await
            .map_err(|error| error.to_string())?;
        self.models_configured.store(false, Ordering::Release);
        self.auth_status().await
    }

    pub async fn list_models(&self) -> Result<Vec<DesktopModel>, String> {
        self.app
            .get(CONFIG_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .ready()
            .await
            .map_err(|error| error.to_string())?;
        self.configured_desktop_models().await
    }

    pub async fn refresh_models(&self) -> Result<Vec<DesktopModel>, String> {
        let models = self.fetch_models().await?;
        self.configure_models(&models).await?;
        self.configured_desktop_models().await
    }

    pub async fn list_providers(&self) -> Result<Vec<DesktopProvider>, String> {
        let config = self
            .app
            .get(CONFIG_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        config.ready().await.map_err(|error| error.to_string())?;
        let providers = user_section::<ProvidersSection>(&config, PROVIDERS_SECTION)?;
        let models = user_section::<ModelsSection>(&config, MODELS_SECTION)?;
        Ok(desktop_providers(&providers, &models))
    }

    pub async fn save_provider(
        &self,
        input: DesktopSaveProviderInput,
    ) -> Result<DesktopProvider, String> {
        let input = validate_provider_input(input)?;
        let _guard = self.config_gate.lock().await;
        let config = self
            .app
            .get(CONFIG_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        config.ready().await.map_err(|error| error.to_string())?;

        let old_providers_value = config.inspect(PROVIDERS_SECTION).user_value;
        let old_models_value = config.inspect(MODELS_SECTION).user_value;
        let old_default_value = config.inspect(DEFAULT_MODEL_SECTION).user_value;
        let mut providers = section_from_value::<ProvidersSection>(old_providers_value.as_ref())?;
        let mut models = section_from_value::<ModelsSection>(old_models_value.as_ref())?;
        let original_id = input.original_id.as_deref();
        if original_id.is_some_and(is_managed_provider_id) || is_managed_provider_id(&input.id) {
            return Err("Managed OAuth providers cannot be edited here.".to_owned());
        }
        if original_id.is_none() && providers.contains_key(&input.id) {
            return Err(format!("Provider `{}` already exists.", input.id));
        }
        if let Some(original_id) = original_id
            && original_id != input.id
            && providers.contains_key(&input.id)
        {
            return Err(format!("Provider `{}` already exists.", input.id));
        }

        let previous_provider = match original_id {
            Some(original_id) => providers
                .shift_remove(original_id)
                .ok_or_else(|| format!("Provider `{original_id}` does not exist."))?,
            None => ProviderConfig::default(),
        };
        if let Some(original_id) = original_id {
            models.retain(|_, model| model.provider.as_deref() != Some(original_id));
        }

        let protocol = protocol_for_provider_type(&input.provider_type)?;
        let mut provider = previous_provider;
        provider.provider_type = Some(ProviderType::new(input.provider_type.clone()));
        provider.base_url = Some(input.base_url.clone());
        provider.default_model = input
            .default_model
            .as_ref()
            .map(|model| provider_model_config_id(&input.id, model));
        if input.replace_api_key || original_id.is_none() {
            provider.api_key = input.api_key.clone();
        }
        providers.insert(input.id.clone(), provider);

        for model in &input.models {
            let config_id = provider_model_config_id(&input.id, &model.model);
            models.insert(
                config_id,
                ModelRecord {
                    provider: Some(input.id.clone()),
                    model: Some(model.model.clone()),
                    protocol: Some(protocol),
                    max_context_size: NonZeroU64::new(model.max_context_size),
                    display_name: model.display_name.clone(),
                    capabilities: (!model.capabilities.is_empty())
                        .then(|| model.capabilities.clone()),
                    support_efforts: (!model.support_efforts.is_empty())
                        .then(|| model.support_efforts.clone()),
                    default_effort: model.default_effort.clone(),
                    adaptive_thinking: model.adaptive_thinking,
                    ..ModelRecord::default()
                },
            );
        }

        let old_default = old_default_value.as_ref().and_then(Value::as_str);
        let old_default_belonged_to_provider = original_id.is_some()
            && old_default.is_some_and(|model_id| {
                section_from_value::<ModelsSection>(old_models_value.as_ref())
                    .ok()
                    .and_then(|models| models.get(model_id).cloned())
                    .and_then(|model| model.provider)
                    .as_deref()
                    == original_id
            });
        let next_default = if old_default_belonged_to_provider {
            input
                .default_model
                .as_ref()
                .or_else(|| input.models.first().map(|model| &model.model))
                .map(|model| Value::String(provider_model_config_id(&input.id, model)))
        } else {
            old_default_value.clone()
        };

        replace_provider_sections(
            &config,
            old_providers_value,
            old_models_value,
            Some(serde_json::to_value(&providers).map_err(|error| error.to_string())?),
            Some(serde_json::to_value(&models).map_err(|error| error.to_string())?),
            next_default,
        )
        .await?;
        self.models_configured.store(true, Ordering::Release);
        desktop_providers(&providers, &models)
            .into_iter()
            .find(|provider| provider.id == input.id)
            .ok_or_else(|| "The saved provider could not be reloaded.".to_owned())
    }

    pub async fn delete_provider(&self, id: String) -> Result<(), String> {
        let id = id.trim();
        if id.is_empty() {
            return Err("A provider id is required.".to_owned());
        }
        if is_managed_provider_id(id) {
            return Err("Managed OAuth providers cannot be deleted here.".to_owned());
        }
        let _guard = self.config_gate.lock().await;
        let config = self
            .app
            .get(CONFIG_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        config.ready().await.map_err(|error| error.to_string())?;
        let old_providers_value = config.inspect(PROVIDERS_SECTION).user_value;
        let old_models_value = config.inspect(MODELS_SECTION).user_value;
        let old_default_value = config.inspect(DEFAULT_MODEL_SECTION).user_value;
        let mut providers = section_from_value::<ProvidersSection>(old_providers_value.as_ref())?;
        if providers.shift_remove(id).is_none() {
            return Ok(());
        }
        let mut models = section_from_value::<ModelsSection>(old_models_value.as_ref())?;
        models.retain(|_, model| model.provider.as_deref() != Some(id));
        let next_default = old_default_value
            .as_ref()
            .and_then(Value::as_str)
            .map_or_else(
                || old_default_value.clone(),
                |model_id| {
                    if models.contains_key(model_id) {
                        old_default_value.clone()
                    } else {
                        models.keys().next().cloned().map(Value::String)
                    }
                },
            );
        replace_provider_sections(
            &config,
            old_providers_value,
            old_models_value,
            Some(serde_json::to_value(&providers).map_err(|error| error.to_string())?),
            Some(serde_json::to_value(&models).map_err(|error| error.to_string())?),
            next_default,
        )
        .await?;
        self.models_configured
            .store(!models.is_empty(), Ordering::Release);
        Ok(())
    }

    async fn configured_desktop_models(&self) -> Result<Vec<DesktopModel>, String> {
        let catalog = self
            .app
            .get(MODEL_CATALOG_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let default_model = self
            .app
            .get(CONFIG_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .get(DEFAULT_MODEL_SECTION)
            .and_then(|value| value.as_str().map(str::to_owned));
        let mut models = catalog
            .list_models()
            .await
            .into_iter()
            .map(|item| {
                let resolved = catalog.get(&item.model).ok();
                let is_default = default_model.as_deref() == Some(item.model.as_str());
                map_desktop_model(item, resolved.as_deref(), is_default)
            })
            .collect::<Vec<_>>();
        models.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        Ok(models)
    }

    pub async fn set_default_model(&self, model: &str) -> Result<(), String> {
        self.ensure_models_configured().await?;
        self.app
            .get(MODEL_CATALOG_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .set_default_model(model)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub async fn get_agent_settings(&self) -> Result<DesktopAgentSettings, String> {
        let config = self
            .app
            .get(CONFIG_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        config.ready().await.map_err(|error| error.to_string())?;
        Ok(agent_settings_from_config(config.0.as_ref()))
    }

    pub async fn update_agent_settings(
        &self,
        patch: DesktopAgentSettingsPatch,
    ) -> Result<DesktopAgentSettings, String> {
        if let Some(model) = patch.default_model.as_deref() {
            let model = model.trim();
            if model.is_empty() {
                return Err("A default model is required.".to_owned());
            }
            self.set_default_model(model).await?;
        }

        let _guard = self.config_gate.lock().await;
        let config = self
            .app
            .get(CONFIG_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        config.ready().await.map_err(|error| error.to_string())?;

        if let Some(permission) = patch.default_permission {
            config
                .replace(
                    DEFAULT_PERMISSION_MODE_SECTION,
                    Some(serde_json::to_value(permission).map_err(|error| error.to_string())?),
                    ConfigTarget::User,
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        if let Some(enabled) = patch.default_thinking {
            config
                .set(
                    THINKING_SECTION,
                    Some(serde_json::json!({ "enabled": enabled })),
                    ConfigTarget::User,
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        if let Some(enabled) = patch.default_plan_mode {
            config
                .replace(
                    DEFAULT_PLAN_MODE_SECTION,
                    Some(Value::Bool(enabled)),
                    ConfigTarget::User,
                )
                .await
                .map_err(|error| error.to_string())?;
        }

        Ok(agent_settings_from_config(config.0.as_ref()))
    }

    pub async fn upload_file(
        &self,
        filename: &str,
        media_type: &str,
        data: Vec<u8>,
    ) -> Result<FileMeta, String> {
        let files = self
            .app
            .get(FILE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let source: FileByteStream = Box::pin(futures_util::stream::once(async move {
            Ok::<_, FileServiceError>(data)
        }));
        files
            .save(
                source,
                filename,
                Some(SaveOptions {
                    mime_type: Some(if media_type.trim().is_empty() {
                        "application/octet-stream".into()
                    } else {
                        media_type.into()
                    }),
                    ..SaveOptions::default()
                }),
            )
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn list_workspaces(&self) -> Result<Vec<DesktopWorkspace>, String> {
        let registry = self
            .app
            .get(WORKSPACE_REGISTRY_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        registry
            .list()
            .await
            .map(|workspaces| workspaces.into_iter().map(map_desktop_workspace).collect())
            .map_err(|error| error.to_string())
    }

    pub async fn folder_home(&self) -> Result<FsHomeResponse, String> {
        self.app
            .get(FS_HOST_FOLDER_BROWSER_ID)
            .map_err(|error| error.to_string())?
            .home()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn browse_folders(
        &self,
        absolute_path: Option<&str>,
    ) -> Result<FsBrowseResponse, String> {
        self.app
            .get(FS_HOST_FOLDER_BROWSER_ID)
            .map_err(|error| error.to_string())?
            .browse(absolute_path)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn create_or_touch_workspace(
        &self,
        root: &str,
        name: Option<&str>,
    ) -> Result<DesktopWorkspace, String> {
        let root = root.trim();
        if root.is_empty() {
            return Err("A workspace directory is required.".to_owned());
        }
        let registry = self
            .app
            .get(WORKSPACE_REGISTRY_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        registry
            .create_or_touch(root, name)
            .await
            .map(map_desktop_workspace)
            .map_err(|error| error.to_string())
    }

    pub async fn remove_workspace(&self, workspace_id: &str) -> Result<(), String> {
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() {
            return Err("A workspace id is required.".to_owned());
        }
        let registry = self
            .app
            .get(WORKSPACE_REGISTRY_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        registry
            .delete(workspace_id)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn list_workspace_sessions(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<SessionSummary>, String> {
        let query = self
            .app
            .get(WORKSPACE_QUERY_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        query
            .list_recent_sessions(workspace_id)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn list_archived_sessions(&self) -> Result<Vec<SessionSummary>, String> {
        let index = self
            .app
            .get(SESSION_INDEX_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let page = index
            .list(SessionListQuery {
                include_archived: Some(true),
                ..SessionListQuery::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(page
            .items
            .into_iter()
            .filter(|session| session.archived)
            .collect())
    }

    pub async fn delete_archived_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<String>, String> {
        let mut seen = HashSet::new();
        let session_ids = session_ids
            .iter()
            .map(|session_id| session_id.trim())
            .filter(|session_id| !session_id.is_empty())
            .filter(|session_id| seen.insert((*session_id).to_owned()))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }

        let index = self
            .app
            .get(SESSION_INDEX_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let mut existing = Vec::with_capacity(session_ids.len());
        for session_id in session_ids {
            let Some(session) = index
                .get(&session_id)
                .await
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            if !session.archived {
                return Err(format!(
                    "Session `{session_id}` must be archived before it can be deleted."
                ));
            }
            existing.push(session_id);
        }

        let lifecycle = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let mut deleted = Vec::with_capacity(existing.len());
        for session_id in existing {
            if lifecycle
                .delete_archived(&session_id)
                .await
                .map_err(|error| error.to_string())?
            {
                deleted.push(session_id);
            }
        }
        Ok(deleted)
    }

    pub async fn fork_session(&self, session_id: &str) -> Result<String, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("A session id is required.".to_owned());
        }

        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let forked = sessions
            .fork(ForkSessionOptions {
                source_session_id: session_id.to_owned(),
                ..ForkSessionOptions::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        let context = forked
            .get(SESSION_CONTEXT_ID)
            .map_err(|error| error.to_string())?;
        Ok(context.session_id.clone())
    }

    pub async fn list_session_skills(&self, session_id: &str) -> Result<Vec<DesktopSkill>, String> {
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "the session is not active; prepare it first".to_owned())?;
        let skills = session
            .get(SESSION_SKILL_CATALOG_ID)
            .map_err(|error| error.to_string())?;
        skills.ready().await.map_err(|error| error.to_string())?;

        Ok(skills
            .catalog()
            .list_skills()
            .into_iter()
            .filter(|skill| is_user_activatable_skill_type(skill.metadata.kind.as_deref()))
            .map(|skill| DesktopSkill {
                name: skill.name,
                description: skill.description,
                source: match skill.source {
                    CatalogSkillSource::Project => "project",
                    CatalogSkillSource::User => "user",
                    CatalogSkillSource::Extra => "extra",
                    CatalogSkillSource::Builtin => "builtin",
                }
                .to_owned(),
            })
            .collect())
    }

    pub async fn get_session_skill_content(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<DesktopSkillContent, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A skill name is required.".to_owned());
        }

        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "the session is not active; prepare it first".to_owned())?;
        let skills = session
            .get(SESSION_SKILL_CATALOG_ID)
            .map_err(|error| error.to_string())?;
        skills.ready().await.map_err(|error| error.to_string())?;
        let skill = skills
            .catalog()
            .get_skill(name)
            .ok_or_else(|| format!("Skill `{name}` was not found."))?;

        if !is_user_activatable_skill_type(skill.metadata.kind.as_deref()) {
            return Err(format!("Skill `{name}` is not available for direct use."));
        }

        Ok(DesktopSkillContent {
            name: skill.name,
            description: skill.description,
            source: match skill.source {
                CatalogSkillSource::Project => "project",
                CatalogSkillSource::User => "user",
                CatalogSkillSource::Extra => "extra",
                CatalogSkillSource::Builtin => "builtin",
            }
            .to_owned(),
            path: skill.path,
            content: skill.content,
        })
    }

    pub async fn list_custom_agents(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DesktopCustomAgent>, String> {
        let mut agents = Vec::new();
        for scope in [
            DesktopCustomAgentScope::App,
            DesktopCustomAgentScope::Project,
        ] {
            let root = self.custom_agent_root(workspace_id, scope).await?;
            let source = custom_agent_source(scope);
            agents.extend(
                list_managed_agent_files(&root, source)
                    .await?
                    .into_iter()
                    .map(|file| map_desktop_custom_agent(scope, file)),
            );
        }
        agents.sort_by(|left, right| {
            custom_agent_scope_order(left.scope)
                .cmp(&custom_agent_scope_order(right.scope))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.relative_path.cmp(&right.relative_path))
        });
        Ok(agents)
    }

    pub async fn save_custom_agent(
        &self,
        input: DesktopSaveCustomAgentInput,
    ) -> Result<DesktopCustomAgent, String> {
        let root = self
            .custom_agent_root(&input.workspace_id, input.scope)
            .await?;
        let file = save_managed_agent_file(
            &root,
            custom_agent_source(input.scope),
            input.relative_path.as_deref(),
            &input.content,
        )
        .await?;
        self.reload_active_agent_catalogs().await?;
        Ok(map_desktop_custom_agent(input.scope, file))
    }

    pub async fn delete_custom_agent(
        &self,
        input: DesktopDeleteCustomAgentInput,
    ) -> Result<(), String> {
        let root = self
            .custom_agent_root(&input.workspace_id, input.scope)
            .await?;
        delete_managed_agent_file(&root, &input.relative_path).await?;
        self.reload_active_agent_catalogs().await
    }

    pub async fn list_cron_tasks(&self, session_id: &str) -> Result<Vec<DesktopCronTask>, String> {
        let cron = self.session_cron_service(session_id)?;
        Ok(cron
            .list()
            .into_iter()
            .map(|task| map_desktop_cron_task(&cron, task))
            .collect())
    }

    pub async fn create_cron_task(
        &self,
        input: DesktopCreateCronTaskInput,
    ) -> Result<DesktopCronTask, String> {
        let cron = self.session_cron_service(&input.session_id)?;
        if cron.is_disabled() {
            return Err("Cron scheduling is disabled (KIMI_DISABLE_CRON=1).".into());
        }
        if input.prompt.is_empty() {
            return Err("A non-empty prompt is required.".into());
        }
        let prompt_bytes = input.prompt.len();
        if prompt_bytes > MAX_CRON_PROMPT_BYTES {
            return Err(format!(
                "Prompt exceeds {MAX_CRON_PROMPT_BYTES} bytes (got {prompt_bytes})."
            ));
        }
        if cron.list().len() >= MAX_CRON_JOBS_PER_SESSION {
            return Err(format!(
                "Cron job cap reached (max {MAX_CRON_JOBS_PER_SESSION} per session)."
            ));
        }

        let normalized_cron = input.cron.split_whitespace().collect::<Vec<_>>().join(" ");
        let parsed = parse_cron_expression(&normalized_cron)
            .map_err(|error| format!("Invalid cron expression: {error}"))?;
        let now = cron.now();
        if !has_fire_within_years(&parsed, 5.0, now) {
            return Err(format!(
                "Cron expression {normalized_cron:?} has no fire within 5 years; refusing to schedule."
            ));
        }
        if !input.recurring
            && let Some(first_fire) = compute_next_cron_run(&parsed, now)
            && first_fire - now > ONE_SHOT_MAX_FUTURE_MS
        {
            return Err(format!(
                "One-shot cron {normalized_cron:?} would not fire until {} (more than a year out).",
                format_local_iso_with_offset(first_fire)
            ));
        }

        let task = cron
            .add_task(CronTaskInit {
                cron: normalized_cron,
                prompt: input.prompt,
                recurring: Some(input.recurring),
                last_fired_at: None,
                tags: None,
            })
            .map_err(|error| error.to_string())?;
        cron.emit_scheduled(&task, None);
        Ok(map_desktop_cron_task(&cron, task))
    }

    pub async fn delete_cron_task(&self, input: DesktopDeleteCronTaskInput) -> Result<(), String> {
        let id = input.id.trim();
        if !CRON_ID_REGEX.is_match(id) {
            return Err("The cron task id must be a valid ULID.".into());
        }
        let cron = self.session_cron_service(&input.session_id)?;
        let removed = cron
            .remove_tasks(&[id.to_owned()])
            .map_err(|error| error.to_string())?;
        if removed.is_empty() {
            return Err(format!("No cron job with id {id}."));
        }
        cron.emit_deleted(id, None);
        Ok(())
    }

    fn session_cron_service(&self, session_id: &str) -> Result<SessionCronServiceHandle, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("A session id is required.".into());
        }
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "the session is not active; prepare it first".to_owned())?;
        session
            .get(SESSION_CRON_SERVICE_ID)
            .map(|service| (*service).clone())
            .map_err(|error| error.to_string())
    }

    async fn custom_agent_root(
        &self,
        workspace_id: &str,
        scope: DesktopCustomAgentScope,
    ) -> Result<PathBuf, String> {
        match scope {
            DesktopCustomAgentScope::App => Ok(self.home_dir.join("agents")),
            DesktopCustomAgentScope::Project => {
                let workspace_id = workspace_id.trim();
                if workspace_id.is_empty() {
                    return Err("A workspace id is required for project agents.".into());
                }
                let registry = self
                    .app
                    .get(WORKSPACE_REGISTRY_SERVICE_ID)
                    .map_err(|error| error.to_string())?;
                let workspace = registry
                    .get(workspace_id)
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Workspace `{workspace_id}` was not found."))?;
                let fs = self
                    .app
                    .get(HOST_FILE_SYSTEM_SERVICE_ID)
                    .map_err(|error| error.to_string())?;
                let project_root =
                    resolve_agent_project_root(fs.0.as_ref(), Path::new(&workspace.root), None)
                        .await
                        .map_err(|error| error.to_string())?;
                Ok(project_root.join(".kimi-code/agents"))
            }
        }
    }

    async fn reload_active_agent_catalogs(&self) -> Result<(), String> {
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        for session in sessions.list() {
            let catalog = session
                .get(SESSION_AGENT_PROFILE_CATALOG_ID)
                .map_err(|error| error.to_string())?;
            catalog.reload().await.map_err(|error| {
                format!(
                    "Agent file was updated, but session {} could not reload it: {error}",
                    session.id()
                )
            })?;
        }
        Ok(())
    }

    pub async fn list_plugins(&self) -> Result<Vec<PluginSummary>, String> {
        self.app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .list_plugins()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn install_plugin(&self, source: String) -> Result<PluginSummary, String> {
        self.app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .install_plugin(InstallPluginInput { source })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn install_plugin_in_background(
        &self,
        source: String,
        operation_id: String,
    ) -> Result<(), String> {
        let plugins = self
            .app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        Arc::clone(&plugins.0)
            .install_plugin_in_background(InstallPluginInput { source }, operation_id)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn plugin_install_progress(
        &self,
        operation_id: String,
    ) -> Result<Option<PluginInstallOperation>, String> {
        let plugins = self
            .app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        Ok(plugins.plugin_install_progress(&operation_id))
    }

    pub async fn list_plugin_install_operations(
        &self,
    ) -> Result<Vec<PluginInstallOperation>, String> {
        let plugins = self
            .app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        Ok(plugins.list_plugin_install_operations())
    }

    pub async fn set_plugin_enabled(&self, id: String, enabled: bool) -> Result<(), String> {
        self.app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .set_plugin_enabled(SetPluginEnabledInput { id, enabled })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn set_plugin_mcp_server_enabled(
        &self,
        id: String,
        server: String,
        enabled: bool,
    ) -> Result<(), String> {
        self.app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .set_plugin_mcp_server_enabled(SetPluginMcpServerEnabledInput {
                id,
                server,
                enabled,
            })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn remove_plugin(&self, id: String) -> Result<(), String> {
        self.app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .remove_plugin(RemovePluginInput { id })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn reload_plugins(&self) -> Result<ReloadSummary, String> {
        self.app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .reload_plugins()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn get_plugin_info(&self, id: String) -> Result<PluginInfo, String> {
        self.app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .get_plugin_info(GetPluginInfoInput { id })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn check_plugin_updates(&self) -> Result<Vec<PluginUpdateStatus>, String> {
        self.app
            .get(PLUGIN_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .check_updates()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn list_capabilities(&self) -> Result<Vec<CapabilityStatus>, String> {
        self.app
            .get(CAPABILITY_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .list_capabilities()
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn get_capability(&self, id: String) -> Result<CapabilityStatus, String> {
        self.app
            .get(CAPABILITY_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .get_capability(&id)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn install_capability(&self, id: String) -> Result<CapabilityStatus, String> {
        self.app
            .get(CAPABILITY_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .install_capability(&id)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn archive_session(&self, session_id: &str) -> Result<(), String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("A session id is required.".to_owned());
        }
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        if sessions
            .resume(session_id)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err(format!("Session `{session_id}` was not found."));
        }
        sessions
            .archive(session_id)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn restore_session(&self, session_id: &str) -> Result<SessionSummary, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("A session id is required.".to_owned());
        }
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        if sessions
            .restore(session_id)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err(format!("Session `{session_id}` was not found."));
        }
        self.app
            .get(SESSION_INDEX_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .get(session_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Session `{session_id}` was not found after restoring it."))
    }

    pub async fn prepare_session(
        &self,
        request: DesktopPrepareSessionRequest,
    ) -> Result<DesktopPreparedSession, String> {
        let work_dir = request.work_dir.trim();
        if work_dir.is_empty() {
            return Err("A workspace directory is required.".to_owned());
        }
        self.ensure_models_configured().await?;

        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let requested_session_id = request
            .session_id
            .as_deref()
            .filter(|session_id| !session_id.trim().is_empty());
        let creating = requested_session_id.is_none();
        let session = if let Some(session_id) = requested_session_id {
            if let Some(session) = sessions.get(session_id) {
                session
            } else {
                sessions
                    .resume(session_id)
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Session `{session_id}` was not found."))?
            }
        } else {
            sessions
                .create(CreateSessionOptions {
                    work_dir: work_dir.to_owned(),
                    main_agent_binding: Some(BindAgentInput {
                        profile: "agent".into(),
                        model: request.model.clone(),
                        thinking: request.thinking.clone(),
                        strict_thinking: None,
                        cwd: Some(work_dir.to_owned()),
                    }),
                    ..CreateSessionOptions::default()
                })
                .await
                .map_err(|error| error.to_string())?
        };

        let agent = ensure_main_agent(&session, None)
            .await
            .map_err(|error| error.to_string())?;
        let rpc = agent
            .get(AGENT_RPC_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        if creating && let Some(permission) = request.permission {
            rpc.set_permission(SetPermissionPayload { mode: permission })
                .await
                .map_err(|error| error.to_string())?;
        }
        let model = rpc
            .get_model(EmptyPayload {})
            .await
            .map_err(|error| error.to_string())?;
        let thinking_level = agent
            .get(AGENT_PROFILE_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .get_effective_thinking_level()
            .map_err(|error| error.to_string())?
            .to_string();
        let permission_mode = agent
            .get(AGENT_PERMISSION_MODE_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .mode();

        Ok(DesktopPreparedSession {
            session_id: session.id().to_owned(),
            agent_id: agent.id().to_owned(),
            model,
            thinking_level,
            permission_mode,
        })
    }

    pub async fn agent_rpc(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<AgentRpcServiceHandle, String> {
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "the session is not active; prepare it first".to_owned())?;
        let agent = if agent_id == MAIN_AGENT_ID {
            ensure_main_agent(&session, None)
                .await
                .map_err(|error| error.to_string())?
        } else {
            session
                .get(AGENT_LIFECYCLE_SERVICE_ID)
                .map_err(|error| error.to_string())?
                .get(agent_id)
                .ok_or_else(|| format!("Agent `{agent_id}` was not found."))?
        };
        agent
            .get(AGENT_RPC_SERVICE_ID)
            .map(|service| (*service).clone())
            .map_err(|error| error.to_string())
    }

    pub async fn subscribe_agent_events(
        &self,
        session_id: &str,
        agent_id: &str,
        on_event: Arc<dyn Fn(String, Value) + Send + Sync>,
        on_interactions: Arc<dyn Fn(Vec<DesktopInteraction>) + Send + Sync>,
    ) -> Result<DisposableHandle, String> {
        self.subscribe_agent_events_inner(session_id, agent_id, on_event, on_interactions, false)
            .await
    }

    pub async fn subscribe_agent_events_with_replay(
        &self,
        session_id: &str,
        agent_id: &str,
        on_event: Arc<dyn Fn(String, Value) + Send + Sync>,
        on_interactions: Arc<dyn Fn(Vec<DesktopInteraction>) + Send + Sync>,
    ) -> Result<DisposableHandle, String> {
        self.subscribe_agent_events_inner(session_id, agent_id, on_event, on_interactions, true)
            .await
    }

    async fn subscribe_agent_events_inner(
        &self,
        session_id: &str,
        agent_id: &str,
        on_event: Arc<dyn Fn(String, Value) + Send + Sync>,
        on_interactions: Arc<dyn Fn(Vec<DesktopInteraction>) + Send + Sync>,
        replay: bool,
    ) -> Result<DisposableHandle, String> {
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| "the session is not active; prepare it first".to_owned())?;
        let agent = if agent_id == MAIN_AGENT_ID {
            ensure_main_agent(&session, None)
                .await
                .map_err(|error| error.to_string())?
        } else {
            session
                .get(AGENT_LIFECYCLE_SERVICE_ID)
                .map_err(|error| error.to_string())?
                .get(agent_id)
                .ok_or_else(|| format!("Agent `{agent_id}` was not found."))?
        };

        let subscriptions = Arc::new(DisposableStore::new());
        if agent_id == MAIN_AGENT_ID {
            let lifecycle = session
                .get(AGENT_LIFECYCLE_SERVICE_ID)
                .map_err(|error| error.to_string())?;
            let attached = Arc::new(Mutex::new(HashSet::new()));
            let subscriptions_for_create = Arc::clone(&subscriptions);
            let attached_for_create = Arc::clone(&attached);
            let on_event_for_create = Arc::clone(&on_event);
            subscriptions.add(lifecycle.on_did_create().subscribe(move |handle| {
                let _ = attach_desktop_agent_events(
                    &subscriptions_for_create,
                    &attached_for_create,
                    handle,
                    &on_event_for_create,
                    replay,
                );
            }));
            let attached_for_dispose = Arc::clone(&attached);
            subscriptions.add(
                lifecycle
                    .on_did_dispose()
                    .subscribe(move |disposed_agent_id| {
                        attached_for_dispose
                            .lock()
                            .unwrap()
                            .remove(disposed_agent_id);
                    }),
            );
            for handle in lifecycle.list(None) {
                attach_desktop_agent_events(&subscriptions, &attached, &handle, &on_event, replay)?;
            }
        } else {
            let attached = Mutex::new(HashSet::new());
            attach_desktop_agent_events(&subscriptions, &attached, &agent, &on_event, replay)?;
        }

        let todo = session
            .get(SESSION_TODO_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let todo_agent_id = agent_id.to_owned();
        subscriptions.add(todo.on_did_change().subscribe(move |todos| {
            on_event(
                todo_agent_id.clone(),
                serde_json::json!({
                    "type": "todo.updated",
                    "todos": todos,
                }),
            );
        }));

        let interaction = session
            .get(SESSION_INTERACTION_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        on_interactions(map_desktop_interactions(
            interaction.list_pending(None).await,
        ));
        let interaction_for_updates = interaction.clone();
        subscriptions.add(interaction.on_did_change_pending().subscribe(move |_| {
            let interaction = interaction_for_updates.clone();
            let on_interactions = Arc::clone(&on_interactions);
            tokio::spawn(async move {
                on_interactions(map_desktop_interactions(
                    interaction.list_pending(None).await,
                ));
            });
        }));

        Ok(subscriptions)
    }

    pub async fn list_messages(
        &self,
        conversation_id: &str,
        agent_id: Option<&str>,
    ) -> Result<DesktopMessagePage, String> {
        let messages = self
            .app
            .get(MESSAGE_LEGACY_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        match messages.list_all(conversation_id, agent_id).await {
            Ok(items) => Ok(DesktopMessagePage {
                items,
                has_more: false,
            }),
            Err(error)
                if error
                    .downcast_ref::<Error2>()
                    .is_some_and(|error| error.code == "session.not_found") =>
            {
                Ok(DesktopMessagePage {
                    items: Vec::new(),
                    has_more: false,
                })
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub async fn context_usage(
        &self,
        conversation_id: &str,
    ) -> Result<Option<DesktopContextUsage>, String> {
        self.ensure_models_configured().await?;
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session = if let Some(session) = sessions.get(conversation_id) {
            session
        } else {
            let Some(session) = sessions
                .resume(conversation_id)
                .await
                .map_err(|error| error.to_string())?
            else {
                return Ok(None);
            };
            session
        };
        let agents = session
            .get(AGENT_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let agent = agents
            .get(MAIN_AGENT_ID)
            .ok_or_else(|| "the session did not create its main agent".to_owned())?;
        let context_size = agent
            .get(AGENT_CONTEXT_SIZE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let profile = agent
            .get(AGENT_PROFILE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        context_usage_snapshot(&context_size, &profile).map(Some)
    }

    pub async fn respond_interaction(
        &self,
        conversation_id: &str,
        interaction_id: &str,
        response: Value,
    ) -> Result<(), String> {
        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session = sessions
            .get(conversation_id)
            .ok_or_else(|| "the conversation session is not active".to_owned())?;
        let interaction = session
            .get(SESSION_INTERACTION_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let is_pending = interaction
            .list_pending(None)
            .await
            .iter()
            .any(|pending| pending.id == interaction_id);
        if !is_pending {
            return Err("the interaction is no longer pending".to_owned());
        }
        interaction.respond(interaction_id, response).await;
        Ok(())
    }

    async fn fetch_models(&self) -> Result<Vec<ManagedKimiCodeModelInfo>, String> {
        let token = self.fresh_token().await?;
        let headers = self.identity_headers()?;
        fetch_managed_kimi_code_models(
            &token,
            Some(DEFAULT_KIMI_CODE_BASE_URL),
            Some(&headers),
            CredentialKind::OAuth,
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn ensure_models_configured(&self) -> Result<(), String> {
        if self.models_configured.load(Ordering::Acquire) {
            return Ok(());
        }
        let configured = self
            .app
            .get(MODEL_CATALOG_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .list_models()
            .await;
        if !configured.is_empty() {
            self.models_configured.store(true, Ordering::Release);
            return Ok(());
        }
        let models = self.fetch_models().await?;
        self.configure_models(&models).await
    }

    async fn configure_models(&self, models: &[ManagedKimiCodeModelInfo]) -> Result<(), String> {
        let _guard = self.config_gate.lock().await;
        let config = self
            .app
            .get(CONFIG_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        config.ready().await.map_err(|error| error.to_string())?;

        let managed_sections = [
            PROVIDERS_SECTION,
            MODELS_SECTION,
            SERVICES_SECTION,
            DEFAULT_MODEL_SECTION,
            THINKING_SECTION,
        ];
        let mut generated = Map::new();
        for section in managed_sections {
            if let Some(value) = config.inspect(section).user_value {
                generated.insert(section.to_owned(), value);
            }
        }
        apply_managed_kimi_code_config(
            &mut generated,
            ManagedKimiCodeApplyOptions {
                models,
                base_url: None,
                oauth_key: None,
                oauth_host: None,
                preserve_default_model: true,
            },
        )
        .map_err(|error| error.to_string())?;
        generated
            .entry(THINKING_SECTION.to_owned())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| "The thinking configuration must be an object.".to_owned())?
            .insert("enabled".to_owned(), Value::Bool(true));
        for section in [PROVIDERS_SECTION, MODELS_SECTION, SERVICES_SECTION] {
            let next = generated.get(section).cloned();
            if config.inspect(section).user_value != next {
                config
                    .replace(section, next, ConfigTarget::User)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            config
                .replace(section, None, ConfigTarget::Memory)
                .await
                .map_err(|error| error.to_string())?;
        }
        for section in [DEFAULT_MODEL_SECTION, THINKING_SECTION] {
            let next = generated.get(section).cloned();
            if config.inspect(section).user_value != next {
                config
                    .set(section, next, ConfigTarget::User)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            config
                .replace(section, None, ConfigTarget::Memory)
                .await
                .map_err(|error| error.to_string())?;
        }
        self.models_configured.store(true, Ordering::Release);
        Ok(())
    }

    async fn fresh_token(&self) -> Result<String, String> {
        let provider = self
            .oauth
            .token_provider(Some(KIMI_CODE_PROVIDER_NAME), None)
            .map_err(|error| error.to_string())?;
        provider
            .get_access_token(false)
            .await
            .map_err(|error| error.to_string())
    }

    fn identity_headers(&self) -> Result<IndexMap<String, String>, String> {
        create_kimi_default_headers(&KimiIdentityOptions {
            home_dir: self.home_dir.clone(),
            host: KimiHostIdentity {
                user_agent_product: "kimi-code-desktop".to_owned(),
                version: self.client_version.clone(),
                user_agent_suffix: Some("Tauri".to_owned()),
            },
        })
        .map_err(|error| error.to_string())
    }
}

fn user_section<T: DeserializeOwned + Default>(
    config: &ConfigServiceHandle,
    domain: &str,
) -> Result<T, String> {
    section_from_value(config.inspect(domain).user_value.as_ref())
}

fn section_from_value<T: DeserializeOwned + Default>(value: Option<&Value>) -> Result<T, String> {
    value.map_or_else(
        || Ok(T::default()),
        |value| serde_json::from_value(value.clone()).map_err(|error| error.to_string()),
    )
}

fn is_managed_provider_id(id: &str) -> bool {
    id == KIMI_CODE_PROVIDER_NAME
}

fn protocol_for_provider_type(provider_type: &str) -> Result<Protocol, String> {
    match provider_type {
        "kimi" | "openai" => Ok(Protocol::OpenAi),
        "openai_responses" => Ok(Protocol::OpenAiResponses),
        "anthropic" => Ok(Protocol::Anthropic),
        "google-genai" => Ok(Protocol::GoogleGenAi),
        _ => Err(format!("Unsupported provider protocol `{provider_type}`.")),
    }
}

fn provider_model_config_id(provider_id: &str, model: &str) -> String {
    format!("{provider_id}/{model}")
}

fn validate_provider_input(
    mut input: DesktopSaveProviderInput,
) -> Result<DesktopSaveProviderInput, String> {
    input.id = input.id.trim().to_owned();
    input.original_id = input
        .original_id
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty());
    input.provider_type = input.provider_type.trim().to_owned();
    input.base_url = input.base_url.trim().trim_end_matches('/').to_owned();
    input.api_key = input
        .api_key
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty());
    input.default_model = input
        .default_model
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty());

    let mut id_chars = input.id.chars();
    if input.id.is_empty()
        || input.id.len() > 64
        || !id_chars.next().is_some_and(char::is_alphanumeric)
        || !id_chars
            .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_' | ' '))
        || input.id == ENV_MODEL_PROVIDER_KEY
    {
        return Err(
            "Provider name must start with a letter or number and contain only letters, numbers, spaces, '-' or '_'."
                .to_owned(),
        );
    }
    protocol_for_provider_type(&input.provider_type)?;
    let url = url::Url::parse(&input.base_url)
        .map_err(|_| "Base URL must be a valid HTTP or HTTPS URL.".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Base URL must be a valid HTTP or HTTPS URL.".to_owned());
    }
    if input.original_id.is_none() && input.api_key.is_none() {
        return Err("API Key is required when adding a provider.".to_owned());
    }
    if input.models.is_empty() {
        return Err("At least one model is required.".to_owned());
    }
    if input.models.len() > 64 {
        return Err("A provider can configure at most 64 models.".to_owned());
    }

    let mut model_names = HashSet::new();
    for model in &mut input.models {
        model.model = model.model.trim().to_owned();
        model.display_name = model
            .display_name
            .take()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        deduplicate_trimmed(&mut model.capabilities);
        deduplicate_trimmed(&mut model.support_efforts);
        model.default_effort = model
            .default_effort
            .take()
            .map(|effort| effort.trim().to_owned())
            .filter(|effort| !effort.is_empty());
        if model.model.is_empty() {
            return Err("Model ID cannot be empty.".to_owned());
        }
        if model.model.len() > 128 {
            return Err("Model ID cannot exceed 128 characters.".to_owned());
        }
        if model.max_context_size == 0 {
            return Err("Model context size must be greater than zero.".to_owned());
        }
        if model
            .default_effort
            .as_ref()
            .is_some_and(|default| !model.support_efforts.iter().any(|effort| effort == default))
        {
            return Err(format!(
                "Default effort for model `{}` must be one of its supported efforts.",
                model.model
            ));
        }
        if !model_names.insert(model.model.clone()) {
            return Err(format!(
                "Model `{}` is configured more than once.",
                model.model
            ));
        }
    }
    if input
        .default_model
        .as_ref()
        .is_some_and(|default| !model_names.contains(default))
    {
        return Err("Default model must be one of the provider models.".to_owned());
    }
    if input.default_model.is_none() {
        input.default_model = input.models.first().map(|model| model.model.clone());
    }
    Ok(input)
}

fn deduplicate_trimmed(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain_mut(|value| {
        *value = value.trim().to_owned();
        !value.is_empty() && seen.insert(value.clone())
    });
}

fn desktop_providers(providers: &ProvidersSection, models: &ModelsSection) -> Vec<DesktopProvider> {
    providers
        .iter()
        .filter(|(id, _)| id.as_str() != ENV_MODEL_PROVIDER_KEY)
        .map(|(id, provider)| {
            let provider_models = models
                .iter()
                .filter(|(_, model)| model.provider.as_deref() == Some(id.as_str()))
                .map(|(config_id, model)| DesktopProviderModel {
                    model: model.model.clone().unwrap_or_else(|| {
                        config_id
                            .strip_prefix(&format!("{id}/"))
                            .unwrap_or(config_id)
                            .to_owned()
                    }),
                    display_name: model.display_name.clone(),
                    max_context_size: model.max_context_size.map_or(0, NonZeroU64::get),
                    capabilities: model.capabilities.clone().unwrap_or_default(),
                    support_efforts: model.support_efforts.clone().unwrap_or_default(),
                    default_effort: model.default_effort.clone(),
                    adaptive_thinking: model.adaptive_thinking,
                })
                .collect::<Vec<_>>();
            let default_model = provider.default_model.as_ref().and_then(|default_id| {
                models
                    .get(default_id)
                    .and_then(|model| model.model.clone())
                    .or_else(|| {
                        default_id
                            .strip_prefix(&format!("{id}/"))
                            .map(str::to_owned)
                    })
            });
            DesktopProvider {
                id: id.clone(),
                provider_type: provider
                    .provider_type
                    .as_ref()
                    .map_or_else(|| "openai".to_owned(), ToString::to_string),
                base_url: provider.base_url.clone(),
                default_model,
                has_api_key: provider
                    .api_key
                    .as_deref()
                    .is_some_and(|key| !key.trim().is_empty()),
                managed: provider.oauth.is_some() || is_managed_provider_id(id),
                models: provider_models,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn replace_provider_sections(
    config: &ConfigServiceHandle,
    old_providers: Option<Value>,
    old_models: Option<Value>,
    next_providers: Option<Value>,
    next_models: Option<Value>,
    next_default: Option<Value>,
) -> Result<(), String> {
    config
        .replace(PROVIDERS_SECTION, next_providers, ConfigTarget::User)
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = config
        .replace(MODELS_SECTION, next_models, ConfigTarget::User)
        .await
    {
        let rollback = config
            .replace(PROVIDERS_SECTION, old_providers, ConfigTarget::User)
            .await;
        return Err(rollback_message(error.to_string(), rollback.err()));
    }
    if let Err(error) = config
        .replace(DEFAULT_MODEL_SECTION, next_default, ConfigTarget::User)
        .await
    {
        let models_rollback = config
            .replace(MODELS_SECTION, old_models, ConfigTarget::User)
            .await;
        let providers_rollback = config
            .replace(PROVIDERS_SECTION, old_providers, ConfigTarget::User)
            .await;
        let rollback = models_rollback.err().or_else(|| providers_rollback.err());
        return Err(rollback_message(error.to_string(), rollback));
    }
    Ok(())
}

fn rollback_message(
    error: String,
    rollback: Option<crate::app::config::ConfigServiceError>,
) -> String {
    rollback.map_or(error.clone(), |rollback| {
        format!("{error}; provider configuration rollback also failed: {rollback}")
    })
}

fn map_desktop_model(
    item: ModelCatalogItem,
    resolved: Option<&Model>,
    is_default: bool,
) -> DesktopModel {
    let declared_capabilities = item.capabilities.as_deref().unwrap_or_default();
    let has_declared_capability = |capability: &str| {
        declared_capabilities
            .iter()
            .any(|value| value == capability)
    };
    let supports_reasoning = resolved.map_or_else(
        || has_declared_capability("thinking") || has_declared_capability("always_thinking"),
        |model| model.capabilities.thinking,
    );
    let supports_image = resolved.map_or_else(
        || has_declared_capability("image_in"),
        |model| model.capabilities.image_in,
    );
    let supports_video = resolved.map_or_else(
        || has_declared_capability("video_in"),
        |model| model.capabilities.video_in,
    );
    let supports_tools = resolved.map_or_else(
        || has_declared_capability("tool_use"),
        |model| model.capabilities.tool_use,
    );
    let wire_model = resolved.map_or_else(
        || {
            item.model
                .rsplit_once('/')
                .map_or_else(|| item.model.clone(), |(_, model)| model.to_owned())
        },
        |model| model.name.clone(),
    );
    let protocol = resolved.map_or("openai", |model| model.protocol.as_str());

    DesktopModel {
        display_name: item
            .display_name
            .clone()
            .unwrap_or_else(|| wire_model.clone()),
        id: item.model,
        model: wire_model,
        provider_id: item.provider,
        is_default,
        context_length: item.max_context_size,
        supports_reasoning,
        supports_image,
        supports_video,
        supports_tools,
        protocol: protocol.to_owned(),
        support_efforts: item.support_efforts.unwrap_or_default(),
        default_effort: item.default_effort,
    }
}

fn map_desktop_workspace(workspace: Workspace) -> DesktopWorkspace {
    DesktopWorkspace {
        id: workspace.id,
        root: workspace.root,
        name: workspace.name,
        created_at: workspace.created_at_millis,
        last_opened_at: workspace.last_opened_at_millis,
    }
}

fn map_desktop_cron_task(cron: &SessionCronServiceHandle, task: CronTask) -> DesktopCronTask {
    let stale = cron.is_stale(&task);
    let (human_schedule, next_fire_at) = match parse_cron_expression(&task.cron) {
        Ok(parsed) => (
            cron_to_human(&parsed),
            cron.get_next_fire_for_task(&task.id)
                .map(format_local_iso_with_offset),
        ),
        Err(_) => (task.cron.clone(), None),
    };
    DesktopCronTask {
        id: task.id,
        cron: task.cron,
        prompt: task.prompt,
        created_at: task.created_at,
        recurring: task.recurring != Some(false),
        last_fired_at: task.last_fired_at,
        human_schedule,
        next_fire_at,
        stale,
    }
}

fn map_desktop_custom_agent(
    scope: DesktopCustomAgentScope,
    file: ManagedAgentFile,
) -> DesktopCustomAgent {
    let fallback_name = Path::new(&file.relative_path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(&file.relative_path)
        .to_owned();
    let definition = file.definition;
    DesktopCustomAgent {
        scope,
        relative_path: file.relative_path,
        path: file.path,
        content: file.content,
        name: definition
            .as_ref()
            .map(|definition| definition.name.clone())
            .unwrap_or(fallback_name),
        description: definition
            .as_ref()
            .map(|definition| definition.description.clone()),
        when_to_use: definition
            .as_ref()
            .and_then(|definition| definition.when_to_use.clone()),
        is_override: definition
            .as_ref()
            .is_some_and(|definition| definition.is_override),
        tools: definition
            .as_ref()
            .and_then(|definition| definition.tools.clone()),
        disallowed_tools: definition
            .as_ref()
            .and_then(|definition| definition.disallowed_tools.clone()),
        subagents: definition
            .as_ref()
            .and_then(|definition| definition.subagents.clone()),
        model: definition
            .as_ref()
            .and_then(|definition| definition.model.clone()),
        valid: definition.is_some(),
        error: file.error,
    }
}

fn custom_agent_source(scope: DesktopCustomAgentScope) -> AgentFileSource {
    match scope {
        DesktopCustomAgentScope::App => AgentFileSource::User,
        DesktopCustomAgentScope::Project => AgentFileSource::Project,
    }
}

fn custom_agent_scope_order(scope: DesktopCustomAgentScope) -> u8 {
    match scope {
        DesktopCustomAgentScope::App => 0,
        DesktopCustomAgentScope::Project => 1,
    }
}

fn context_usage_snapshot(
    context_size: &AgentContextSizeServiceHandle,
    profile: &AgentProfileServiceHandle,
) -> Result<DesktopContextUsage, String> {
    let context = context_size.get(None, None);
    let max_context_tokens = profile
        .get_model_capabilities()
        .map_err(|error| error.to_string())?
        .max_context_tokens;
    Ok(context_usage_from_size(context, max_context_tokens))
}

fn context_usage_from_size(context: ContextSize, max_context_tokens: u64) -> DesktopContextUsage {
    let usage_ratio = if max_context_tokens == 0 {
        0.0
    } else {
        context.size / max_context_tokens as f64
    };
    DesktopContextUsage {
        context_tokens: context.size,
        measured_tokens: context.measured,
        estimated_tokens: context.estimated,
        max_context_tokens,
        usage_ratio,
    }
}

fn attach_desktop_agent_events(
    subscriptions: &DisposableStore,
    attached: &Mutex<HashSet<String>>,
    agent: &ScopeHandle,
    on_event: &Arc<dyn Fn(String, Value) + Send + Sync>,
    replay: bool,
) -> Result<(), String> {
    let agent_id = agent.id().to_owned();
    {
        let mut attached = attached.lock().unwrap();
        if !attached.insert(agent_id.clone()) {
            return Ok(());
        }
    }
    let event_bus = match agent.get(EVENT_BUS_SERVICE_ID) {
        Ok(event_bus) => event_bus,
        Err(error) => {
            attached.lock().unwrap().remove(&agent_id);
            return Err(error.to_string());
        }
    };
    let on_event = Arc::clone(on_event);
    let handler = Arc::new(move |event: &crate::app::event::event_bus::DomainEvent| {
        on_event(agent_id.clone(), event.clone().into_value());
    });
    subscriptions.add(if replay {
        event_bus.subscribe_with_replay(handler)
    } else {
        event_bus.subscribe(handler)
    });
    Ok(())
}

fn agent_settings_from_config(config: &dyn ConfigServiceContract) -> DesktopAgentSettings {
    let default_model = config
        .get(DEFAULT_MODEL_SECTION)
        .and_then(|value| value.as_str().map(str::to_owned));
    let default_permission = config
        .get(DEFAULT_PERMISSION_MODE_SECTION)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or(PermissionMode::Manual);
    let default_thinking = config
        .get(THINKING_SECTION)
        .and_then(|value| value.get("enabled").and_then(Value::as_bool))
        .unwrap_or(true);
    let default_plan_mode = config
        .get(DEFAULT_PLAN_MODE_SECTION)
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    DesktopAgentSettings {
        default_model,
        default_permission,
        default_thinking,
        default_plan_mode,
    }
}

fn map_desktop_interactions(interactions: Vec<Interaction>) -> Vec<DesktopInteraction> {
    interactions
        .into_iter()
        .map(|interaction| DesktopInteraction {
            id: interaction.id,
            kind: match interaction.kind {
                InteractionKind::Approval => "approval",
                InteractionKind::Question => "question",
                InteractionKind::UserTool => "user_tool",
            }
            .to_owned(),
            payload: interaction.payload,
            created_at: interaction.created_at,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use indexmap::IndexMap;
    use serde_json::json;

    use super::{
        DesktopAgentSettingsPatch, DesktopChatEvent, DesktopChatRequest,
        DesktopCreateCronTaskInput, DesktopDeleteCronTaskInput, DesktopPrepareSessionRequest,
        DesktopProviderModelInput, DesktopSaveProviderInput, KimiCodeDesktopClient,
        ManagedKimiCodeModelInfo, context_usage_from_size, map_desktop_model,
    };
    use crate::{
        agent::{
            context_size::ContextSize, loop_::AssistantDeltaEvent,
            permission_policy::PermissionMode,
        },
        app::{
            bootstrap::BOOTSTRAP_SERVICE_ID,
            cron::{CRON_SESSION_TAG, CRON_TASK_PERSISTENCE_SERVICE_ID, CronTask, CronTaskQuery},
            event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID},
            session_index::SESSION_INDEX_SERVICE_ID,
            session_lifecycle::SESSION_LIFECYCLE_SERVICE_ID,
        },
        kosong::model::ModelCatalogItem,
        session::agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, CreateAgentOptions, MAIN_AGENT_ID},
    };

    #[test]
    fn desktop_catalog_models_expose_the_configured_alias_as_id() {
        let model = map_desktop_model(
            ModelCatalogItem {
                provider: "managed:kimi-code".into(),
                model: "kimi-code/kimi-for-coding".into(),
                display_name: Some("Kimi for Coding".into()),
                max_context_size: 262_144,
                capabilities: Some(vec!["thinking".into(), "tool_use".into()]),
                support_efforts: Some(vec!["low".into(), "high".into()]),
                default_effort: Some("high".into()),
            },
            None,
            true,
        );

        assert_eq!(model.id, "kimi-code/kimi-for-coding");
        assert_eq!(model.model, "kimi-for-coding");
        assert_eq!(model.provider_id, "managed:kimi-code");
        assert!(model.is_default);
        assert!(model.supports_reasoning);
        assert!(model.supports_tools);
    }

    #[tokio::test]
    async fn agent_settings_defaults_and_updates_are_persisted() {
        let root = std::env::temp_dir().join(format!(
            "kimi-desktop-agent-settings-{}",
            uuid::Uuid::new_v4()
        ));
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();

        let defaults = client.get_agent_settings().await.unwrap();
        assert_eq!(defaults.default_model, None);
        assert_eq!(defaults.default_permission, PermissionMode::Manual);
        assert!(defaults.default_thinking);
        assert!(!defaults.default_plan_mode);

        let updated = client
            .update_agent_settings(DesktopAgentSettingsPatch {
                default_permission: Some(PermissionMode::Yolo),
                default_thinking: Some(false),
                default_plan_mode: Some(true),
                ..DesktopAgentSettingsPatch::default()
            })
            .await
            .unwrap();
        assert_eq!(updated.default_permission, PermissionMode::Yolo);
        assert!(!updated.default_thinking);
        assert!(updated.default_plan_mode);

        let persisted = std::fs::read_to_string(home.join("config.toml")).unwrap();
        let persisted = toml::from_str::<toml::Value>(&persisted).unwrap();
        assert_eq!(
            persisted
                .get("default_permission_mode")
                .and_then(toml::Value::as_str),
            Some("yolo")
        );
        assert_eq!(
            persisted
                .get("default_plan_mode")
                .and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            persisted
                .get("thinking")
                .and_then(toml::Value::as_table)
                .and_then(|thinking| thinking.get("enabled"))
                .and_then(toml::Value::as_bool),
            Some(false)
        );

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_configuration_keeps_secrets_out_of_responses_and_removes_models_together() {
        let root =
            std::env::temp_dir().join(format!("kimi-desktop-providers-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();
        let model = DesktopProviderModelInput {
            model: "example-model".into(),
            display_name: Some("Example Model".into()),
            max_context_size: 131_072,
            capabilities: vec!["tool_use".into(), "thinking".into()],
            support_efforts: vec!["low".into(), "high".into()],
            default_effort: Some("high".into()),
            adaptive_thinking: Some(true),
        };
        let saved = client
            .save_provider(DesktopSaveProviderInput {
                original_id: None,
                id: "example-provider".into(),
                provider_type: "openai".into(),
                api_key: Some("YOUR_API_KEY".into()),
                replace_api_key: true,
                base_url: "https://api.example.test/v1/".into(),
                default_model: Some("example-model".into()),
                models: vec![model.clone()],
            })
            .await
            .unwrap();
        assert!(saved.has_api_key);
        assert_eq!(saved.models[0].default_effort.as_deref(), Some("high"));
        assert_eq!(
            saved.base_url.as_deref(),
            Some("https://api.example.test/v1")
        );
        assert!(
            serde_json::to_value(&saved)
                .unwrap()
                .get("apiKey")
                .is_none()
        );

        let mut invalid_model = model.clone();
        invalid_model.default_effort = Some("max".into());
        let invalid_error = client
            .save_provider(DesktopSaveProviderInput {
                original_id: Some("example-provider".into()),
                id: "example-provider".into(),
                provider_type: "openai".into(),
                api_key: None,
                replace_api_key: false,
                base_url: "https://api.example.test/v1".into(),
                default_model: Some("example-model".into()),
                models: vec![invalid_model],
            })
            .await
            .unwrap_err();
        assert!(invalid_error.contains("must be one of its supported efforts"));

        let models = client.list_models().await.unwrap();
        assert!(models.iter().any(|model| {
            model.id == "example-provider/example-model"
                && model.model == "example-model"
                && model.protocol == "openai"
        }));

        client
            .save_provider(DesktopSaveProviderInput {
                original_id: Some("example-provider".into()),
                id: "example-provider".into(),
                provider_type: "openai_responses".into(),
                api_key: None,
                replace_api_key: false,
                base_url: "https://responses.example.test/v1".into(),
                default_model: Some("example-model".into()),
                models: vec![model],
            })
            .await
            .unwrap();
        let persisted = std::fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(persisted.contains("YOUR_API_KEY"));
        assert!(persisted.contains("openai_responses"));
        assert!(persisted.contains("default_effort = \"high\""));

        client
            .delete_provider("example-provider".into())
            .await
            .unwrap();
        assert!(client.list_providers().await.unwrap().is_empty());
        assert!(
            !client
                .list_models()
                .await
                .unwrap()
                .iter()
                .any(|model| model.id == "example-provider/example-model")
        );

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    fn managed_model(id: &str) -> ManagedKimiCodeModelInfo {
        ManagedKimiCodeModelInfo {
            id: id.into(),
            context_length: 262_144,
            supports_reasoning: true,
            supports_image_in: false,
            supports_video_in: false,
            supports_tool_use: true,
            supports_thinking_type: None,
            support_efforts: Some(vec!["low".into(), "high".into()]),
            default_effort: Some("high".into()),
            display_name: Some(id.into()),
            protocol: None,
        }
    }

    #[tokio::test]
    async fn desktop_cron_api_creates_lists_and_deletes_session_tasks() {
        let root =
            std::env::temp_dir().join(format!("kimi-desktop-cron-tasks-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let work_dir = root.join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();
        client
            .configure_models(&[managed_model("first")])
            .await
            .unwrap();
        let prepared = client
            .prepare_session(DesktopPrepareSessionRequest {
                session_id: None,
                work_dir: work_dir.to_string_lossy().into_owned(),
                model: Some("kimi-code/first".into()),
                thinking: Some("high".into()),
                permission: Some(PermissionMode::Yolo),
            })
            .await
            .unwrap();

        let created = client
            .create_cron_task(DesktopCreateCronTaskInput {
                session_id: prepared.session_id.clone(),
                cron: "  */15   * * * *  ".into(),
                prompt: "Check the build".into(),
                recurring: true,
            })
            .await
            .unwrap();
        assert_eq!(created.cron, "*/15 * * * *");
        assert_eq!(created.human_schedule, "every 15 minutes");
        assert!(created.next_fire_at.is_some());

        let tasks = client.list_cron_tasks(&prepared.session_id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, created.id);
        assert_eq!(tasks[0].prompt, "Check the build");

        client
            .delete_cron_task(DesktopDeleteCronTaskInput {
                session_id: prepared.session_id.clone(),
                id: created.id,
            })
            .await
            .unwrap();
        assert!(
            client
                .list_cron_tasks(&prepared.session_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            client
                .create_cron_task(DesktopCreateCronTaskInput {
                    session_id: prepared.session_id,
                    cron: "not cron".into(),
                    prompt: "x".into(),
                    recurring: true,
                })
                .await
                .is_err()
        );

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_settings_are_restored_instead_of_overridden_by_new_defaults() {
        let root = std::env::temp_dir().join(format!(
            "kimi-desktop-model-selection-{}",
            uuid::Uuid::new_v4()
        ));
        let home = root.join("home");
        let work_dir = root.join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        std::fs::write(home.join("config.toml"), "[thinking]\nenabled = false\n").unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();
        let models = [managed_model("first"), managed_model("second")];
        client.configure_models(&models).await.unwrap();
        client
            .models_configured
            .store(false, std::sync::atomic::Ordering::Release);
        client.ensure_models_configured().await.unwrap();

        let created = client
            .prepare_session(DesktopPrepareSessionRequest {
                session_id: None,
                work_dir: work_dir.to_string_lossy().into_owned(),
                model: Some("kimi-code/first".into()),
                thinking: Some("high".into()),
                permission: Some(PermissionMode::Yolo),
            })
            .await
            .unwrap();
        assert_eq!(created.model, "kimi-code/first");
        assert_eq!(created.thinking_level, "high");
        assert_eq!(created.permission_mode, PermissionMode::Yolo);
        let created_json = serde_json::to_value(&created).unwrap();
        assert_eq!(created_json["thinkingLevel"], "high");
        assert_eq!(created_json["permissionMode"], "yolo");

        let resumed = client
            .prepare_session(DesktopPrepareSessionRequest {
                session_id: Some(created.session_id),
                work_dir: work_dir.to_string_lossy().into_owned(),
                model: Some("kimi-code/second".into()),
                thinking: Some("low".into()),
                permission: Some(PermissionMode::Auto),
            })
            .await
            .unwrap();
        assert_eq!(resumed.model, "kimi-code/first");
        assert_eq!(resumed.thinking_level, "high");
        assert_eq!(resumed.permission_mode, PermissionMode::Yolo);

        client.set_default_model("kimi-code/second").await.unwrap();
        client.configure_models(&models).await.unwrap();
        let persisted = std::fs::read_to_string(home.join("config.toml")).unwrap();
        let persisted = toml::from_str::<toml::Value>(&persisted).unwrap();
        assert_eq!(
            persisted.get("default_model").and_then(toml::Value::as_str),
            Some("kimi-code/second")
        );
        assert_eq!(
            persisted
                .get("thinking")
                .and_then(toml::Value::as_table)
                .and_then(|thinking| thinking.get("enabled"))
                .and_then(toml::Value::as_bool),
            Some(true)
        );
        assert!(
            persisted
                .get("models")
                .and_then(toml::Value::as_table)
                .is_some_and(|models| models.contains_key("kimi-code/second"))
        );

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn archived_sessions_can_be_listed_and_restored() {
        let root = std::env::temp_dir().join(format!(
            "kimi-desktop-archived-sessions-{}",
            uuid::Uuid::new_v4()
        ));
        let home = root.join("home");
        let work_dir = root.join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();
        client
            .configure_models(&[managed_model("first")])
            .await
            .unwrap();

        let prepared = client
            .prepare_session(DesktopPrepareSessionRequest {
                session_id: None,
                work_dir: work_dir.to_string_lossy().into_owned(),
                model: Some("kimi-code/first".into()),
                thinking: Some("high".into()),
                permission: Some(PermissionMode::Yolo),
            })
            .await
            .unwrap();
        client.archive_session(&prepared.session_id).await.unwrap();

        let archived = client.list_archived_sessions().await.unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, prepared.session_id);
        assert!(archived[0].archived);

        let restored = client.restore_session(&prepared.session_id).await.unwrap();
        assert_eq!(restored.id, prepared.session_id);
        assert!(!restored.archived);
        assert!(client.list_archived_sessions().await.unwrap().is_empty());

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn archived_sessions_can_be_permanently_deleted_in_batches() {
        let root = std::env::temp_dir().join(format!(
            "kimi-desktop-delete-archived-sessions-{}",
            uuid::Uuid::new_v4()
        ));
        let home = root.join("home");
        let first_work_dir = root.join("workspace-one");
        let second_work_dir = root.join("workspace-two");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&first_work_dir).unwrap();
        std::fs::create_dir_all(&second_work_dir).unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();
        client
            .configure_models(&[managed_model("first")])
            .await
            .unwrap();

        let prepare = |work_dir: &std::path::Path| DesktopPrepareSessionRequest {
            session_id: None,
            work_dir: work_dir.to_string_lossy().into_owned(),
            model: Some("kimi-code/first".into()),
            thinking: Some("high".into()),
            permission: Some(PermissionMode::Yolo),
        };
        let first = client
            .prepare_session(prepare(&first_work_dir))
            .await
            .unwrap();
        let second = client
            .prepare_session(prepare(&first_work_dir))
            .await
            .unwrap();
        let active = client
            .prepare_session(prepare(&first_work_dir))
            .await
            .unwrap();
        let other = client
            .prepare_session(prepare(&second_work_dir))
            .await
            .unwrap();
        client.archive_session(&first.session_id).await.unwrap();
        client.archive_session(&second.session_id).await.unwrap();
        client.archive_session(&other.session_id).await.unwrap();

        let index = client.app.get(SESSION_INDEX_SERVICE_ID).unwrap();
        let first_summary = index.get(&first.session_id).await.unwrap().unwrap();
        let first_session_dir = client
            .app
            .get(BOOTSTRAP_SERVICE_ID)
            .unwrap()
            .session_dir(&first_summary.workspace_id, &first.session_id);
        assert!(first_session_dir.is_dir());

        let cron = client.app.get(CRON_TASK_PERSISTENCE_SERVICE_ID).unwrap();
        let tagged_task = CronTask {
            id: "deadbeef".into(),
            cron: "0 9 * * *".into(),
            prompt: "delete me".into(),
            created_at: 1.0,
            recurring: Some(true),
            last_fired_at: None,
            tags: Some(IndexMap::from_iter([(
                CRON_SESSION_TAG.into(),
                first.session_id.clone(),
            )])),
        };
        let unrelated_task = CronTask {
            id: "cafebabe".into(),
            tags: None,
            ..tagged_task.clone()
        };
        cron.save(&first_summary.workspace_id, &tagged_task)
            .await
            .unwrap();
        cron.save(&first_summary.workspace_id, &unrelated_task)
            .await
            .unwrap();

        let preflight_error = client
            .delete_archived_sessions(&[first.session_id.clone(), active.session_id.clone()])
            .await
            .unwrap_err();
        assert!(preflight_error.contains("must be archived"));
        assert!(index.get(&first.session_id).await.unwrap().is_some());
        let lifecycle = client.app.get(SESSION_LIFECYCLE_SERVICE_ID).unwrap();
        let active_error = lifecycle
            .delete_archived(&active.session_id)
            .await
            .unwrap_err();
        assert!(
            active_error
                .to_string()
                .contains("active or changing state")
        );

        let deleted = client
            .delete_archived_sessions(&[
                first.session_id.clone(),
                " ".into(),
                second.session_id.clone(),
                first.session_id.clone(),
            ])
            .await
            .unwrap();
        assert_eq!(
            deleted,
            vec![first.session_id.clone(), second.session_id.clone()]
        );
        assert!(!first_session_dir.exists());
        assert!(index.get(&first.session_id).await.unwrap().is_none());
        assert!(index.get(&second.session_id).await.unwrap().is_none());
        assert!(index.get(&active.session_id).await.unwrap().is_some());
        assert!(index.get(&other.session_id).await.unwrap().is_some());
        assert_eq!(
            client
                .delete_archived_sessions(&[first.session_id.clone(), second.session_id.clone()])
                .await
                .unwrap(),
            Vec::<String>::new()
        );

        let remaining_tasks = cron
            .list(CronTaskQuery {
                workspace_id: first_summary.workspace_id,
            })
            .await
            .unwrap();
        assert_eq!(
            remaining_tasks
                .into_iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            vec![unrelated_task.id]
        );
        assert_eq!(client.list_archived_sessions().await.unwrap().len(), 1);

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn main_subscription_multiplexes_new_subagent_events() {
        let root = std::env::temp_dir().join(format!(
            "kimi-desktop-subagent-events-{}",
            uuid::Uuid::new_v4()
        ));
        let home = root.join("home");
        let work_dir = root.join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();
        client
            .configure_models(&[managed_model("first")])
            .await
            .unwrap();
        let prepared = client
            .prepare_session(DesktopPrepareSessionRequest {
                session_id: None,
                work_dir: work_dir.to_string_lossy().into_owned(),
                model: Some("kimi-code/first".into()),
                thinking: Some("high".into()),
                permission: Some(PermissionMode::Yolo),
            })
            .await
            .unwrap();
        let received = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
        let received_for_events = Arc::clone(&received);
        let subscription = client
            .subscribe_agent_events(
                &prepared.session_id,
                MAIN_AGENT_ID,
                Arc::new(move |agent_id, event| {
                    received_for_events.lock().unwrap().push((agent_id, event));
                }),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();

        let sessions = client.app.get(SESSION_LIFECYCLE_SERVICE_ID).unwrap();
        let session = sessions.get(&prepared.session_id).unwrap();
        let lifecycle = session.get(AGENT_LIFECYCLE_SERVICE_ID).unwrap();
        let child = lifecycle
            .create(CreateAgentOptions {
                agent_id: Some("child-live".into()),
                ..CreateAgentOptions::default()
            })
            .await
            .unwrap();
        child
            .get(EVENT_BUS_SERVICE_ID)
            .unwrap()
            .publish(DomainEvent::new("test.child-live", serde_json::Map::new()));

        assert!(received.lock().unwrap().iter().any(|(agent_id, event)| {
            agent_id == "child-live" && event["type"] == "test.child-live"
        }));

        subscription.dispose().unwrap();
        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn replay_subscription_backfills_main_and_existing_subagent_then_stays_live() {
        let root = std::env::temp_dir().join(format!(
            "kimi-desktop-replay-events-{}",
            uuid::Uuid::new_v4()
        ));
        let home = root.join("home");
        let work_dir = root.join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();
        client
            .configure_models(&[managed_model("first")])
            .await
            .unwrap();
        let prepared = client
            .prepare_session(DesktopPrepareSessionRequest {
                session_id: None,
                work_dir: work_dir.to_string_lossy().into_owned(),
                model: Some("kimi-code/first".into()),
                thinking: Some("high".into()),
                permission: Some(PermissionMode::Yolo),
            })
            .await
            .unwrap();

        let sessions = client.app.get(SESSION_LIFECYCLE_SERVICE_ID).unwrap();
        let session = sessions.get(&prepared.session_id).unwrap();
        let lifecycle = session.get(AGENT_LIFECYCLE_SERVICE_ID).unwrap();
        let main = lifecycle.get(MAIN_AGENT_ID).unwrap();
        let child = lifecycle
            .create(CreateAgentOptions {
                agent_id: Some("child-replay".into()),
                ..CreateAgentOptions::default()
            })
            .await
            .unwrap();

        for (agent, turn_id, delta) in [
            (&main, 1_i64, "main-before"),
            (&child, 2_i64, "child-before"),
        ] {
            let bus = agent.get(EVENT_BUS_SERVICE_ID).unwrap();
            bus.publish(
                DomainEvent::try_from(json!({
                    "type": "turn.started",
                    "turnId": turn_id
                }))
                .unwrap(),
            );
            bus.publish(
                DomainEvent::try_from(json!({
                    "type": "assistant.delta",
                    "turnId": turn_id,
                    "delta": delta
                }))
                .unwrap(),
            );
        }

        let received = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
        let received_for_events = Arc::clone(&received);
        let subscription = client
            .subscribe_agent_events_with_replay(
                &prepared.session_id,
                MAIN_AGENT_ID,
                Arc::new(move |agent_id, event| {
                    received_for_events.lock().unwrap().push((agent_id, event));
                }),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();

        for (agent_id, expected_delta) in [
            (MAIN_AGENT_ID, "main-before"),
            ("child-replay", "child-before"),
        ] {
            let types = received
                .lock()
                .unwrap()
                .iter()
                .filter(|(id, _)| id == agent_id)
                .map(|(_, event)| event["type"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            assert_eq!(types, ["turn.started", "assistant.delta"]);
            assert!(
                received
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(id, event)| { id == agent_id && event["delta"] == expected_delta })
            );
        }

        main.get(EVENT_BUS_SERVICE_ID).unwrap().publish(
            DomainEvent::try_from(json!({
                "type": "assistant.delta",
                "turnId": 1,
                "delta": "main-live"
            }))
            .unwrap(),
        );
        assert_eq!(
            received
                .lock()
                .unwrap()
                .iter()
                .filter(|(agent_id, event)| {
                    agent_id == MAIN_AGENT_ID && event["delta"] == "main-live"
                })
                .count(),
            1
        );

        subscription.dispose().unwrap();
        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn desktop_chat_requests_accept_only_a_direct_prompt() {
        let request: DesktopChatRequest = serde_json::from_value(json!({
            "prompt": " explain this crate ",
            "model": "kimi-k2",
            "projectPath": "/repo"
        }))
        .unwrap();
        assert_eq!(request.prompt, " explain this crate ");

        assert!(
            serde_json::from_value::<DesktopChatRequest>(json!({
                "model": "kimi-k2",
                "projectPath": "/repo",
                "messages": [{"role": "user", "content": "legacy"}]
            }))
            .is_err()
        );
    }

    #[test]
    fn serializes_realtime_events_as_a_discriminated_event_union() {
        let event = DesktopChatEvent::AssistantDelta(AssistantDeltaEvent {
            turn_id: crate::agent::TurnId::new(7),
            delta: "hello".into(),
        });

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "type": "assistant_delta",
                "turnId": 7,
                "delta": "hello"
            })
        );
    }

    #[test]
    fn computes_desktop_context_usage_from_measured_and_estimated_tokens() {
        let usage = context_usage_from_size(
            ContextSize {
                size: 64_000.0,
                measured: 60_000.0,
                estimated: 4_000.0,
            },
            128_000,
        );

        assert_eq!(usage.context_tokens, 64_000.0);
        assert_eq!(usage.measured_tokens, 60_000.0);
        assert_eq!(usage.estimated_tokens, 4_000.0);
        assert_eq!(usage.max_context_tokens, 128_000);
        assert_eq!(usage.usage_ratio, 0.5);
    }

    #[tokio::test]
    async fn session_skills_include_sub_skills_for_listing_and_content() {
        let root =
            std::env::temp_dir().join(format!("kimi-desktop-sub-skills-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let work_dir = root.join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        let parent_dir = work_dir.join(".kimi-code/skills/parent");
        std::fs::create_dir_all(parent_dir.join("child")).unwrap();
        std::fs::write(
            parent_dir.join("SKILL.md"),
            "---\nname: parent\ndescription: Parent skill\nhas-sub-skill: true\n---\nParent body.\n",
        )
        .unwrap();
        std::fs::write(
            parent_dir.join("child").join("SKILL.md"),
            "---\nname: child\ndescription: Child skill\n---\nChild body.\n",
        )
        .unwrap();
        let client = KimiCodeDesktopClient::new(&home, "test").unwrap();
        client
            .configure_models(&[managed_model("first")])
            .await
            .unwrap();
        let prepared = client
            .prepare_session(DesktopPrepareSessionRequest {
                session_id: None,
                work_dir: work_dir.to_string_lossy().into_owned(),
                model: Some("kimi-code/first".into()),
                thinking: Some("high".into()),
                permission: Some(PermissionMode::Yolo),
            })
            .await
            .unwrap();

        let skills = client
            .list_session_skills(&prepared.session_id)
            .await
            .unwrap();
        let names = skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"parent"), "{names:?}");
        assert!(names.contains(&"parent.child"), "{names:?}");

        let content = client
            .get_session_skill_content(&prepared.session_id, "parent.child")
            .await
            .unwrap();
        assert_eq!(content.name, "parent.child");
        assert!(content.content.contains("Child body."));

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }
}
