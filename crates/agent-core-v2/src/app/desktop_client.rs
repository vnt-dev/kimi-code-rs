//! High-level Kimi Code client facade for desktop and other graphical hosts.
//!
//! The facade owns application composition, session lifecycle, managed model
//! configuration, streamed output, and host-mediated interactions.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use indexmap::IndexMap;
use kimi_code_oauth::{
    CredentialKind, DeviceAuthorization, DeviceCodeObserver, KIMI_CODE_PROVIDER_NAME,
    KimiHostIdentity, KimiIdentityOptions, KimiOAuthLoginOptions, ManagedKimiCodeApplyOptions,
    ManagedKimiCodeModelInfo, OAuthManagerError, apply_managed_kimi_code_config,
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
        context_memory::{ContextMessage, PromptOrigin, protocol_message::ProtocolMessage},
        context_size::{AGENT_CONTEXT_SIZE_SERVICE_ID, AgentContextSizeServiceHandle, ContextSize},
        loop_::{
            AssistantContentEvent, AssistantDeltaEvent, LoopRunResult, ThinkingDeltaEvent,
            ToolCallDeltaEvent, TurnEndedEvent, TurnStartedEvent, TurnStepCompletedEvent,
            TurnStepInterruptedEvent, TurnStepStartedEvent,
        },
        permission_mode::AGENT_PERMISSION_MODE_SERVICE_ID,
        permission_policy::PermissionMode,
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle, BindAgentInput},
        prompt::{AGENT_PROMPT_SERVICE_ID, PromptCompletionState, PromptInput},
        rpc::{
            AGENT_RPC_SERVICE_ID, AgentRpcServiceHandle, PromptMetadataUpdateTarget,
            SetModelPayload, SetPermissionPayload, SetThinkingPayload,
            apply_prompt_metadata_update, prompt_metadata_text_from_content_parts,
        },
        tool_executor::{ToolCallStartedEvent, ToolProgressEvent, ToolResultEvent},
    },
    app::{
        agent_app_runtime::bootstrap_agent_app,
        auth::{OAuthToolkitContract, OAuthToolkitService},
        bootstrap::{BootstrapInput, ensure_kimi_home, resolve_bootstrap_options},
        config::{CONFIG_SERVICE_ID, ConfigTarget},
        event::{
            EVENT_SERVICE_ID,
            event_bus::{DomainEvent, DomainEventPayload, EVENT_BUS_SERVICE_ID},
        },
        message_legacy::{
            MESSAGE_LEGACY_SERVICE_ID, MessageListQuery, PageResponse as MessagePageResponse,
        },
        session_index::SessionSummary,
        session_lifecycle::{CreateSessionOptions, SESSION_LIFECYCLE_SERVICE_ID},
        workspace_registry::{
            WORKSPACE_QUERY_SERVICE_ID, WORKSPACE_REGISTRY_SERVICE_ID, Workspace,
        },
    },
    kosong::contract::message::{ContentPart, Message, Role},
    session::{
        agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, MAIN_AGENT_ID, ensure_main_agent},
        interaction::{Interaction, InteractionKind, SESSION_INTERACTION_SERVICE_ID},
        session_metadata::SESSION_METADATA_ID,
    },
    wire::contract::WIRE_SERVICE_ID,
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

#[derive(Default)]
struct DesktopChatEventRelayState {
    turn_id: Option<i64>,
    emitting: bool,
    buffered: VecDeque<DesktopChatEvent>,
}

struct DesktopChatEventRelay {
    state: Mutex<DesktopChatEventRelayState>,
    content: Arc<Mutex<String>>,
    thinking: Arc<Mutex<String>>,
    callback: Arc<dyn Fn(DesktopChatEvent) + Send + Sync>,
}

impl DesktopChatEvent {
    fn turn_id(&self) -> i64 {
        match self {
            Self::TurnStarted(event) => event.turn_id,
            Self::TurnEnded(event) => event.turn_id,
            Self::StepStarted(event) => event.turn_id,
            Self::StepCompleted(event) => event.turn_id,
            Self::StepInterrupted(event) => event.turn_id,
            Self::AssistantDelta(event) => event.turn_id,
            Self::AssistantContent(event) => event.turn_id,
            Self::ThinkingDelta(event) => event.turn_id,
            Self::ToolCallDelta(event) => event.turn_id,
            Self::ToolCallStarted(event) => event.turn_id,
            Self::ToolProgress(event) => event.turn_id,
            Self::ToolResult(event) => event.turn_id,
        }
    }
}

impl DesktopChatEventRelay {
    fn new(
        content: Arc<Mutex<String>>,
        thinking: Arc<Mutex<String>>,
        callback: Arc<dyn Fn(DesktopChatEvent) + Send + Sync>,
    ) -> Self {
        Self {
            state: Mutex::new(DesktopChatEventRelayState::default()),
            content,
            thinking,
            callback,
        }
    }

    fn push(&self, event: DesktopChatEvent) {
        let should_drain = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            match state.turn_id {
                None => {
                    state.buffered.push_back(event);
                    return;
                }
                Some(turn_id) if turn_id != event.turn_id() => return,
                Some(_) => state.buffered.push_back(event),
            }
            if state.emitting {
                false
            } else {
                state.emitting = true;
                true
            }
        };
        if should_drain {
            self.drain();
        }
    }

    fn select_turn(&self, turn_id: i64) {
        let should_drain = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            state.turn_id = Some(turn_id);
            state.buffered = std::mem::take(&mut state.buffered)
                .into_iter()
                .filter(|event| event.turn_id() == turn_id)
                .collect();
            if state.buffered.is_empty() || state.emitting {
                false
            } else {
                state.emitting = true;
                true
            }
        };
        if should_drain {
            self.drain();
        }
    }

    fn drain(&self) {
        loop {
            let event = {
                let Ok(mut state) = self.state.lock() else {
                    return;
                };
                let Some(event) = state.buffered.pop_front() else {
                    state.emitting = false;
                    return;
                };
                event
            };
            self.emit(event);
        }
    }

    fn emit(&self, event: DesktopChatEvent) {
        match &event {
            DesktopChatEvent::AssistantDelta(event) => {
                if let Ok(mut content) = self.content.lock() {
                    content.push_str(&event.delta);
                }
            }
            DesktopChatEvent::ThinkingDelta(event) => {
                if let Ok(mut thinking) = self.thinking.lock() {
                    thinking.push_str(&event.delta);
                }
            }
            _ => {}
        }
        (self.callback)(event);
    }
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
        let models = self.fetch_models().await?;
        self.configure_models(&models).await?;

        let mut models = models
            .into_iter()
            .map(map_desktop_model)
            .collect::<Vec<_>>();
        models.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        Ok(models)
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
        let session = if let Some(session_id) = request
            .session_id
            .as_deref()
            .filter(|session_id| !session_id.trim().is_empty())
        {
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
                        model: request.model.as_deref().map(managed_model_alias),
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
        if let Some(model) = request.model.filter(|model| !model.trim().is_empty()) {
            rpc.set_model(SetModelPayload {
                model: managed_model_alias(&model),
            })
            .await
            .map_err(|error| error.to_string())?;
        }
        if let Some(thinking) = request
            .thinking
            .filter(|thinking| !thinking.trim().is_empty())
        {
            rpc.set_thinking(SetThinkingPayload { level: thinking })
                .await
                .map_err(|error| error.to_string())?;
        }
        if let Some(permission) = request.permission {
            rpc.set_permission(SetPermissionPayload { mode: permission })
                .await
                .map_err(|error| error.to_string())?;
        }

        Ok(DesktopPreparedSession {
            session_id: session.id().to_owned(),
            agent_id: agent.id().to_owned(),
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

    pub async fn chat(
        &self,
        conversation_id: &str,
        request: DesktopChatRequest,
        on_event: Arc<dyn Fn(DesktopChatEvent) + Send + Sync>,
        on_interactions: Arc<dyn Fn(Vec<DesktopInteraction>) + Send + Sync>,
        on_compaction: Arc<dyn Fn(DesktopCompactionEvent) + Send + Sync>,
        on_context_usage: Arc<dyn Fn(DesktopContextUsage) + Send + Sync>,
    ) -> Result<DesktopChatResult, String> {
        if request.model.trim().is_empty() {
            return Err("A model must be selected.".to_owned());
        }
        let work_dir = request
            .project_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| "A project directory is required.".to_owned())?
            .to_owned();
        let prompt = request.prompt.trim();
        if prompt.is_empty() {
            return Err("A user message is required.".to_owned());
        }
        let prompt = prompt.to_owned();

        self.ensure_models_configured().await?;

        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session_id = conversation_id.to_owned();
        let model = managed_model_alias(&request.model);
        let session = if let Some(session) = sessions.get(&session_id) {
            session
        } else if let Some(session) = sessions
            .resume(&session_id)
            .await
            .map_err(|error| error.to_string())?
        {
            session
        } else {
            sessions
                .create(CreateSessionOptions {
                    session_id: Some(session_id.clone()),
                    work_dir: work_dir.clone(),
                    main_agent_binding: Some(BindAgentInput {
                        profile: "agent".into(),
                        model: Some(model.clone()),
                        thinking: request.effort.clone(),
                        strict_thinking: None,
                        cwd: Some(work_dir),
                    }),
                    ..CreateSessionOptions::default()
                })
                .await
                .map_err(|error| error.to_string())?
        };

        let agents = session
            .get(AGENT_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let agent = agents
            .get(MAIN_AGENT_ID)
            .ok_or_else(|| "the session did not create its main agent".to_owned())?;

        let interaction = session
            .get(SESSION_INTERACTION_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let subscriptions = DisposableStore::new();
        on_interactions(map_desktop_interactions(
            interaction.list_pending(None).await,
        ));
        let interaction_for_updates = interaction.clone();
        let on_interactions_for_updates = Arc::clone(&on_interactions);
        subscriptions.add(interaction.on_did_change_pending().subscribe(move |_| {
            let interaction = interaction_for_updates.clone();
            let on_interactions = Arc::clone(&on_interactions_for_updates);
            tokio::spawn(async move {
                on_interactions(map_desktop_interactions(
                    interaction.list_pending(None).await,
                ));
            });
        }));

        let profile = agent
            .get(AGENT_PROFILE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        profile
            .set_model(model)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(effort) = request.effort.filter(|effort| !effort.is_empty()) {
            profile
                .set_thinking(effort)
                .map_err(|error| error.to_string())?;
        }
        let permission_mode = match request.permission_mode.as_deref() {
            Some("yolo" | "full_access") => PermissionMode::Yolo,
            Some("auto") => PermissionMode::Auto,
            _ => PermissionMode::Manual,
        };
        agent
            .get(AGENT_PERMISSION_MODE_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .set_mode(permission_mode)
            .map_err(|error| error.to_string())?;

        let context_size = agent
            .get(AGENT_CONTEXT_SIZE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        emit_context_usage(&context_size, &profile, &on_context_usage);

        let content = Arc::new(Mutex::new(String::new()));
        let thinking = Arc::new(Mutex::new(String::new()));
        let chat_event_relay = Arc::new(DesktopChatEventRelay::new(
            Arc::clone(&content),
            Arc::clone(&thinking),
            on_event,
        ));
        let event_bus = agent
            .get(EVENT_BUS_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let context_size_for_updates = context_size.clone();
        let profile_for_updates = profile.clone();
        let on_context_usage_for_updates = Arc::clone(&on_context_usage);
        subscriptions.add(event_bus.subscribe(Arc::new(move |event| {
            if matches!(
                event.event_type.as_str(),
                "agent.status.updated" | "context.spliced"
            ) {
                emit_context_usage(
                    &context_size_for_updates,
                    &profile_for_updates,
                    &on_context_usage_for_updates,
                );
            }
        })));
        subscriptions.add(event_bus.subscribe(Arc::new(move |event| {
            if let Some(event) = map_desktop_compaction_event(event) {
                on_compaction(event);
            }
        })));
        let relay_for_events = Arc::clone(&chat_event_relay);
        subscriptions.add(event_bus.subscribe(Arc::new(move |event| {
            if let Some(event) = map_desktop_chat_event(event) {
                relay_for_events.push(event);
            }
        })));

        let metadata = session
            .get(SESSION_METADATA_ID)
            .map_err(|error| error.to_string())?;
        let event_service = self
            .app
            .get(EVENT_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let prompt_content = vec![ContentPart::Text { text: prompt }];
        let prompt_metadata = prompt_metadata_text_from_content_parts(&prompt_content);
        apply_prompt_metadata_update(
            PromptMetadataUpdateTarget {
                metadata: metadata.0.as_ref(),
                event_service: event_service.0.as_ref(),
                session_id: &session_id,
            },
            prompt_metadata.as_deref(),
        )
        .await
        .map_err(|error| error.to_string())?;

        let prompt_service = agent
            .get(AGENT_PROMPT_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let handle = prompt_service
            .enqueue(PromptInput {
                id: None,
                message: ContextMessage {
                    message: Message::new(Role::User, prompt_content, Vec::new()),
                    id: None,
                    provider_message_id: None,
                    origin: Some(PromptOrigin::User),
                    is_error: None,
                    note: None,
                },
            })
            .await
            .map_err(|error| error.to_string())?;
        if let Some(turn) = handle.launched().await {
            chat_event_relay.select_turn(turn.id());
        }
        let completion = handle.completion().await;

        agent
            .get(WIRE_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .flush()
            .await
            .map_err(|error| error.to_string())?;
        emit_context_usage(&context_size, &profile, &on_context_usage);

        let finish_reason = match completion.result {
            Some(LoopRunResult::Completed { steps, truncated }) => {
                Some(format!("completed:steps={steps},truncated={truncated}"))
            }
            Some(LoopRunResult::Failed { error, .. }) => return Err(error.to_string()),
            Some(LoopRunResult::Cancelled { reason, .. }) => return Err(reason.to_string()),
            None if completion.state == PromptCompletionState::Blocked => {
                return Err("The prompt was blocked by a submission hook.".to_owned());
            }
            None => {
                return Err(format!("The prompt did not launch: {:?}", completion.state));
            }
        };
        let content = content
            .lock()
            .map_err(|_| "failed to collect assistant output".to_owned())?
            .clone();
        let thinking = thinking
            .lock()
            .map_err(|_| "failed to collect assistant thinking".to_owned())?
            .clone();

        Ok(DesktopChatResult {
            content,
            thinking,
            finish_reason,
        })
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

        let mut generated = Map::new();
        apply_managed_kimi_code_config(
            &mut generated,
            ManagedKimiCodeApplyOptions {
                models,
                base_url: None,
                oauth_key: None,
                oauth_host: None,
                preserve_default_model: false,
            },
        )
        .map_err(|error| error.to_string())?;
        for (section, value) in generated {
            config
                .replace(&section, Some(value), ConfigTarget::Memory)
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

fn map_desktop_model(model: ManagedKimiCodeModelInfo) -> DesktopModel {
    DesktopModel {
        display_name: model
            .display_name
            .clone()
            .unwrap_or_else(|| model.id.clone()),
        id: model.id,
        context_length: model.context_length,
        supports_reasoning: model.supports_reasoning,
        supports_image: model.supports_image_in,
        supports_video: model.supports_video_in,
        supports_tools: model.supports_tool_use,
        protocol: match model.protocol {
            Some(kimi_code_oauth::ManagedKimiCodeProtocol::Anthropic) => "anthropic",
            None => "openai",
        }
        .to_owned(),
        support_efforts: model.support_efforts.unwrap_or_default(),
        default_effort: model.default_effort,
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

fn emit_context_usage(
    context_size: &AgentContextSizeServiceHandle,
    profile: &AgentProfileServiceHandle,
    callback: &Arc<dyn Fn(DesktopContextUsage) + Send + Sync>,
) {
    if let Ok(usage) = context_usage_snapshot(context_size, profile) {
        callback(usage);
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

fn map_desktop_chat_event(event: &DomainEvent) -> Option<DesktopChatEvent> {
    match event.event_type.as_str() {
        TurnStartedEvent::TYPE => event
            .with_payload::<TurnStartedEvent, _>(|event| {
                DesktopChatEvent::TurnStarted(event.clone())
            })
            .ok(),
        TurnEndedEvent::TYPE => event
            .with_payload::<TurnEndedEvent, _>(|event| DesktopChatEvent::TurnEnded(event.clone()))
            .ok(),
        TurnStepStartedEvent::TYPE => event
            .with_payload::<TurnStepStartedEvent, _>(|event| {
                DesktopChatEvent::StepStarted(event.clone())
            })
            .ok(),
        TurnStepCompletedEvent::TYPE => event
            .with_payload::<TurnStepCompletedEvent, _>(|event| {
                DesktopChatEvent::StepCompleted(event.clone())
            })
            .ok(),
        TurnStepInterruptedEvent::TYPE => event
            .with_payload::<TurnStepInterruptedEvent, _>(|event| {
                DesktopChatEvent::StepInterrupted(event.clone())
            })
            .ok(),
        AssistantDeltaEvent::TYPE => event
            .with_payload::<AssistantDeltaEvent, _>(|event| {
                DesktopChatEvent::AssistantDelta(event.clone())
            })
            .ok(),
        AssistantContentEvent::TYPE => event
            .with_payload::<AssistantContentEvent, _>(|event| {
                DesktopChatEvent::AssistantContent(event.clone())
            })
            .ok(),
        ThinkingDeltaEvent::TYPE => event
            .with_payload::<ThinkingDeltaEvent, _>(|event| {
                DesktopChatEvent::ThinkingDelta(event.clone())
            })
            .ok(),
        ToolCallDeltaEvent::TYPE => event
            .with_payload::<ToolCallDeltaEvent, _>(|event| {
                DesktopChatEvent::ToolCallDelta(event.clone())
            })
            .ok(),
        ToolCallStartedEvent::TYPE => event
            .with_payload::<ToolCallStartedEvent, _>(|event| {
                DesktopChatEvent::ToolCallStarted(event.clone())
            })
            .ok(),
        ToolProgressEvent::TYPE => event
            .with_payload::<ToolProgressEvent, _>(|event| {
                DesktopChatEvent::ToolProgress(event.clone())
            })
            .ok(),
        ToolResultEvent::TYPE => event
            .with_payload::<ToolResultEvent, _>(|event| DesktopChatEvent::ToolResult(event.clone()))
            .ok(),
        _ => None,
    }
}

fn map_desktop_compaction_event(event: &DomainEvent) -> Option<DesktopCompactionEvent> {
    match event.event_type.as_str() {
        "compaction.started" => Some(DesktopCompactionEvent {
            phase: "started".to_owned(),
            trigger: event
                .fields
                .get("trigger")
                .and_then(Value::as_str)
                .map(str::to_owned),
            compacted_count: None,
            tokens_before: None,
            tokens_after: None,
        }),
        "compaction.completed" => {
            let result = event.fields.get("result").and_then(Value::as_object);
            Some(DesktopCompactionEvent {
                phase: "completed".to_owned(),
                trigger: None,
                compacted_count: result
                    .and_then(|result| result.get("compactedCount"))
                    .and_then(Value::as_f64),
                tokens_before: result
                    .and_then(|result| result.get("tokensBefore"))
                    .and_then(Value::as_f64),
                tokens_after: result
                    .and_then(|result| result.get("tokensAfter"))
                    .and_then(Value::as_f64),
            })
        }
        "compaction.cancelled" => Some(DesktopCompactionEvent {
            phase: "cancelled".to_owned(),
            trigger: None,
            compacted_count: None,
            tokens_before: None,
            tokens_after: None,
        }),
        _ => None,
    }
}

fn managed_model_alias(model: &str) -> String {
    if model.contains('/') {
        model.to_owned()
    } else {
        format!("kimi-code/{model}")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::{
        DesktopChatEvent, DesktopChatEventRelay, DesktopChatRequest, context_usage_from_size,
        managed_model_alias, map_desktop_chat_event, map_desktop_compaction_event,
    };
    use crate::{
        agent::{
            context_size::ContextSize,
            loop_::{AssistantDeltaEvent, ThinkingDeltaEvent},
        },
        app::event::event_bus::DomainEvent,
    };

    #[test]
    fn qualifies_managed_model_ids_once() {
        assert_eq!(managed_model_alias("kimi-k2"), "kimi-code/kimi-k2");
        assert_eq!(
            managed_model_alias("kimi-code/kimi-k2"),
            "kimi-code/kimi-k2"
        );
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
    fn maps_and_relays_only_the_launched_turn_in_original_event_order() {
        let mapped = map_desktop_chat_event(&DomainEvent::typed(AssistantDeltaEvent {
            turn_id: 11,
            delta: "a".into(),
        }))
        .unwrap();
        assert!(matches!(
            mapped,
            DesktopChatEvent::AssistantDelta(AssistantDeltaEvent {
                turn_id: 11,
                ref delta
            }) if delta == "a"
        ));

        let received = Arc::new(Mutex::new(Vec::new()));
        let received_for_callback = Arc::clone(&received);
        let content = Arc::new(Mutex::new(String::new()));
        let thinking = Arc::new(Mutex::new(String::new()));
        let relay = DesktopChatEventRelay::new(
            Arc::clone(&content),
            Arc::clone(&thinking),
            Arc::new(move |event| received_for_callback.lock().unwrap().push(event)),
        );
        relay.push(DesktopChatEvent::AssistantDelta(AssistantDeltaEvent {
            turn_id: 12,
            delta: "other".into(),
        }));
        relay.push(DesktopChatEvent::AssistantDelta(AssistantDeltaEvent {
            turn_id: 11,
            delta: "hello".into(),
        }));
        relay.select_turn(11);
        relay.push(DesktopChatEvent::ThinkingDelta(ThinkingDeltaEvent {
            turn_id: 11,
            delta: "reason".into(),
        }));

        assert_eq!(received.lock().unwrap().len(), 2);
        assert_eq!(*content.lock().unwrap(), "hello");
        assert_eq!(*thinking.lock().unwrap(), "reason");
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

    #[test]
    fn maps_compaction_lifecycle_events_for_desktop_hosts() {
        let started = DomainEvent::try_from(json!({
            "type": "compaction.started",
            "trigger": "auto"
        }))
        .unwrap();
        let started = map_desktop_compaction_event(&started).unwrap();
        assert_eq!(started.phase, "started");
        assert_eq!(started.trigger.as_deref(), Some("auto"));

        let completed = DomainEvent::try_from(json!({
            "type": "compaction.completed",
            "result": {
                "compactedCount": 12,
                "tokensBefore": 64000,
                "tokensAfter": 9000
            }
        }))
        .unwrap();
        let completed = map_desktop_compaction_event(&completed).unwrap();
        assert_eq!(completed.phase, "completed");
        assert_eq!(completed.compacted_count, Some(12.0));
        assert_eq!(completed.tokens_before, Some(64_000.0));
        assert_eq!(completed.tokens_after, Some(9_000.0));

        let unrelated = DomainEvent::try_from(json!({"type": "assistant.delta"})).unwrap();
        assert!(map_desktop_compaction_event(&unrelated).is_none());
    }
}
