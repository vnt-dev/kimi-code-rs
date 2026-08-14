use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_summary: Option<String>,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub compacted_count: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub tokens_before: u64,
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u64")]
    pub tokens_after: u64,
    #[serde(
        default,
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub kept_user_message_count: Option<u64>,
    #[serde(
        default,
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub kept_head_user_message_count: Option<u64>,
    #[serde(
        default,
        deserialize_with = "kimi_code_protocol::lenient::lenient_optional_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub dropped_count: Option<u64>,
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
