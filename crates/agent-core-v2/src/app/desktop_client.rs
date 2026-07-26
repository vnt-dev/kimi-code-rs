//! High-level Kimi Code client facade for desktop and other graphical hosts.
//!
//! The UI should not need to compose the OAuth toolkit, managed model catalog,
//! protocol registry, and streamed message contract itself. This module keeps
//! that boundary inside agent-core-v2 while exposing serializable DTOs.

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use futures_util::StreamExt;
use indexmap::IndexMap;
use kimi_code_oauth::{
    CredentialKind, DeviceAuthorization, DeviceCodeObserver, KIMI_CODE_PROVIDER_NAME,
    KimiHostIdentity, KimiIdentityOptions, KimiOAuthLoginOptions, OAuthManagerError,
    create_kimi_default_headers, fetch_managed_kimi_code_models,
    managed_usage::DEFAULT_KIMI_CODE_BASE_URL,
};
use serde::{Deserialize, Serialize};

use crate::{
    app::{
        auth::{OAuthToolkitContract, OAuthToolkitService},
        bootstrap::ensure_kimi_home,
    },
    kosong::{
        contract::{
            message::{
                ContentPart, Message, StreamedMessagePart, create_assistant_message,
                create_user_message,
            },
            provider::{GenerateOptions, ThinkingEffort, ThinkingRequestOptions},
        },
        protocol::identity::{
            Protocol, ProtocolAdapterConfig, ProtocolAdapterRegistry as _, ProtocolProviderOptions,
        },
        provider::{
            bases::{
                anthropic::anthropic_contrib::ensure_anthropic_base_registered,
                openai::openai_legacy_contrib::ensure_openai_legacy_base_registered,
            },
            protocol_adapter_registry::ProtocolAdapterRegistry,
            providers::ensure_provider_definitions_registered,
        },
    },
};

const DEFAULT_SYSTEM_PROMPT: &str = "You are Kimi Code, a careful and capable coding agent. \
Understand the user's goal, reason from the supplied project context, and give concrete, \
implementation-ready help. Keep answers direct, accurate, and easy to act on.";

#[derive(Clone)]
pub struct KimiCodeDesktopClient {
    home_dir: PathBuf,
    client_version: String,
    oauth: Arc<OAuthToolkitService>,
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
    pub fn new(
        home_dir: impl Into<PathBuf>,
        client_version: impl Into<String>,
    ) -> Result<Self, String> {
        let home_dir = home_dir.into();
        ensure_kimi_home(&home_dir).map_err(|error| error.to_string())?;
        let oauth = OAuthToolkitService::new(&home_dir).map_err(|error| error.to_string())?;
        ensure_provider_definitions_registered().map_err(|error| error.to_string())?;
        ensure_anthropic_base_registered().map_err(|error| error.to_string())?;
        ensure_openai_legacy_base_registered().map_err(|error| error.to_string())?;
        Ok(Self {
            home_dir,
            client_version: client_version.into(),
            oauth: Arc::new(oauth),
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
                    provision_config: Some(false),
                    ..KimiOAuthLoginOptions::default()
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        self.auth_status().await
    }

    pub async fn logout(&self) -> Result<DesktopAuthStatus, String> {
        self.oauth
            .logout(Some(KIMI_CODE_PROVIDER_NAME), None)
            .await
            .map_err(|error| error.to_string())?;
        self.auth_status().await
    }

    pub async fn list_models(&self) -> Result<Vec<DesktopModel>, String> {
        let token = self.fresh_token().await?;
        let headers = self.identity_headers()?;
        let mut models = fetch_managed_kimi_code_models(
            &token,
            Some(DEFAULT_KIMI_CODE_BASE_URL),
            Some(&headers),
            CredentialKind::OAuth,
        )
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|model| DesktopModel {
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
        })
        .collect::<Vec<_>>();
        models.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        Ok(models)
    }

    pub async fn chat(
        &self,
        request: DesktopChatRequest,
        on_delta: Arc<dyn Fn(DesktopChatDelta) + Send + Sync>,
    ) -> Result<DesktopChatResult, String> {
        if request.model.trim().is_empty() {
            return Err("A model must be selected.".to_owned());
        }
        let token = self.fresh_token().await?;
        let protocol = match request.protocol.as_deref() {
            Some("openai") => Protocol::OpenAi,
            _ => Protocol::Anthropic,
        };
        let provider = ProtocolAdapterRegistry::new()
            .create_chat_provider(ProtocolAdapterConfig {
                protocol,
                provider_type: Some("kimi".to_owned()),
                base_url: Some(DEFAULT_KIMI_CODE_BASE_URL.to_owned()),
                model_name: request.model.clone(),
                api_key: Some(token),
                default_headers: Some(self.identity_headers()?),
                provider_options: Some(ProtocolProviderOptions {
                    adaptive_thinking: Some(true),
                    beta_api: Some(protocol == Protocol::Anthropic),
                    default_max_tokens: Some(32_768.0),
                    ..ProtocolProviderOptions::default()
                }),
            })
            .map_err(|error| error.to_string())?;

        let history = request
            .messages
            .iter()
            .filter(|message| !message.content.trim().is_empty())
            .map(|message| match message.role.as_str() {
                "assistant" => create_assistant_message(
                    vec![ContentPart::Text {
                        text: message.content.clone(),
                    }],
                    None,
                ),
                _ => create_user_message(message.content.clone()),
            })
            .collect::<Vec<Message>>();
        let project_context = request
            .project_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(|path| format!("\n\nThe active project directory is: {path}"))
            .unwrap_or_default();
        let system_prompt = format!("{DEFAULT_SYSTEM_PROMPT}{project_context}");
        let thinking = request
            .effort
            .as_deref()
            .filter(|value| !value.is_empty() && *value != "off")
            .map(|value| ThinkingRequestOptions {
                effort: ThinkingEffort::new(value),
                keep: None,
            });
        let options = GenerateOptions {
            thinking,
            cache_key: request.project_path.clone(),
            ..GenerateOptions::default()
        };
        let mut stream = provider
            .generate(&system_prompt, &[], &history, Some(&options))
            .await
            .map_err(|error| error.to_string())?;
        let mut content = String::new();
        let mut thinking = String::new();
        while let Some(part) = stream.next().await {
            match part.map_err(|error| error.to_string())? {
                StreamedMessagePart::Content(ContentPart::Text { text }) => {
                    content.push_str(&text);
                    on_delta(DesktopChatDelta {
                        kind: "text".to_owned(),
                        content: text,
                    });
                }
                StreamedMessagePart::Content(ContentPart::Think { think, .. }) => {
                    thinking.push_str(&think);
                    on_delta(DesktopChatDelta {
                        kind: "thinking".to_owned(),
                        content: think,
                    });
                }
                _ => {}
            }
        }
        Ok(DesktopChatResult {
            content,
            thinking,
            finish_reason: stream.finish_reason().map(|reason| format!("{reason:?}")),
        })
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
