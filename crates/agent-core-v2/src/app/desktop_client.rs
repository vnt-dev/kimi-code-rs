//! High-level Kimi Code client facade for desktop and other graphical hosts.
//!
//! The facade owns application composition, session lifecycle, managed model
//! configuration, streamed output, and host-mediated interactions.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use indexmap::IndexMap;
use kimi_code_oauth::{
    AuthManagedUsageResult, AuthenticatedServiceOptions, BoosterWalletInfo, CredentialKind,
    DeviceAuthorization, DeviceCodeObserver, KIMI_CODE_PROVIDER_NAME, KimiHostIdentity,
    KimiIdentityOptions, KimiOAuthLoginOptions, ManagedKimiCodeApplyOptions,
    ManagedKimiCodeModelInfo, OAuthManagerError, UsageRow, apply_managed_kimi_code_config,
    create_kimi_default_headers, fetch_managed_kimi_code_models,
    managed_usage::DEFAULT_KIMI_CODE_BASE_URL,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    _base::{
        di::{
            lifecycle::{DisposableHandle, DisposableStore},
            scope::Scope,
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
        permission_mode::AGENT_PERMISSION_MODE_SERVICE_ID,
        permission_policy::PermissionMode,
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle, BindAgentInput},
        rpc::{AGENT_RPC_SERVICE_ID, AgentRpcServiceHandle, EmptyPayload, SetPermissionPayload},
        tool_executor::{ToolCallStartedEvent, ToolProgressEvent, ToolResultEvent},
    },
    app::{
        agent_app_runtime::bootstrap_agent_app,
        auth::{OAuthToolkitContract, OAuthToolkitService, config_section::SERVICES_SECTION},
        bootstrap::{BootstrapInput, ensure_kimi_home, resolve_bootstrap_options},
        config::{CONFIG_SERVICE_ID, ConfigTarget},
        event::event_bus::EVENT_BUS_SERVICE_ID,
        message_legacy::{
            MESSAGE_LEGACY_SERVICE_ID, MessageListQuery, PageResponse as MessagePageResponse,
        },
        session_index::SessionSummary,
        session_lifecycle::{CreateSessionOptions, SESSION_LIFECYCLE_SERVICE_ID},
        workspace_registry::{
            WORKSPACE_QUERY_SERVICE_ID, WORKSPACE_REGISTRY_SERVICE_ID, Workspace,
        },
    },
    kosong::{
        model::{
            MODEL_CATALOG_SERVICE_ID, Model, ModelCatalogItem,
            contract::{DEFAULT_MODEL_SECTION, MODELS_SECTION},
            thinking::THINKING_SECTION,
        },
        provider::config::PROVIDERS_SECTION,
    },
    session::{
        agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, MAIN_AGENT_ID, ensure_main_agent},
        interaction::{Interaction, InteractionKind, SESSION_INTERACTION_SERVICE_ID},
    },
};

pub struct KimiCodeDesktopClient {
    home_dir: PathBuf,
    client_version: String,
    oauth: Arc<OAuthToolkitService>,
    app: Scope,
    models_configured: AtomicBool,
    config_gate: AsyncMutex<()>,
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
        })
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

    pub async fn list_models(&self, refresh: bool) -> Result<Vec<DesktopModel>, String> {
        if refresh {
            let models = self.fetch_models().await?;
            self.configure_models(&models).await?;
        } else {
            self.ensure_models_configured().await?;
        }

        self.configured_desktop_models().await
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
        on_event: Arc<dyn Fn(Value) + Send + Sync>,
        on_interactions: Arc<dyn Fn(Vec<DesktopInteraction>) + Send + Sync>,
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
        let event_bus = agent
            .get(EVENT_BUS_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        subscriptions.add(event_bus.subscribe(Arc::new(move |event| {
            on_event(event.clone().into_value());
        })));

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
        before_id: Option<String>,
        page_size: Option<usize>,
    ) -> Result<DesktopMessagePage, String> {
        let messages = self
            .app
            .get(MESSAGE_LEGACY_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let page = messages
            .list(
                conversation_id,
                MessageListQuery {
                    before_id,
                    page_size,
                    ..MessageListQuery::default()
                },
            )
            .await;

        match page {
            Ok(MessagePageResponse { items, has_more }) => {
                Ok(DesktopMessagePage { items, has_more })
            }
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
    use serde_json::json;

    use super::{
        DesktopChatEvent, DesktopChatRequest, DesktopPrepareSessionRequest, KimiCodeDesktopClient,
        ManagedKimiCodeModelInfo, context_usage_from_size, map_desktop_model,
    };
    use crate::{
        agent::{
            context_size::ContextSize, loop_::AssistantDeltaEvent,
            permission_policy::PermissionMode,
        },
        kosong::model::ModelCatalogItem,
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
            turn_id: 7,
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
}
