use serde::{Deserialize, Serialize};

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

// MIGRATION-TODO:
// Original: packages/agent-core-v2/src/kosong/model/model.types.ts
// Missing units: ModelOverrides, ResolvedModelAuthMaterial,
// ThinkingDefaults, and ModelThinkingMetadata.
// Temporary behavior: those public data types are not exported yet.
// Completion condition: migrate their provider/auth dependencies and add the
// remaining structures to this module.
