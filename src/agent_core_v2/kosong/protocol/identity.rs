use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

use crate::agent_core_v2::kosong::contract::capability::ModelCapability;
use crate::agent_core_v2::kosong::contract::inspection::InspectionSource;

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

// Original: protocol.ts, ProtocolProviderOptions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolProviderOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_max_tokens: Option<f64>,
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

// MIGRATION-TODO:
// Original: protocol.ts, IProtocolAdapterRegistry
// Missing dependency: protocolBase.ts and protocolTrait.ts have not yet been
// migrated, so their ResolvedAdapterIdentity/ResolvedTrait types do not exist.
// Temporary behavior: none; no registry implementation is exposed here.
// Completion condition: migrate those L1 contracts, then add the Rust
// IProtocolAdapterRegistry trait with the same six resolution/construction
// methods. The TypeScript DI decorator will map to the Rust service container
// when that application-level dependency-injection unit is migrated.

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
                default_max_tokens: Some(8192.0),
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
                    "defaultMaxTokens": 8192.0,
                    "supportEfforts": ["low", "high"],
                    "vertexai": true,
                    "project": "project-1",
                    "location": "us-central1"
                }
            })
        );
    }
}
