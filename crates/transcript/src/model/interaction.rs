//! Session-global approval and question entities.
//!
//! Original: `packages/transcript/src/model/interaction.ts`.

use serde::{Deserialize, Serialize};

use super::{InteractionId, OptionalJsonValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    Approval,
    Question,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionState {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    Answered,
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptInteraction {
    pub interaction_id: InteractionId,
    pub interaction_kind: InteractionKind,
    pub tool_call_id: String,
    pub state: InteractionState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub request: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub response: OptionalJsonValue,
}
