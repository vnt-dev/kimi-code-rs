//! Model configuration record and registry contract.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/model.ts`.

use std::{error::Error, num::NonZeroU64, ops::Deref, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    _base::{di::instantiation::ServiceIdentifier, event::Event},
    kosong::{protocol::identity::Protocol, provider::config::OAuthRef},
};

pub const MODELS_SECTION: &str = "models";
pub const DEFAULT_MODEL_SECTION: &str = "defaultModel";

// Original: ModelOverrideSchema. Authentication, routing identity, aliases,
// protocol, and betaApi are intentionally unavailable in per-model overrides.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRecordOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_size: Option<NonZeroU64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_size: Option<NonZeroU64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

// Original: ModelRecordSchema. `extra` preserves Zod `.passthrough()` fields.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_size: Option<NonZeroU64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_size: Option<NonZeroU64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beta_api: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overrides: Option<ModelRecordOverride>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub type ModelsSection = IndexMap<String, ModelRecord>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModelsChangedEvent {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

pub type ModelServiceError = Box<dyn Error + Send + Sync>;
pub type ModelServiceResult<T> = Result<T, ModelServiceError>;

#[async_trait]
pub trait ModelServiceContract: Send + Sync {
    fn on_did_change_models(&self) -> Event<ModelsChangedEvent>;
    fn get(&self, id: &str) -> Option<ModelRecord>;
    fn list(&self) -> ModelsSection;
    async fn set(&self, id: &str, model: ModelRecord) -> ModelServiceResult<()>;
    async fn delete(&self, id: &str) -> ModelServiceResult<()>;
}

#[derive(Clone)]
pub struct ModelServiceHandle(pub Arc<dyn ModelServiceContract>);

impl Deref for ModelServiceHandle {
    type Target = dyn ModelServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const MODEL_SERVICE_ID: ServiceIdentifier<ModelServiceHandle> =
    ServiceIdentifier::new("modelService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_known_override_and_passthrough_fields() {
        let value = serde_json::json!({
            "providerId": "shared",
            "baseUrl": "https://api.example.test/v1",
            "apiKey": "secret",
            "oauth": {"storage": "keyring", "key": "account"},
            "protocol": "anthropic",
            "name": "claude-sonnet",
            "aliases": ["sonnet"],
            "provider": "legacy-provider",
            "model": "legacy-model",
            "maxContextSize": 200000,
            "maxOutputSize": 8192,
            "capabilities": ["thinking"],
            "displayName": "Sonnet",
            "reasoningKey": "reasoning",
            "adaptiveThinking": true,
            "betaApi": false,
            "supportEfforts": ["low", "high"],
            "defaultEffort": "high",
            "overrides": {
                "maxOutputSize": 4096,
                "supportEfforts": ["low"]
            },
            "vendorExtension": {"enabled": true}
        });
        let record: ModelRecord = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(record.protocol, Some(Protocol::Anthropic));
        assert_eq!(record.max_context_size.unwrap().get(), 200_000);
        assert_eq!(
            record
                .overrides
                .as_ref()
                .unwrap()
                .max_output_size
                .unwrap()
                .get(),
            4096
        );
        assert_eq!(record.extra["vendorExtension"]["enabled"], true);
        assert_eq!(serde_json::to_value(record).unwrap(), value);
    }

    #[test]
    fn record_rejects_non_positive_fractional_and_unknown_protocol_sizes() {
        for invalid in [
            serde_json::json!({"maxContextSize": 0}),
            serde_json::json!({"maxOutputSize": -1}),
            serde_json::json!({"maxOutputSize": 1.5}),
            serde_json::json!({"overrides": {"maxContextSize": 0}}),
            serde_json::json!({"protocol": "kimi"}),
        ] {
            assert!(serde_json::from_value::<ModelRecord>(invalid).is_err());
        }
    }

    #[test]
    fn service_identity_and_empty_sections_match_source_contract() {
        assert_eq!(MODELS_SECTION, "models");
        assert_eq!(DEFAULT_MODEL_SECTION, "defaultModel");
        assert_eq!(MODEL_SERVICE_ID.to_string(), "modelService");
        assert!(ModelsSection::new().is_empty());
    }
}
