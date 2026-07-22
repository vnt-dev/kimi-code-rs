use serde::{Deserialize, Serialize};

use crate::agent_core_v2::kosong::contract::capability::ModelCapability;

// Original:
//   packages/agent-core-v2/src/kosong/model/model.types.ts
//   CompletionBudgetConfig
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompletionBudgetConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hard_cap: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<f64>,
}

// Original:
//   packages/agent-core-v2/src/kosong/model/model.types.ts
//   CompletionBudgetParams
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionBudgetParams {
    pub max_completion_tokens: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_context_tokens: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
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

// MIGRATION-TODO:
// Original: packages/agent-core-v2/src/kosong/model/model.types.ts
// Missing units: ModelOverrides and ResolvedModelAuthMaterial.
// Temporary behavior: those public data types are not exported yet.
// Completion condition: migrate their provider/auth dependencies and add the
// remaining structures to this module.

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
}
