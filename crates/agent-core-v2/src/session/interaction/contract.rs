//! Interaction request and response contracts.
//!
//! Original: `session/interaction/interaction.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    Approval,
    Question,
    UserTool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionOrigin {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: InteractionKind,
    pub payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<InteractionOrigin>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interaction {
    pub id: String,
    pub kind: InteractionKind,
    pub payload: Value,
    pub origin: InteractionOrigin,
    pub created_at: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InteractionResolution {
    pub id: String,
    pub response: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InteractionPendingChangedEvent {
    pub pending: Vec<String>,
}
