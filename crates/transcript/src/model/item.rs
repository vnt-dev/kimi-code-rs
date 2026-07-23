//! Top-level timeline item union.
//!
//! Original: `packages/transcript/src/model/item.ts`.

use serde::{Deserialize, Serialize};

use super::{MarkerId, OptionalJsonValue, TaskId, TaskRefId, TranscriptTurn};

pub type MarkerKey = String;

pub const KNOWN_MARKERS: [&str; 11] = [
    "compaction",
    "undo",
    "clear",
    "goal",
    "plan.enter",
    "plan.exit",
    "swarm.enter",
    "swarm.exit",
    "skill",
    "cron.fired",
    "notice",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMarker {
    pub marker_id: MarkerId,
    pub marker: MarkerKey,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub payload: OptionalJsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptTaskRef {
    pub ref_id: TaskRefId,
    pub task_id: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TranscriptItem {
    #[serde(rename = "turn")]
    Turn(TranscriptTurn),
    #[serde(rename = "marker")]
    Marker(TranscriptMarker),
    #[serde(rename = "taskref")]
    TaskRef(TranscriptTaskRef),
}

pub fn item_id(item: &TranscriptItem) -> &str {
    match item {
        TranscriptItem::Turn(turn) => turn.turn_id.as_ref(),
        TranscriptItem::Marker(marker) => marker.marker_id.as_ref(),
        TranscriptItem::TaskRef(task_ref) => task_ref.ref_id.as_ref(),
    }
}
