use serde::{Deserialize, Serialize};

use super::events::SkillSource;
use super::validation::non_empty;

// Original: skill.ts, skillDescriptorSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDescriptor {
    #[serde(deserialize_with = "non_empty")]
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: SkillSource,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub skill_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_model_invocation: Option<bool>,
}
