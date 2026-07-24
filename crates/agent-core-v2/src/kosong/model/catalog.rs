//! Pure model-catalog data, auth, and projection helpers.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/catalog.ts`.
//!
//! The service contract and cache-owning implementation are migrated with the
//! requester and inspection types they require. This module owns the source
//! file's independent data and projection methods.

use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::kosong::{
    contract::{capability::ModelCapability, provider::ProviderRequestAuth, usage::TokenUsage},
    protocol::identity::{Protocol, ProtocolProviderOptions},
    provider::config::{ProviderConfig, ProviderType},
};

use super::{
    contract::{ModelRecord, ModelsSection},
    model_auth::effective_model_config,
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
    pub duration_ms: f64,
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
}
