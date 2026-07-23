use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_summary: Option<String>,
    pub compacted_count: f64,
    pub tokens_before: f64,
    pub tokens_after: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_user_message_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_head_user_message_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_count: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionSource {
    Manual,
    Auto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactionBeginData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    pub source: CompactionSource,
}
