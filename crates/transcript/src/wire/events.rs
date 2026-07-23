//! Transcript-owned WebSocket event payloads.
//!
//! Original:
//!   `packages/transcript/src/wire/events.ts`

use serde::{Deserialize, Serialize};

use crate::model::AgentId;
use crate::ops::{AgentTranscriptSnapshot, TranscriptOperation};

use super::schema::{TranscriptOpsPayload, TranscriptResetPayload, WireError, WireValidate};

pub const TRANSCRIPT_EVENT_TYPES: [&str; 2] = ["transcript.reset", "transcript.ops"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TranscriptEventType {
    #[serde(rename = "transcript.reset")]
    Reset,
    #[serde(rename = "transcript.ops")]
    Ops,
}

impl TranscriptEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reset => "transcript.reset",
            Self::Ops => "transcript.ops",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TranscriptEvent {
    #[serde(rename = "transcript.reset")]
    Reset {
        agent_id: AgentId,
        snapshot: Box<AgentTranscriptSnapshot>,
        has_more_older: bool,
    },
    #[serde(rename = "transcript.ops")]
    Ops {
        agent_id: AgentId,
        ops: Vec<TranscriptOperation>,
    },
}

impl TranscriptEvent {
    pub const fn event_type(&self) -> TranscriptEventType {
        match self {
            Self::Reset { .. } => TranscriptEventType::Reset,
            Self::Ops { .. } => TranscriptEventType::Ops,
        }
    }
}

impl WireValidate for TranscriptEvent {
    fn validate_wire(&self) -> Result<(), WireError> {
        match self {
            Self::Reset {
                agent_id,
                snapshot,
                has_more_older,
            } => TranscriptResetPayload {
                agent_id: agent_id.clone(),
                snapshot: snapshot.as_ref().clone(),
                has_more_older: *has_more_older,
            }
            .validate_wire(),
            Self::Ops { agent_id, ops } => TranscriptOpsPayload {
                agent_id: agent_id.clone(),
                ops: ops.clone(),
            }
            .validate_wire(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::wire::parse_wire_value;

    #[test]
    fn round_trips_both_dotted_event_discriminants() {
        let cases = [
            json!({
                "type": "transcript.reset",
                "agent_id": "main",
                "snapshot": {"items": [], "tasks": [], "meta": {}},
                "has_more_older": true
            }),
            json!({
                "type": "transcript.ops",
                "agent_id": "main",
                "ops": [{"op": "items.remove", "ids": ["t1"]}]
            }),
        ];
        for expected in cases {
            let event: TranscriptEvent = parse_wire_value(expected.clone()).unwrap();
            let mut serialized = serde_json::to_value(event).unwrap();
            if expected["type"] == "transcript.reset" {
                let snapshot = serialized["snapshot"].as_object_mut().unwrap();
                snapshot.remove("interactions");
                snapshot.remove("attachments");
                snapshot.remove("todos");
            }
            assert_eq!(serialized, expected);
        }
        assert_eq!(
            TranscriptEventType::Reset.as_str(),
            TRANSCRIPT_EVENT_TYPES[0]
        );
    }
}
