//! L2 transport vocabulary.
//!
//! Original:
//!   `packages/transcript/src/ops/operation.ts`

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::model::{
    AgentId, AttachmentId, FrameId, StepId, StepState, TaskId, TranscriptAttachment,
    TranscriptFrame, TranscriptInteraction, TranscriptItem, TranscriptMarker, TranscriptMeta,
    TranscriptMetaMerge, TranscriptStep, TranscriptTask, TranscriptTaskRef, TranscriptTodo,
    TranscriptTurn, TranscriptUsage, TurnId, TurnOrigin, TurnState,
};

/// Turn header as carried on the wire: steps arrive through `step.upsert`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnHeader {
    pub turn_id: TurnId,
    pub ordinal: i64,
    pub state: TurnState,
    pub origin: TurnOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ids: Option<Vec<AttachmentId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TranscriptUsage>,
}

impl From<&TranscriptTurn> for TurnHeader {
    fn from(turn: &TranscriptTurn) -> Self {
        Self {
            turn_id: turn.turn_id.clone(),
            ordinal: turn.ordinal,
            state: turn.state,
            origin: turn.origin.clone(),
            prompt: turn.prompt.clone(),
            attachment_ids: turn.attachment_ids.clone(),
            started_at: turn.started_at.clone(),
            ended_at: turn.ended_at.clone(),
            usage: turn.usage.clone(),
        }
    }
}

impl TurnHeader {
    pub fn into_turn(self, steps: Vec<TranscriptStep>) -> TranscriptTurn {
        TranscriptTurn {
            turn_id: self.turn_id,
            ordinal: self.ordinal,
            state: self.state,
            origin: self.origin,
            prompt: self.prompt,
            attachment_ids: self.attachment_ids,
            steps,
            started_at: self.started_at,
            ended_at: self.ended_at,
            usage: self.usage,
        }
    }
}

/// Step header as carried on the wire: frames arrive through `frame.upsert`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepHeader {
    pub step_id: StepId,
    pub turn_id: TurnId,
    pub ordinal: i64,
    pub state: StepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

impl From<&TranscriptStep> for StepHeader {
    fn from(step: &TranscriptStep) -> Self {
        Self {
            step_id: step.step_id.clone(),
            turn_id: step.turn_id.clone(),
            ordinal: step.ordinal,
            state: step.state,
            started_at: step.started_at.clone(),
            ended_at: step.ended_at.clone(),
        }
    }
}

impl StepHeader {
    pub fn into_step(self, frames: Vec<TranscriptFrame>) -> TranscriptStep {
        TranscriptStep {
            step_id: self.step_id,
            turn_id: self.turn_id,
            ordinal: self.ordinal,
            state: self.state,
            frames,
            started_at: self.started_at,
            ended_at: self.ended_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppendTarget {
    Frame {
        #[serde(rename = "turnId")]
        turn_id: TurnId,
        #[serde(rename = "stepId")]
        step_id: StepId,
        #[serde(rename = "frameId")]
        frame_id: FrameId,
    },
    Task {
        #[serde(rename = "taskId")]
        task_id: TaskId,
    },
}

/// All operations accepted by an `AgentTranscript`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum TranscriptOperation {
    #[serde(rename = "reset", rename_all = "camelCase")]
    Reset {
        agent_id: AgentId,
        snapshot: AgentTranscriptSnapshot,
    },
    #[serde(rename = "turn.upsert")]
    TurnUpsert {
        #[serde(with = "turn_header_wire")]
        turn: TurnHeader,
    },
    #[serde(rename = "step.upsert", rename_all = "camelCase")]
    StepUpsert {
        turn_id: TurnId,
        #[serde(with = "step_header_wire")]
        step: StepHeader,
    },
    #[serde(rename = "frame.upsert", rename_all = "camelCase")]
    FrameUpsert {
        turn_id: TurnId,
        step_id: StepId,
        frame: TranscriptFrame,
    },
    #[serde(rename = "append")]
    Append {
        target: AppendTarget,
        offset: u64,
        text: String,
    },
    #[serde(rename = "marker.upsert", rename_all = "camelCase")]
    MarkerUpsert {
        #[serde(with = "marker_wire")]
        item: TranscriptMarker,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_turn: Option<i64>,
    },
    #[serde(rename = "taskref.upsert", rename_all = "camelCase")]
    TaskRefUpsert {
        #[serde(with = "task_ref_wire")]
        item: TranscriptTaskRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_turn: Option<i64>,
    },
    #[serde(rename = "task.upsert")]
    TaskUpsert { task: TranscriptTask },
    #[serde(rename = "interaction.upsert")]
    InteractionUpsert { interaction: TranscriptInteraction },
    #[serde(rename = "attachment.upsert")]
    AttachmentUpsert { attachment: TranscriptAttachment },
    #[serde(rename = "todo.upsert")]
    TodoUpsert { todo: TranscriptTodo },
    #[serde(rename = "meta.merge")]
    MetaMerge { meta: TranscriptMetaMerge },
    #[serde(rename = "items.remove")]
    ItemsRemove { ids: Vec<String> },
}

impl TranscriptOperation {
    pub fn op_name(&self) -> &'static str {
        match self {
            Self::Reset { .. } => "reset",
            Self::TurnUpsert { .. } => "turn.upsert",
            Self::StepUpsert { .. } => "step.upsert",
            Self::FrameUpsert { .. } => "frame.upsert",
            Self::Append { .. } => "append",
            Self::MarkerUpsert { .. } => "marker.upsert",
            Self::TaskRefUpsert { .. } => "taskref.upsert",
            Self::TaskUpsert { .. } => "task.upsert",
            Self::InteractionUpsert { .. } => "interaction.upsert",
            Self::AttachmentUpsert { .. } => "attachment.upsert",
            Self::TodoUpsert { .. } => "todo.upsert",
            Self::MetaMerge { .. } => "meta.merge",
            Self::ItemsRemove { .. } => "items.remove",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptOpBatch {
    pub agent_id: AgentId,
    pub ops: Vec<TranscriptOperation>,
}

/// Full materialized state of one agent transcript, as used by `reset`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTranscriptSnapshot {
    pub items: Vec<TranscriptItem>,
    pub tasks: Vec<TranscriptTask>,
    #[serde(default)]
    pub interactions: Vec<TranscriptInteraction>,
    #[serde(default)]
    pub attachments: Vec<TranscriptAttachment>,
    #[serde(default)]
    pub todos: Vec<TranscriptTodo>,
    pub meta: TranscriptMeta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_more_older: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendGap {
    pub target: AppendTarget,
    pub expected: u64,
    pub got: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppliedOps {
    pub accepted: Vec<TranscriptOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<AppendGap>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptChangeEvent {
    pub agent_id: AgentId,
    pub ops: Vec<TranscriptOperation>,
}

macro_rules! literal_kind_wire {
    ($module:ident, $value:ty, $kind:literal) => {
        mod $module {
            use super::*;

            #[derive(Serialize)]
            struct WireRef<'a> {
                kind: &'static str,
                #[serde(flatten)]
                value: &'a $value,
            }

            #[derive(Deserialize)]
            struct WireOwned {
                kind: String,
                #[serde(flatten)]
                value: $value,
            }

            pub fn serialize<S>(value: &$value, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                WireRef { kind: $kind, value }.serialize(serializer)
            }

            pub fn deserialize<'de, D>(deserializer: D) -> Result<$value, D::Error>
            where
                D: Deserializer<'de>,
            {
                let wire = WireOwned::deserialize(deserializer)?;
                if wire.kind != $kind {
                    return Err(de::Error::custom(format_args!(
                        "expected kind `{}`, got `{}`",
                        $kind, wire.kind
                    )));
                }
                Ok(wire.value)
            }
        }
    };
}

literal_kind_wire!(turn_header_wire, TurnHeader, "turn");
literal_kind_wire!(step_header_wire, StepHeader, "step");
literal_kind_wire!(marker_wire, TranscriptMarker, "marker");
literal_kind_wire!(task_ref_wire, TranscriptTaskRef, "taskref");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn round_trips_every_operation_wire_kind() {
        let cases = [
            json!({
                "op": "reset",
                "agentId": "agent-1",
                "snapshot": {"items": [], "tasks": [], "meta": {}}
            }),
            json!({
                "op": "turn.upsert",
                "turn": {
                    "kind": "turn", "turnId": "t1", "ordinal": 1,
                    "state": "running", "origin": {"kind": "user"}
                }
            }),
            json!({
                "op": "step.upsert",
                "turnId": "t1",
                "step": {
                    "kind": "step", "stepId": "t1.0", "turnId": "t1",
                    "ordinal": 0, "state": "running"
                }
            }),
            json!({
                "op": "frame.upsert", "turnId": "t1", "stepId": "t1.0",
                "frame": {"kind": "thinking", "frameId": "f1", "text": "hmm"}
            }),
            json!({
                "op": "append", "offset": 3, "text": "lo",
                "target": {
                    "type": "frame", "turnId": "t1", "stepId": "t1.0", "frameId": "f1"
                }
            }),
            json!({
                "op": "marker.upsert", "beforeTurn": 2,
                "item": {"kind": "marker", "markerId": "m1", "marker": "clear"}
            }),
            json!({
                "op": "taskref.upsert",
                "item": {"kind": "taskref", "refId": "r1", "taskId": "task-1"}
            }),
            json!({
                "op": "task.upsert",
                "task": {
                    "taskId": "task-1", "kind": "shell", "state": "running",
                    "detached": false, "outputTail": ""
                }
            }),
            json!({
                "op": "interaction.upsert",
                "interaction": {
                    "interactionId": "i1", "interactionKind": "approval",
                    "toolCallId": "call-1", "state": "pending"
                }
            }),
            json!({
                "op": "attachment.upsert",
                "attachment": {"attachmentId": "a1", "mediaType": "image/png"}
            }),
            json!({
                "op": "todo.upsert",
                "todo": {"todoId": "todo-1", "items": [{"title": "Port", "status": "in_progress"}]}
            }),
            json!({"op": "meta.merge", "meta": {"modes": {"plan": null}}}),
            json!({"op": "items.remove", "ids": ["t1", "m1"]}),
        ];

        for expected in cases {
            let operation: TranscriptOperation =
                serde_json::from_value(expected.clone()).expect("operation should deserialize");
            assert_eq!(operation.op_name(), expected["op"]);

            let mut serialized =
                serde_json::to_value(operation).expect("operation should serialize");
            if expected["op"] == "reset" {
                let snapshot = serialized["snapshot"]
                    .as_object_mut()
                    .expect("snapshot should be an object");
                snapshot.remove("interactions");
                snapshot.remove("attachments");
                snapshot.remove("todos");
            }
            assert_eq!(serialized, expected);
        }
    }

    #[test]
    fn preserves_dotted_discriminants_camel_case_and_snapshot_defaults() {
        let wire = json!({
            "op": "reset",
            "agentId": "agent-1",
            "snapshot": {"items": [], "tasks": [], "meta": {}, "hasMoreOlder": true}
        });

        let operation: TranscriptOperation = serde_json::from_value(wire).unwrap();
        let TranscriptOperation::Reset { agent_id, snapshot } = operation else {
            panic!("expected reset");
        };
        assert_eq!(agent_id.as_ref(), "agent-1");
        assert!(snapshot.interactions.is_empty());
        assert!(snapshot.attachments.is_empty());
        assert!(snapshot.todos.is_empty());
        assert_eq!(snapshot.has_more_older, Some(true));

        let error = serde_json::from_value::<TranscriptOperation>(json!({
            "op": "marker.upsert",
            "item": {"kind": "taskref", "markerId": "m1", "marker": "clear"}
        }))
        .unwrap_err();
        assert!(error.to_string().contains("expected kind `marker`"));
    }
}
