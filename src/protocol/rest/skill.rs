use serde::{Deserialize, Serialize};

use crate::protocol::SkillDescriptor;
use crate::protocol::validation::{literal_true, non_empty};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSkillsResponse {
    pub skills: Vec<SkillDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ActivateSkillRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateSkillResult {
    #[serde(deserialize_with = "literal_true")]
    pub activated: bool,
    #[serde(deserialize_with = "non_empty")]
    pub skill_name: String,
}
