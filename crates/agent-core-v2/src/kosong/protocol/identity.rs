use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, ops::Deref, str::FromStr, sync::Arc};

use crate::_base::di::instantiation::ServiceIdentifier;
use crate::kosong::contract::capability::ModelCapability;
use crate::kosong::contract::inspection::InspectionSource;
use crate::kosong::contract::provider::{ChatProvider, ProviderError};

use super::protocol_base::{ProtocolBaseId, ResolvedAdapterIdentity};

// Original:
//   packages/agent-core-v2/src/kosong/protocol/protocol.ts
//   ProtocolSchema / Protocol
//
// Rust adaptation:
//   Zod runtime parsing becomes FromStr/TryFrom plus Serde validation. The
//   enum remains closed over the four real wire formats; vendor identities
//   continue to live in ProtocolAdapterConfig::provider_type as free strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    #[serde(rename = "google-genai")]
    GoogleGenAi,
}

impl Protocol {
    pub const ALL: [Self; 4] = [
        Self::Anthropic,
        Self::OpenAi,
        Self::OpenAiResponses,
        Self::GoogleGenAi,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenAiResponses => "openai_responses",
            Self::GoogleGenAi => "google-genai",
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Protocol {
    type Err = ProtocolParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "openai_responses" => Ok(Self::OpenAiResponses),
            "google-genai" => Ok(Self::GoogleGenAi),
            _ => Err(ProtocolParseError {
                value: value.to_owned(),
            }),
        }
    }
}

impl TryFrom<&str> for Protocol {
    type Error = ProtocolParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolParseError {
    value: String,
}

impl ProtocolParseError {
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for ProtocolParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown protocol '{}'", self.value)
    }
}

impl Error for ProtocolParseError {}

/// Controls how assistant reasoning is replayed to OpenAI-compatible chat APIs.
///
/// Some compatible providers require the reasoning field on every historical
/// assistant message, including messages whose reasoning payload is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningHistoryMode {
    Disabled,
    WhenPresent,
    #[default]
    Auto,
    Required,
}

// Original: protocol.ts, ProtocolProviderOptions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolProviderOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_history: Option<ReasoningHistoryMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beta_api: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertexai: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

// Original: protocol.ts, ProtocolAdapterConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolAdapterConfig {
    pub protocol: Protocol,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub model_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_headers: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProtocolProviderOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplainedCapability {
    pub capability: ModelCapability,
    pub source: InspectionSource,
}

// Original: protocol.ts, IProtocolAdapterRegistry
//
// Rust adaptation:
//   TypeScript factory exceptions become ProviderError results. The trait is
//   object-safe so the application service container can bind the eventual L2
//   implementation without exposing vendor definitions to this L1 contract.
pub trait ProtocolAdapterRegistry: Send + Sync {
    fn supported_protocols(&self) -> Vec<Protocol>;

    fn resolve_adapter_identity(
        &self,
        protocol: Protocol,
        provider_type: Option<&str>,
    ) -> ResolvedAdapterIdentity;

    fn resolve_provider_base_id(
        &self,
        protocol: Protocol,
        provider_type: Option<&str>,
    ) -> ProtocolBaseId;

    fn resolve_capability(
        &self,
        protocol: Protocol,
        model_name: &str,
        provider_type: Option<&str>,
    ) -> ModelCapability;

    fn explain_capability(
        &self,
        protocol: Protocol,
        model_name: &str,
        provider_type: Option<&str>,
    ) -> ExplainedCapability;

    fn create_chat_provider(
        &self,
        config: ProtocolAdapterConfig,
    ) -> Result<std::sync::Arc<dyn ChatProvider>, ProviderError>;
}

#[derive(Clone)]
pub struct ProtocolAdapterRegistryHandle(pub Arc<dyn ProtocolAdapterRegistry>);

impl Deref for ProtocolAdapterRegistryHandle {
    type Target = dyn ProtocolAdapterRegistry;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

// Original: protocol.ts, IProtocolAdapterRegistry service identifier.
pub const PROTOCOL_ADAPTER_REGISTRY_SERVICE_ID: ServiceIdentifier<ProtocolAdapterRegistryHandle> =
    ServiceIdentifier::new("protocolAdapterRegistry");

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_exactly_the_four_wire_protocols() {
        for protocol in Protocol::ALL {
            assert_eq!(protocol.as_str().parse::<Protocol>().unwrap(), protocol);
            assert_eq!(
                serde_json::to_string(&protocol).unwrap(),
                format!("\"{}\"", protocol.as_str())
            );
        }

        for rejected in ["kimi", "vertexai", "azure", "", "OpenAI"] {
            let error = rejected.parse::<Protocol>().unwrap_err();
            assert_eq!(error.value(), rejected);
            assert!(serde_json::from_value::<Protocol>(json!(rejected)).is_err());
        }
        assert!(serde_json::from_value::<Protocol>(json!(42)).is_err());
    }

    #[test]
    fn adapter_config_keeps_provider_type_free_form_and_optional() {
        let config: ProtocolAdapterConfig = serde_json::from_value(json!({
            "protocol": "openai",
            "providerType": "vendor-registered-elsewhere",
            "modelName": "vendor-model-1"
        }))
        .unwrap();
        assert_eq!(
            config.provider_type.as_deref(),
            Some("vendor-registered-elsewhere")
        );

        let without_vendor: ProtocolAdapterConfig = serde_json::from_value(json!({
            "protocol": "anthropic",
            "modelName": "claude-sonnet-4"
        }))
        .unwrap();
        assert_eq!(without_vendor.provider_type, None);
    }

    #[test]
    fn provider_options_preserve_camel_case_wire_fields_and_vertex_mode() {
        let config = ProtocolAdapterConfig {
            protocol: Protocol::GoogleGenAi,
            provider_type: Some("google-vertex".to_owned()),
            base_url: None,
            model_name: "gemini-2.5-pro".to_owned(),
            api_key: None,
            default_headers: Some(IndexMap::from([(
                "x-client".to_owned(),
                "kimi-code".to_owned(),
            )])),
            provider_options: Some(ProtocolProviderOptions {
                default_max_tokens: Some(8192),
                support_efforts: Some(vec!["low".to_owned(), "high".to_owned()]),
                vertexai: Some(true),
                project: Some("project-1".to_owned()),
                location: Some("us-central1".to_owned()),
                ..ProtocolProviderOptions::default()
            }),
        };

        assert_eq!(
            serde_json::to_value(config).unwrap(),
            json!({
                "protocol": "google-genai",
                "providerType": "google-vertex",
                "modelName": "gemini-2.5-pro",
                "defaultHeaders": {"x-client": "kimi-code"},
                "providerOptions": {
                    "defaultMaxTokens": 8192,
                    "supportEfforts": ["low", "high"],
                    "vertexai": true,
                    "project": "project-1",
                    "location": "us-central1"
                }
            })
        );
    }

    #[test]
    fn reasoning_history_mode_uses_stable_snake_case_values() {
        let options: ProtocolProviderOptions = serde_json::from_value(json!({
            "reasoningKey": "reasoning_content",
            "reasoningHistory": "required"
        }))
        .unwrap();
        assert_eq!(
            options.reasoning_history,
            Some(ReasoningHistoryMode::Required)
        );
        assert_eq!(
            serde_json::to_value(options).unwrap(),
            json!({
                "reasoningKey": "reasoning_content",
                "reasoningHistory": "required"
            })
        );
        assert!(serde_json::from_value::<ReasoningHistoryMode>(json!("sometimes")).is_err());
    }

    #[test]
    fn adapter_registry_keeps_the_established_service_identity() {
        assert_eq!(
            PROTOCOL_ADAPTER_REGISTRY_SERVICE_ID.to_string(),
            "protocolAdapterRegistry"
        );
    }
}
