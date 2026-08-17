//! Pure model-catalog data, auth, and projection helpers.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/catalog.ts`.
//!
//! The service contract and cache-owning implementation are migrated with the
//! requester and inspection types they require. This module owns the source
//! file's independent data and projection methods.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use kimi_code_oauth::parse_kimi_code_custom_headers;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::kosong::{
    contract::{capability::ModelCapability, provider::ProviderRequestAuth, usage::TokenUsage},
    protocol::identity::{Protocol, ProtocolProviderOptions, ReasoningHistoryMode},
    provider::{
        config::{ProviderConfig, ProviderType},
        provider_definition::{
            HostHeaders, ProviderDefinitionRegistryError, get_provider_definition,
            resolve_provider_endpoint,
        },
    },
};

use super::{
    contract::{ModelRecord, ModelsSection},
    model_auth::effective_model_config,
    thinking::drives_thinking_through_traits,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AuthRequestOptions {
    pub force: bool,
}

#[async_trait]
pub trait AuthProvider: Send + Sync {
    // Original: AuthProvider.canRefresh.
    fn can_refresh(&self) -> bool {
        false
    }

    // Original: AuthProvider.getAuth(options?).
    async fn get_auth(
        &self,
        options: Option<AuthRequestOptions>,
    ) -> Result<Option<ProviderRequestAuth>, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StaticAuthProvider {
    api_key: Option<String>,
}

impl StaticAuthProvider {
    // Original: StaticAuthProvider.constructor().
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl AuthProvider for StaticAuthProvider {
    // Original: StaticAuthProvider.getAuth(). The whitespace test decides
    // whether credentials exist, but the original untrimmed API key is sent.
    async fn get_auth(
        &self,
        _options: Option<AuthRequestOptions>,
    ) -> Result<Option<ProviderRequestAuth>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .api_key
            .as_ref()
            .filter(|key| !key.trim().is_empty())
            .map(|api_key| ProviderRequestAuth {
                api_key: Some(api_key.clone()),
                headers: None,
            }))
    }
}

#[derive(Clone)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub protocol: Protocol,
    pub base_url: Option<String>,
    pub headers: IndexMap<String, String>,
    pub capabilities: ModelCapability,
    pub max_context_size: u64,
    pub max_output_size: Option<u64>,
    pub display_name: Option<String>,
    pub reasoning_key: Option<String>,
    pub reasoning_history: Option<ReasoningHistoryMode>,
    pub support_efforts: Option<Vec<String>>,
    pub default_effort: Option<String>,
    pub always_thinking: bool,
    pub provider_type: Option<ProviderType>,
    pub provider_name: String,
    pub auth_provider: Arc<dyn AuthProvider>,
    pub provider_options: Option<ProtocolProviderOptions>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPingResult {
    pub ok: bool,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelCatalogItem {
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub max_context_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderCatalogStatus {
    Connected,
    Error,
    Unconfigured,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCatalogItem {
    pub id: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    pub has_api_key: bool,
    pub status: ProviderCatalogStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetDefaultModelResponse {
    pub default_model: String,
    pub model: ModelCatalogItem,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProviderCredentialState {
    pub has_api_key: bool,
    pub has_oauth_token: bool,
}

// Original: toProtocolModel(). The effective-model pass may resolve provider
// traits, so its source exception becomes a Result at this Rust boundary.
pub fn to_protocol_model(
    model: &Model,
    record: &ModelRecord,
    provider_type: Option<&str>,
) -> Result<
    ModelCatalogItem,
    crate::kosong::provider::provider_definition::ProviderDefinitionRegistryError,
> {
    let effective = effective_model_config(
        record,
        provider_type.or_else(|| model.provider_type.as_ref().map(ProviderType::as_str)),
    )?;
    Ok(ModelCatalogItem {
        provider: model.provider_name.clone(),
        model: model.id.clone(),
        display_name: Some(
            model
                .display_name
                .clone()
                .unwrap_or_else(|| model.name.clone()),
        ),
        max_context_size: model.max_context_size,
        capabilities: effective.capabilities,
        support_efforts: model.support_efforts.clone(),
        default_effort: model.default_effort.clone(),
    })
}

// Original: toProtocolModelFallback().
pub fn to_protocol_model_fallback(
    model_id: &str,
    record: &ModelRecord,
    provider_type: Option<&str>,
) -> Result<
    ModelCatalogItem,
    crate::kosong::provider::provider_definition::ProviderDefinitionRegistryError,
> {
    let effective = effective_model_config(record, provider_type)?;
    Ok(ModelCatalogItem {
        provider: effective.provider.unwrap_or_default(),
        model: model_id.to_owned(),
        display_name: Some(
            effective
                .display_name
                .or(effective.model)
                .unwrap_or_else(|| model_id.to_owned()),
        ),
        max_context_size: effective.max_context_size.map_or(0, |size| size.get()),
        capabilities: effective.capabilities,
        support_efforts: effective.support_efforts,
        default_effort: effective.default_effort,
    })
}

// Original: toProtocolProvider().
pub fn to_protocol_provider(
    provider_id: &str,
    provider: &ProviderConfig,
    models: &ModelsSection,
    global_default_model: Option<&str>,
    credential: ProviderCredentialState,
) -> ProviderCatalogItem {
    let provider_models = model_ids_for_provider(models, provider_id);
    let default_model = provider
        .default_model
        .clone()
        .or_else(|| global_default_for_provider(models, global_default_model, provider_id));
    ProviderCatalogItem {
        id: provider_id.to_owned(),
        provider_type: provider
            .provider_type
            .as_ref()
            .map_or_else(|| "openai".to_owned(), ToString::to_string),
        base_url: provider.base_url.clone(),
        default_model,
        has_api_key: credential.has_api_key,
        status: if credential.has_api_key || credential.has_oauth_token {
            ProviderCatalogStatus::Connected
        } else {
            ProviderCatalogStatus::Unconfigured
        },
        models: Some(provider_models),
    }
}

// Original: modelIdsForProvider(). IndexMap preserves JavaScript object
// enumeration order for configured model ids.
pub fn model_ids_for_provider(models: &ModelsSection, provider_id: &str) -> Vec<String> {
    models
        .iter()
        .filter(|(_, record)| record.provider.as_deref() == Some(provider_id))
        .map(|(model_id, _)| model_id.clone())
        .collect()
}

// Original: globalDefaultForProvider().
pub fn global_default_for_provider(
    models: &ModelsSection,
    global_default_model: Option<&str>,
    provider_id: &str,
) -> Option<String> {
    let model_id = global_default_model?;
    (models
        .get(model_id)
        .and_then(|record| record.provider.as_deref())
        == Some(provider_id))
    .then(|| model_id.to_owned())
}

// Original:
//   packages/agent-core-v2/src/kosong/model/catalogService.ts
//   resolveOutboundHeaders()
pub fn resolve_outbound_headers(
    provider_type: Option<&str>,
    custom_headers: Option<&IndexMap<String, String>>,
    host_headers: &IndexMap<String, String>,
) -> Result<IndexMap<String, String>, ProviderDefinitionRegistryError> {
    let forwards_all = provider_type
        .map(|provider_type| get_provider_definition(provider_type, None))
        .transpose()?
        .flatten()
        .is_some_and(|definition| definition.host_headers == Some(HostHeaders::Full));
    let mut headers = parse_kimi_code_custom_headers(&std::env::vars().collect());
    if forwards_all {
        headers.extend(host_headers.clone());
    } else if let Some(user_agent) = host_headers.get("User-Agent") {
        headers.insert("User-Agent".into(), user_agent.clone());
    }
    if let Some(custom_headers) = custom_headers {
        headers.extend(custom_headers.clone());
    }
    Ok(headers)
}

// Original: catalogService.ts, resolveModelCapabilities().
pub fn resolve_model_capabilities(
    declared_capabilities: Option<&[String]>,
    detected: &ModelCapability,
    max_context_size: u64,
) -> ModelCapability {
    let declared = declared_capabilities
        .unwrap_or_default()
        .iter()
        .map(|capability| capability.trim().to_ascii_lowercase())
        .collect::<HashSet<_>>();
    ModelCapability {
        image_in: declared.contains("image_in") || detected.image_in,
        video_in: declared.contains("video_in") || detected.video_in,
        audio_in: declared.contains("audio_in") || detected.audio_in,
        thinking: declared.contains("thinking")
            || declared.contains("always_thinking")
            || detected.thinking,
        tool_use: declared.contains("tool_use") || detected.tool_use,
        max_context_tokens: max_context_size,
        dynamically_loaded_tools: Some(
            declared.contains("dynamically_loaded_tools")
                || detected.dynamically_loaded_tools == Some(true),
        ),
    }
}

// Original: catalogService.ts, stripTrailingV1().
pub fn strip_trailing_v1(base_url: &str) -> String {
    base_url
        .strip_suffix("/v1/")
        .or_else(|| base_url.strip_suffix("/v1"))
        .unwrap_or(base_url)
        .to_owned()
}

// Original: catalogService.ts, buildProtocolProviderOptions().
pub fn build_protocol_provider_options(
    model: &ModelRecord,
    protocol: Protocol,
    provider: Option<&ProviderConfig>,
    base_url: Option<&str>,
) -> Option<ProtocolProviderOptions> {
    let options = match protocol {
        Protocol::Anthropic => ProtocolProviderOptions {
            default_max_tokens: model.max_output_size.map(|size| size.get()),
            support_efforts: model.support_efforts.clone(),
            adaptive_thinking: model.adaptive_thinking,
            beta_api: model.beta_api,
            ..ProtocolProviderOptions::default()
        },
        Protocol::OpenAi => ProtocolProviderOptions {
            reasoning_key: super::model_auth::non_empty(model.reasoning_key.as_deref())
                .map(str::to_owned),
            reasoning_history: model.reasoning_history,
            ..ProtocolProviderOptions::default()
        },
        Protocol::GoogleGenAi => {
            let project = env_value(
                provider.and_then(|provider| provider.env.as_ref()),
                "GOOGLE_CLOUD_PROJECT",
            );
            let location = env_value(
                provider.and_then(|provider| provider.env.as_ref()),
                "GOOGLE_CLOUD_LOCATION",
            )
            .or_else(|| location_from_vertex_ai_base_url(base_url));
            match (project, location) {
                (Some(project), Some(location)) => ProtocolProviderOptions {
                    vertexai: Some(true),
                    project: Some(project),
                    location: Some(location),
                    ..ProtocolProviderOptions::default()
                },
                _ => ProtocolProviderOptions::default(),
            }
        }
        Protocol::OpenAiResponses => ProtocolProviderOptions::default(),
    };
    (options.reasoning_key.is_some()
        || options.reasoning_history.is_some()
        || options.default_max_tokens.is_some()
        || options.support_efforts.is_some()
        || options.adaptive_thinking.is_some()
        || options.beta_api.is_some()
        || options.metadata.is_some()
        || options.vertexai.is_some()
        || options.project.is_some()
        || options.location.is_some())
    .then_some(options)
}

// Original: catalogService.ts, profileForAttribution().
pub fn profile_for_attribution(
    configured_model: &ModelRecord,
    provider_config: Option<&ProviderConfig>,
    wire_name: Option<&str>,
) -> Result<
    (
        Option<crate::kosong::provider::bases::anthropic::anthropic_profile::AnthropicModelProfile>,
        bool,
    ),
    ProviderDefinitionRegistryError,
> {
    use crate::kosong::provider::bases::anthropic::anthropic_profile::{
        LATEST_OPUS_PROFILE, match_known_anthropic_model_profile,
    };

    let Some(wire_name) = wire_name else {
        return Ok((None, false));
    };
    let profile_arg = provider_config
        .and_then(|provider| provider.provider_type.as_ref())
        .map(ProviderType::as_str)
        .or_else(|| configured_model.protocol.map(Protocol::as_str));
    let gate_protocol = configured_model
        .protocol
        .map(Protocol::as_str)
        .or(profile_arg);
    let known = match_known_anthropic_model_profile(wire_name);
    let infer = profile_arg.is_some()
        && !drives_thinking_through_traits(profile_arg)?
        && gate_protocol == Some("anthropic");
    if infer {
        return Ok((Some(known.unwrap_or(LATEST_OPUS_PROFILE)), known.is_none()));
    }
    Ok((known, false))
}

// Original: catalogService.ts, locationFromVertexAIBaseUrl().
pub fn location_from_vertex_ai_base_url(base_url: Option<&str>) -> Option<String> {
    let base_url = super::model_auth::non_empty(base_url)?;
    let url = Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    let suffix = "-aiplatform.googleapis.com";
    host.strip_suffix(suffix)
        .and_then(|location| super::model_auth::non_empty(Some(location)))
        .map(str::to_owned)
}

// Original: catalogService.ts, hasConfiguredApiKey().
pub fn has_configured_api_key(
    provider: &ProviderConfig,
) -> Result<bool, ProviderDefinitionRegistryError> {
    if super::model_auth::non_empty(provider.api_key.as_deref()).is_some() {
        return Ok(true);
    }
    let Some(provider_type) = provider.provider_type.as_ref() else {
        return Ok(false);
    };
    Ok(resolve_provider_endpoint(
        provider_type.as_str(),
        provider.env.as_ref().unwrap_or(&IndexMap::new()),
    )?
    .api_key
    .is_some())
}

fn env_value(env: Option<&IndexMap<String, String>>, key: &str) -> Option<String> {
    super::model_auth::non_empty(env.and_then(|env| env.get(key).map(String::as_str)))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use crate::kosong::{contract::capability::UNKNOWN_CAPABILITY, provider::config::ProviderType};

    use super::*;

    #[tokio::test]
    async fn static_auth_preserves_the_original_untrimmed_api_key_but_rejects_blank_keys() {
        let auth = StaticAuthProvider::new(Some("  secret  ".into()));
        assert!(!auth.can_refresh());
        assert_eq!(
            auth.get_auth(None)
                .await
                .unwrap()
                .unwrap()
                .api_key
                .as_deref(),
            Some("  secret  ")
        );
        assert!(
            StaticAuthProvider::new(Some(" \t\n".into()))
                .get_auth(None)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn provider_projection_keeps_default_selection_status_and_model_order() {
        let mut models = ModelsSection::new();
        models.insert(
            "first".into(),
            ModelRecord {
                provider: Some("kimi".into()),
                max_context_size: Some(NonZeroU64::new(1).unwrap()),
                ..ModelRecord::default()
            },
        );
        models.insert(
            "other".into(),
            ModelRecord {
                provider: Some("other".into()),
                ..ModelRecord::default()
            },
        );
        models.insert(
            "last".into(),
            ModelRecord {
                provider: Some("kimi".into()),
                ..ModelRecord::default()
            },
        );
        let item = to_protocol_provider(
            "kimi",
            &ProviderConfig {
                provider_type: Some(ProviderType::from("kimi")),
                ..ProviderConfig::default()
            },
            &models,
            Some("last"),
            ProviderCredentialState {
                has_api_key: false,
                has_oauth_token: true,
            },
        );
        assert_eq!(item.default_model.as_deref(), Some("last"));
        assert_eq!(item.status, ProviderCatalogStatus::Connected);
        assert_eq!(item.models, Some(vec!["first".into(), "last".into()]));
        assert_eq!(
            serde_json::to_value(item).unwrap(),
            serde_json::json!({
                "id": "kimi", "type": "kimi", "default_model": "last",
                "has_api_key": false, "status": "connected", "models": ["first", "last"]
            })
        );
    }

    #[test]
    fn model_projection_uses_materialized_scalars_but_config_capabilities() {
        let model = Model {
            id: "configured".into(),
            name: "wire-name".into(),
            aliases: vec![],
            protocol: Protocol::OpenAi,
            base_url: None,
            headers: IndexMap::new(),
            capabilities: UNKNOWN_CAPABILITY,
            max_context_size: 100,
            max_output_size: None,
            display_name: None,
            reasoning_key: None,
            reasoning_history: None,
            support_efforts: Some(vec!["low".into(), "high".into()]),
            default_effort: Some("high".into()),
            always_thinking: false,
            provider_type: Some(ProviderType::from("openai")),
            provider_name: "provider-a".into(),
            auth_provider: Arc::new(StaticAuthProvider::default()),
            provider_options: None,
        };
        let record = ModelRecord {
            capabilities: Some(vec!["thinking".into()]),
            ..ModelRecord::default()
        };
        let item = to_protocol_model(&model, &record, None).unwrap();
        assert_eq!(item.provider, "provider-a");
        assert_eq!(item.display_name.as_deref(), Some("wire-name"));
        assert_eq!(item.capabilities, Some(vec!["thinking".into()]));
        assert_eq!(
            item.support_efforts,
            Some(vec!["low".into(), "high".into()])
        );
    }

    #[test]
    fn resolution_helpers_preserve_capability_header_and_url_rules() {
        let capability = resolve_model_capabilities(
            Some(&[" ALWAYS_THINKING ".into(), "tool_use".into()]),
            &ModelCapability {
                image_in: true,
                dynamically_loaded_tools: Some(true),
                ..UNKNOWN_CAPABILITY
            },
            128_000,
        );
        assert!(capability.image_in);
        assert!(capability.thinking);
        assert!(capability.tool_use);
        assert_eq!(capability.dynamically_loaded_tools, Some(true));
        assert_eq!(capability.max_context_tokens, 128_000);

        assert_eq!(
            strip_trailing_v1("https://api.example.test/v1"),
            "https://api.example.test"
        );
        assert_eq!(
            strip_trailing_v1("https://api.example.test/v1/"),
            "https://api.example.test"
        );
        assert_eq!(
            strip_trailing_v1("https://api.example.test/v10"),
            "https://api.example.test/v10"
        );

        let headers = resolve_outbound_headers(
            Some("unregistered"),
            Some(&IndexMap::from([("X-Custom".into(), "yes".into())])),
            &IndexMap::from([
                ("User-Agent".into(), "kimi-test".into()),
                ("X-Device".into(), "not-forwarded".into()),
            ]),
        )
        .unwrap();
        assert_eq!(headers["User-Agent"], "kimi-test");
        assert_eq!(headers["X-Custom"], "yes");
        assert!(!headers.contains_key("X-Device"));
    }

    #[test]
    fn protocol_options_and_vertex_location_keep_source_precedence() {
        let openai = build_protocol_provider_options(
            &ModelRecord {
                reasoning_key: Some(" reasoning_content ".into()),
                reasoning_history: Some(ReasoningHistoryMode::Required),
                ..ModelRecord::default()
            },
            Protocol::OpenAi,
            None,
            None,
        )
        .unwrap();
        assert_eq!(openai.reasoning_key.as_deref(), Some("reasoning_content"));
        assert_eq!(
            openai.reasoning_history,
            Some(ReasoningHistoryMode::Required)
        );

        let anthropic = build_protocol_provider_options(
            &ModelRecord {
                max_output_size: NonZeroU64::new(8192),
                support_efforts: Some(vec!["low".into(), "high".into()]),
                adaptive_thinking: Some(false),
                beta_api: Some(true),
                ..ModelRecord::default()
            },
            Protocol::Anthropic,
            None,
            None,
        )
        .unwrap();
        assert_eq!(anthropic.default_max_tokens, Some(8192));
        assert_eq!(anthropic.beta_api, Some(true));

        let vertex = build_protocol_provider_options(
            &ModelRecord::default(),
            Protocol::GoogleGenAi,
            Some(&ProviderConfig {
                env: Some(IndexMap::from([(
                    "GOOGLE_CLOUD_PROJECT".into(),
                    "project-a".into(),
                )])),
                ..ProviderConfig::default()
            }),
            Some("https://us-central1-aiplatform.googleapis.com/v1"),
        )
        .unwrap();
        assert_eq!(vertex.vertexai, Some(true));
        assert_eq!(vertex.project.as_deref(), Some("project-a"));
        assert_eq!(vertex.location.as_deref(), Some("us-central1"));
        assert_eq!(
            location_from_vertex_ai_base_url(Some("https://wrong.example.test/v1")),
            None
        );
        assert_eq!(
            build_protocol_provider_options(
                &ModelRecord::default(),
                Protocol::OpenAiResponses,
                None,
                None
            ),
            None
        );
    }

    #[test]
    fn configured_api_key_uses_trimmed_inline_value_before_registry_env() {
        assert!(
            has_configured_api_key(&ProviderConfig {
                api_key: Some("  present  ".into()),
                ..ProviderConfig::default()
            })
            .unwrap()
        );
        assert!(
            !has_configured_api_key(&ProviderConfig {
                api_key: Some(" \t ".into()),
                ..ProviderConfig::default()
            })
            .unwrap()
        );
    }
}
