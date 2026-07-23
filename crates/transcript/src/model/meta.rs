//! Session and agent metadata above the transcript timeline.
//!
//! Original: `packages/transcript/src/model/meta.ts`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalMeta {
    pub objective: String,
    pub status: GoalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limit: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanMode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmMode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModesMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm: Option<SwarmMode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModesMetaMerge {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub plan: Option<Option<PlanMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub swarm: Option<Option<SwarmMode>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityMeta {
    Idle,
    Turn,
    Disposing,
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<GoalMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<ModesMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<ActivityMeta>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptMetaMerge {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<GoalMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<ModesMetaMerge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<ActivityMeta>,
}
