//! Turn and step containers.
//!
//! Original: `packages/transcript/src/model/turn.ts`.

use serde::{Deserialize, Serialize};

use super::{AttachmentId, OptionalJsonValue, StepId, TaskId, TranscriptFrame, TurnId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TurnOrigin {
    #[serde(rename = "user")]
    User {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::serde_utils::double_option"
        )]
        payload: OptionalJsonValue,
    },
    #[serde(rename = "cron", rename_all = "camelCase")]
    Cron {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<TaskId>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::serde_utils::double_option"
        )]
        payload: OptionalJsonValue,
    },
    #[serde(rename = "task", rename_all = "camelCase")]
    Task {
        task_id: TaskId,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::serde_utils::double_option"
        )]
        payload: OptionalJsonValue,
    },
    #[serde(rename = "hook")]
    Hook {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::serde_utils::double_option"
        )]
        payload: OptionalJsonValue,
    },
    #[serde(rename = "compaction")]
    Compaction {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::serde_utils::double_option"
        )]
        payload: OptionalJsonValue,
    },
    #[serde(rename = "side")]
    Side {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::serde_utils::double_option"
        )]
        payload: OptionalJsonValue,
    },
    #[serde(rename = "other")]
    Other {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::serde_utils::double_option"
        )]
        payload: OptionalJsonValue,
    },
}

impl TurnOrigin {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::User { .. } => "user",
            Self::Cron { .. } => "cron",
            Self::Task { .. } => "task",
            Self::Hook { .. } => "hook",
            Self::Compaction { .. } => "compaction",
            Self::Side { .. } => "side",
            Self::Other { .. } => "other",
        }
    }

    pub fn other() -> Self {
        Self::Other { payload: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Running,
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptTurn {
    pub turn_id: TurnId,
    pub ordinal: i64,
    pub state: TurnState,
    pub origin: TurnOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ids: Option<Vec<AttachmentId>>,
    pub steps: Vec<TranscriptStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TranscriptUsage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptStep {
    pub step_id: StepId,
    pub turn_id: TurnId,
    pub ordinal: i64,
    pub state: StepState,
    pub frames: Vec<TranscriptFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}
