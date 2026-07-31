use serde::{Deserialize, Serialize};

use crate::kosong::{contract::capability::ModelCapability, provider::config::OAuthRef};

// Original:
//   packages/agent-core-v2/src/kosong/model/model.types.ts
//   ModelOverrides
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_keep: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u64>,
}

// Original:
//   packages/agent-core-v2/src/kosong/model/model.types.ts
//   CompletionBudgetConfig
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompletionBudgetConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hard_cap: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<u64>,
}

// Original:
//   packages/agent-core-v2/src/kosong/model/model.types.ts
//   CompletionBudgetParams
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionBudgetParams {
    pub max_completion_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_context_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
}

// Original:
//   packages/agent-core-v2/src/kosong/model/model.types.ts
//   ResolvedModelAuthMaterial
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedModelAuthMaterial {
    pub api_key: Option<String>,
    pub oauth: Option<OAuthRef>,
    pub oauth_provider_key: Option<String>,
}

// Original:
//   packages/agent-core-v2/src/kosong/model/model.types.ts
//   ThinkingDefaults
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

// Original TypeScript accepts either the structured ModelCapability object or
// its older string-list representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelThinkingCapabilities {
    Structured(ModelCapability),
    Names(Vec<String>),
}

// Original:
//   packages/agent-core-v2/src/kosong/model/model.types.ts
//   ModelThinkingMetadata
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelThinkingMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelThinkingCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub always_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_metadata_preserves_camel_case_and_legacy_capability_list() {
        let metadata = ModelThinkingMetadata {
            capabilities: Some(ModelThinkingCapabilities::Names(vec![
                "thinking".to_owned(),
            ])),
            adaptive_thinking: Some(true),
            always_thinking: Some(false),
            support_efforts: Some(vec!["low".to_owned(), "high".to_owned()]),
            default_effort: Some("high".to_owned()),
        };
        assert_eq!(
            serde_json::to_value(metadata).unwrap(),
            serde_json::json!({
                "capabilities": ["thinking"],
                "adaptiveThinking": true,
                "alwaysThinking": false,
                "supportEfforts": ["low", "high"],
                "defaultEffort": "high",
            })
        );
    }

    #[test]
    fn model_overrides_round_trip_camel_case_config_fields() {
        let value = serde_json::json!({
            "temperature": 0.25,
            "topP": 0.9,
            "thinkingKeep": "all",
            "maxCompletionTokens": 4096,
        });
        let overrides: ModelOverrides = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(overrides.temperature, Some(0.25));
        assert_eq!(overrides.top_p, Some(0.9));
        assert_eq!(overrides.thinking_keep.as_deref(), Some("all"));
        assert_eq!(overrides.max_completion_tokens, Some(4096));
        assert_eq!(serde_json::to_value(overrides).unwrap(), value);
        assert_eq!(
            serde_json::to_value(ModelOverrides::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn resolved_auth_material_keeps_oauth_provider_identity_distinct() {
        let material = ResolvedModelAuthMaterial {
            api_key: Some("secret".into()),
            oauth: Some(OAuthRef {
                storage: crate::kosong::provider::config::OAuthStorage::Keyring,
                key: "account".into(),
                oauth_host: Some("oauth.example.test".into()),
            }),
            oauth_provider_key: Some("anthropic".into()),
        };
        assert_eq!(material.api_key.as_deref(), Some("secret"));
        assert_eq!(material.oauth.as_ref().unwrap().key, "account");
        assert_eq!(material.oauth_provider_key.as_deref(), Some("anthropic"));
    }
}
