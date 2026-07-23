//! Leaf render units inside transcript steps.
//!
//! Original: `packages/transcript/src/model/frame.ts`.

use serde::{Deserialize, Serialize};

use super::{
    AgentId, AttachmentId, FrameId, InteractionId, InteractionKind, InteractionState,
    OptionalJsonValue, TaskId, TodoId,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum FrameRef {
    Frame {
        #[serde(rename = "frameId")]
        frame_id: FrameId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextRole {
    Assistant,
    User,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFrame {
    pub frame_id: FrameId,
    pub role: TextRole,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ids: Option<Vec<AttachmentId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingFrame {
    pub frame_id: FrameId,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFrameState {
    Running,
    Done,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRefRole {
    Child,
    Member,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRef {
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<AgentRefRole>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallFrame {
    pub frame_id: FrameId,
    pub tool_call_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    pub state: ToolFrameState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub input: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub output: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub display: OptionalJsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<InteractionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo_id: Option<TodoId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_refs: Option<Vec<AgentRef>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionFrame {
    pub frame_id: FrameId,
    pub interaction_id: InteractionId,
    pub interaction_kind: InteractionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeFrame {
    pub frame_id: FrameId,
    pub level: NoticeLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_utils::double_option"
    )]
    pub detail: OptionalJsonValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TranscriptFrame {
    #[serde(rename = "text")]
    Text(TextFrame),
    #[serde(rename = "thinking")]
    Thinking(ThinkingFrame),
    #[serde(rename = "tool")]
    Tool(Box<ToolCallFrame>),
    #[serde(rename = "interaction")]
    Interaction(InteractionFrame),
    #[serde(rename = "notice")]
    Notice(NoticeFrame),
}

impl TranscriptFrame {
    pub fn frame_id(&self) -> &FrameId {
        match self {
            Self::Text(frame) => &frame.frame_id,
            Self::Thinking(frame) => &frame.frame_id,
            Self::Tool(frame) => &frame.frame_id,
            Self::Interaction(frame) => &frame.frame_id,
            Self::Notice(frame) => &frame.frame_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn preserves_wire_discriminants_camel_case_and_explicit_null() {
        let wire = json!({
            "kind": "tool",
            "frameId": "t1.1.call_1",
            "toolCallId": "call_1",
            "name": "Read",
            "state": "running",
            "input": null
        });

        let frame: TranscriptFrame = serde_json::from_value(wire.clone()).unwrap();
        let TranscriptFrame::Tool(tool) = &frame else {
            panic!("expected tool frame");
        };
        assert_eq!(tool.input, Some(None));
        assert_eq!(tool.output, None);
        assert_eq!(serde_json::to_value(frame).unwrap(), wire);
    }
}
