//! High-level Kimi Code client facade for desktop and other graphical hosts.
//!
//! The facade owns application composition, session lifecycle, managed model
//! configuration, streamed output, and host-mediated interactions.

use std::{
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
    _base::di::scope::Scope,
    agent::{
        context_memory::{ContextMessage, PromptOrigin},
        loop_::LoopRunResult,
        profile::{AGENT_PROFILE_SERVICE_ID, BindAgentInput},
        prompt::{AGENT_PROMPT_SERVICE_ID, PromptCompletionState, PromptInput},
    },
    app::{
        agent_app_runtime::bootstrap_agent_app,
        auth::{OAuthToolkitContract, OAuthToolkitService},
        bootstrap::{BootstrapInput, ensure_kimi_home, resolve_bootstrap_options},
        config::{CONFIG_SERVICE_ID, ConfigTarget},
        event::event_bus::EVENT_BUS_SERVICE_ID,
        session_lifecycle::{CreateSessionOptions, SESSION_LIFECYCLE_SERVICE_ID},
    },
    kosong::contract::message::{ContentPart, Message, Role},
    session::{
        agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, MAIN_AGENT_ID},
        interaction::{Interaction, InteractionKind, SESSION_INTERACTION_SERVICE_ID},
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopChatRequest {
    pub model: String,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub project_path: Option<String>,
    #[serde(default)]
    pub messages: Vec<DesktopChatMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopChatDelta {
    pub kind: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopChatResult {
    pub content: String,
    pub thinking: String,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInteraction {
    pub id: String,
    pub kind: String,
    pub payload: Value,
    pub created_at: i64,
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

    pub async fn chat(
        &self,
        conversation_id: &str,
        request: DesktopChatRequest,
        on_delta: Arc<dyn Fn(DesktopChatDelta) + Send + Sync>,
        on_interactions: Arc<dyn Fn(Vec<DesktopInteraction>) + Send + Sync>,
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
        let prompt = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.trim())
            .filter(|message| !message.is_empty())
            .ok_or_else(|| "A user message is required.".to_owned())?
            .to_owned();

        self.ensure_models_configured().await?;

        let sessions = self
            .app
            .get(SESSION_LIFECYCLE_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let session_id = desktop_session_id(conversation_id);
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
                    session_id: Some(session_id),
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
        on_interactions(map_desktop_interactions(
            interaction.list_pending(None).await,
        ));
        let interaction_for_updates = interaction.clone();
        let on_interactions_for_updates = Arc::clone(&on_interactions);
        let _interaction_updates = interaction.on_did_change_pending().subscribe(move |_| {
            let interaction = interaction_for_updates.clone();
            let on_interactions = Arc::clone(&on_interactions_for_updates);
            tokio::spawn(async move {
                on_interactions(map_desktop_interactions(
                    interaction.list_pending(None).await,
                ));
            });
        });

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

        let content = Arc::new(Mutex::new(String::new()));
        let streamed_content = Arc::clone(&content);
        let event_bus = agent
            .get(EVENT_BUS_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let _assistant_output = event_bus.subscribe_type(
            "assistant.delta",
            Arc::new(move |event| {
                let Some(text) = event.fields.get("delta").and_then(Value::as_str) else {
                    return;
                };
                if let Ok(mut content) = streamed_content.lock() {
                    content.push_str(text);
                }
                on_delta(DesktopChatDelta {
                    kind: "text".to_owned(),
                    content: text.to_owned(),
                });
            }),
        );

        let prompt_service = agent
            .get(AGENT_PROMPT_SERVICE_ID)
            .map_err(|error| error.to_string())?;
        let handle = prompt_service
            .enqueue(PromptInput {
                id: None,
                message: ContextMessage {
                    message: Message::new(
                        Role::User,
                        vec![ContentPart::Text { text: prompt }],
                        Vec::new(),
                    ),
                    id: None,
                    provider_message_id: None,
                    origin: Some(PromptOrigin::User),
                    is_error: None,
                    note: None,
                },
            })
            .await
            .map_err(|error| error.to_string())?;
        let completion = handle.completion().await;

        agent
            .get(WIRE_SERVICE_ID)
            .map_err(|error| error.to_string())?
            .flush()
            .await
            .map_err(|error| error.to_string())?;

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

        Ok(DesktopChatResult {
            content,
            thinking: String::new(),
            finish_reason,
        })
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
        let session_id = desktop_session_id(conversation_id);
        let session = sessions
            .get(&session_id)
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

fn managed_model_alias(model: &str) -> String {
    if model.contains('/') {
        model.to_owned()
    } else {
        format!("kimi-code/{model}")
    }
}

fn desktop_session_id(conversation_id: &str) -> String {
    let normalized = conversation_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("desktop-{normalized}")
}

#[cfg(test)]
mod tests {
    use super::{desktop_session_id, managed_model_alias};

    #[test]
    fn qualifies_managed_model_ids_once() {
        assert_eq!(managed_model_alias("kimi-k2"), "kimi-code/kimi-k2");
        assert_eq!(
            managed_model_alias("kimi-code/kimi-k2"),
            "kimi-code/kimi-k2"
        );
    }

    #[test]
    fn normalizes_frontend_conversation_ids_for_agent_sessions() {
        assert_eq!(
            desktop_session_id("chat_1234/abcd"),
            "desktop-chat-1234-abcd"
        );
    }
}
