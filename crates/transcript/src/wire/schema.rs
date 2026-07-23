//! Validation for every transcript value crossing a process boundary.
//!
//! Original:
//!   `packages/transcript/src/wire/schema.ts`
//!
//! The source exports one Zod schema constant per model shape. Rust
//! consolidates those constants into the corresponding Serde model type plus
//! this module's `WireValidate` implementation. `parse_wire_value` is the
//! common parse entry point; unknown object fields are ignored just as Zod
//! object parsing strips them, while opaque JSON envelopes remain open.

use std::error::Error;
use std::fmt;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::granularity::{TranscriptGrade, TranscriptGradeSpec};
use crate::model::{
    AgentId, AgentRef, FrameId, GoalMeta, InteractionFrame, InteractionId, ModesMeta,
    ModesMetaMerge, NoticeFrame, StepId, StepState, TaskId, TextFrame, ThinkingFrame, TodoItem,
    ToolCallFrame, TranscriptAttachment, TranscriptFrame, TranscriptInteraction, TranscriptItem,
    TranscriptMarker, TranscriptMeta, TranscriptMetaMerge, TranscriptStep, TranscriptTask,
    TranscriptTaskRef, TranscriptTodo, TranscriptTurn, TranscriptUsage, TurnId, TurnOrigin,
    TurnState,
};
use crate::ops::{
    AgentTranscriptSnapshot, AppendTarget, StepHeader, TranscriptOpBatch, TranscriptOperation,
    TurnHeader,
};
use crate::store::AgentDescriptor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    Deserialize(String),
    Validation { path: String, message: &'static str },
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deserialize(message) => {
                write!(formatter, "invalid transcript wire value: {message}")
            }
            Self::Validation { path, message } => {
                write!(
                    formatter,
                    "invalid transcript wire value at {path}: {message}"
                )
            }
        }
    }
}

impl Error for WireError {}

pub trait WireValidate {
    fn validate_wire(&self) -> Result<(), WireError>;
}

pub fn parse_wire_value<T>(value: Value) -> Result<T, WireError>
where
    T: DeserializeOwned + WireValidate,
{
    let parsed: T =
        serde_json::from_value(value).map_err(|error| WireError::Deserialize(error.to_string()))?;
    parsed.validate_wire()?;
    Ok(parsed)
}

pub fn parse_wire_json<T>(json: &str) -> Result<T, WireError>
where
    T: DeserializeOwned + WireValidate,
{
    let parsed: T =
        serde_json::from_str(json).map_err(|error| WireError::Deserialize(error.to_string()))?;
    parsed.validate_wire()?;
    Ok(parsed)
}

fn validation(path: impl Into<String>, message: &'static str) -> WireError {
    WireError::Validation {
        path: path.into(),
        message,
    }
}

fn non_empty(path: impl Into<String>, value: &str) -> Result<(), WireError> {
    if value.is_empty() {
        Err(validation(path, "must not be empty"))
    } else {
        Ok(())
    }
}

fn indexed(path: &str, index: usize) -> String {
    format!("{path}[{index}]")
}

fn field(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

fn validate_origin(origin: &TurnOrigin, path: &str) -> Result<(), WireError> {
    match origin {
        TurnOrigin::Cron {
            task_id: Some(task_id),
            ..
        }
        | TurnOrigin::Task { task_id, .. } => non_empty(field(path, "taskId"), task_id.as_ref()),
        _ => Ok(()),
    }
}

fn validate_agent_ref(agent_ref: &AgentRef, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "agentId"), agent_ref.agent_id.as_ref())
}

fn validate_text_frame(frame: &TextFrame, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "frameId"), frame.frame_id.as_ref())?;
    if let Some(task_id) = &frame.task_id {
        non_empty(field(path, "taskId"), task_id.as_ref())?;
    }
    Ok(())
}

fn validate_thinking_frame(frame: &ThinkingFrame, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "frameId"), frame.frame_id.as_ref())
}

fn validate_tool_frame(frame: &ToolCallFrame, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "frameId"), frame.frame_id.as_ref())?;
    if let Some(task_id) = &frame.task_id {
        non_empty(field(path, "taskId"), task_id.as_ref())?;
    }
    if let Some(agent_refs) = &frame.agent_refs {
        for (index, agent_ref) in agent_refs.iter().enumerate() {
            validate_agent_ref(agent_ref, &indexed(&field(path, "agentRefs"), index))?;
        }
    }
    Ok(())
}

fn validate_interaction_frame(frame: &InteractionFrame, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "frameId"), frame.frame_id.as_ref())
}

fn validate_notice_frame(frame: &NoticeFrame, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "frameId"), frame.frame_id.as_ref())
}

fn validate_frame(frame: &TranscriptFrame, path: &str) -> Result<(), WireError> {
    match frame {
        TranscriptFrame::Text(frame) => validate_text_frame(frame, path),
        TranscriptFrame::Thinking(frame) => validate_thinking_frame(frame, path),
        TranscriptFrame::Tool(frame) => validate_tool_frame(frame, path),
        TranscriptFrame::Interaction(frame) => validate_interaction_frame(frame, path),
        TranscriptFrame::Notice(frame) => validate_notice_frame(frame, path),
    }
}

fn validate_step(step: &TranscriptStep, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "stepId"), step.step_id.as_ref())?;
    non_empty(field(path, "turnId"), step.turn_id.as_ref())?;
    for (index, frame) in step.frames.iter().enumerate() {
        validate_frame(frame, &indexed(&field(path, "frames"), index))?;
    }
    Ok(())
}

fn validate_turn(turn: &TranscriptTurn, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "turnId"), turn.turn_id.as_ref())?;
    validate_origin(&turn.origin, &field(path, "origin"))?;
    for (index, step) in turn.steps.iter().enumerate() {
        validate_step(step, &indexed(&field(path, "steps"), index))?;
    }
    Ok(())
}

fn validate_item(item: &TranscriptItem, path: &str) -> Result<(), WireError> {
    match item {
        TranscriptItem::Turn(turn) => validate_turn(turn, path),
        TranscriptItem::TaskRef(task_ref) => {
            non_empty(field(path, "taskId"), task_ref.task_id.as_ref())
        }
        TranscriptItem::Marker(_) => Ok(()),
    }
}

fn validate_task(task: &TranscriptTask, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "taskId"), task.task_id.as_ref())?;
    if let Some(agent_id) = &task.agent_id {
        non_empty(field(path, "agentId"), agent_id.as_ref())?;
    }
    Ok(())
}

fn validate_snapshot(snapshot: &AgentTranscriptSnapshot, path: &str) -> Result<(), WireError> {
    for (index, item) in snapshot.items.iter().enumerate() {
        validate_item(item, &indexed(&field(path, "items"), index))?;
    }
    for (index, task) in snapshot.tasks.iter().enumerate() {
        validate_task(task, &indexed(&field(path, "tasks"), index))?;
    }
    Ok(())
}

fn validate_turn_header(header: &TurnHeader, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "turnId"), header.turn_id.as_ref())?;
    validate_origin(&header.origin, &field(path, "origin"))
}

fn validate_step_header(header: &StepHeader, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "stepId"), header.step_id.as_ref())?;
    non_empty(field(path, "turnId"), header.turn_id.as_ref())
}

fn validate_append_target(target: &AppendTarget, path: &str) -> Result<(), WireError> {
    match target {
        AppendTarget::Frame {
            turn_id,
            step_id,
            frame_id,
        } => {
            non_empty(field(path, "turnId"), turn_id.as_ref())?;
            non_empty(field(path, "stepId"), step_id.as_ref())?;
            non_empty(field(path, "frameId"), frame_id.as_ref())
        }
        AppendTarget::Task { task_id } => non_empty(field(path, "taskId"), task_id.as_ref()),
    }
}

fn validate_operation(operation: &TranscriptOperation, path: &str) -> Result<(), WireError> {
    match operation {
        TranscriptOperation::Reset { agent_id, snapshot } => {
            non_empty(field(path, "agentId"), agent_id.as_ref())?;
            validate_snapshot(snapshot, &field(path, "snapshot"))
        }
        TranscriptOperation::TurnUpsert { turn } => {
            validate_turn_header(turn, &field(path, "turn"))
        }
        TranscriptOperation::StepUpsert { turn_id, step } => {
            non_empty(field(path, "turnId"), turn_id.as_ref())?;
            validate_step_header(step, &field(path, "step"))
        }
        TranscriptOperation::FrameUpsert {
            turn_id,
            step_id,
            frame,
        } => {
            non_empty(field(path, "turnId"), turn_id.as_ref())?;
            non_empty(field(path, "stepId"), step_id.as_ref())?;
            validate_frame(frame, &field(path, "frame"))
        }
        TranscriptOperation::Append { target, .. } => {
            validate_append_target(target, &field(path, "target"))
        }
        TranscriptOperation::TaskRefUpsert { item, .. } => {
            non_empty(field(path, "item.taskId"), item.task_id.as_ref())
        }
        TranscriptOperation::TaskUpsert { task } => validate_task(task, &field(path, "task")),
        _ => Ok(()),
    }
}

macro_rules! no_extra_validation {
    ($($type:ty),+ $(,)?) => {
        $(
            impl WireValidate for $type {
                fn validate_wire(&self) -> Result<(), WireError> {
                    Ok(())
                }
            }
        )+
    };
}

no_extra_validation!(
    TranscriptUsage,
    TranscriptInteraction,
    TranscriptMarker,
    GoalMeta,
    ModesMeta,
    ModesMetaMerge,
    TranscriptMeta,
    TranscriptMetaMerge,
    TranscriptAttachment,
    TodoItem,
    TranscriptTodo,
    TranscriptGrade,
    TurnState,
    StepState,
);

macro_rules! non_empty_id_validation {
    ($($type:ty),+ $(,)?) => {
        $(
            impl WireValidate for $type {
                fn validate_wire(&self) -> Result<(), WireError> {
                    non_empty("id", self.as_ref())
                }
            }
        )+
    };
}

non_empty_id_validation!(TurnId, StepId, FrameId, TaskId, AgentId);

impl WireValidate for TurnOrigin {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_origin(self, "origin")
    }
}

impl WireValidate for AgentRef {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_agent_ref(self, "agentRef")
    }
}

impl WireValidate for TextFrame {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_text_frame(self, "frame")
    }
}

impl WireValidate for ThinkingFrame {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_thinking_frame(self, "frame")
    }
}

impl WireValidate for ToolCallFrame {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_tool_frame(self, "frame")
    }
}

impl WireValidate for InteractionFrame {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_interaction_frame(self, "frame")
    }
}

impl WireValidate for NoticeFrame {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_notice_frame(self, "frame")
    }
}

impl WireValidate for TranscriptFrame {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_frame(self, "frame")
    }
}

impl WireValidate for TranscriptStep {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_step(self, "step")
    }
}

impl WireValidate for TranscriptTurn {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_turn(self, "turn")
    }
}

impl WireValidate for TranscriptTaskRef {
    fn validate_wire(&self) -> Result<(), WireError> {
        non_empty("taskref.taskId", self.task_id.as_ref())
    }
}

impl WireValidate for TranscriptItem {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_item(self, "item")
    }
}

impl WireValidate for TranscriptTask {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_task(self, "task")
    }
}

impl WireValidate for AgentTranscriptSnapshot {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_snapshot(self, "snapshot")
    }
}

impl WireValidate for TurnHeader {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_turn_header(self, "turn")
    }
}

impl WireValidate for StepHeader {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_step_header(self, "step")
    }
}

impl WireValidate for AppendTarget {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_append_target(self, "target")
    }
}

impl WireValidate for TranscriptOperation {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_operation(self, "operation")
    }
}

impl WireValidate for TranscriptOpBatch {
    fn validate_wire(&self) -> Result<(), WireError> {
        non_empty("batch.agentId", self.agent_id.as_ref())?;
        for (index, operation) in self.ops.iter().enumerate() {
            validate_operation(operation, &indexed("batch.ops", index))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireTranscriptGradeSpec(pub IndexMap<String, TranscriptGrade>);

impl WireValidate for WireTranscriptGradeSpec {
    fn validate_wire(&self) -> Result<(), WireError> {
        Ok(())
    }
}

impl From<WireTranscriptGradeSpec> for TranscriptGradeSpec {
    fn from(spec: WireTranscriptGradeSpec) -> Self {
        spec.0
            .into_iter()
            .map(|(agent_id, grade)| (agent_id, Some(grade)))
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TranscriptSubscription(pub IndexMap<String, WireTranscriptGradeSpec>);

impl WireValidate for TranscriptSubscription {
    fn validate_wire(&self) -> Result<(), WireError> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptQuery {
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_turn: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_turn: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
}

impl WireValidate for TranscriptQuery {
    fn validate_wire(&self) -> Result<(), WireError> {
        non_empty("query.agent_id", self.agent_id.as_ref())?;
        if !is_plain_agent_id(self.agent_id.as_ref()) {
            return Err(validation(
                "query.agent_id",
                "must be a plain agent id (no path separators)",
            ));
        }
        if let Some(before_turn) = &self.before_turn {
            non_empty("query.before_turn", before_turn.as_ref())?;
        }
        if let Some(after_turn) = &self.after_turn {
            non_empty("query.after_turn", after_turn.as_ref())?;
        }
        if self.before_turn.is_some() && self.after_turn.is_some() {
            return Err(validation(
                "query.before_turn",
                "before_turn and after_turn are mutually exclusive",
            ));
        }
        if let Some(page_size) = self.page_size
            && !(1..=100).contains(&page_size)
        {
            return Err(validation("query.page_size", "must be between 1 and 100"));
        }
        Ok(())
    }
}

pub fn is_plain_agent_id(agent_id: &str) -> bool {
    let length = agent_id.len();
    (1..=128).contains(&length)
        && agent_id != "."
        && agent_id != ".."
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptResponse {
    pub agent_id: AgentId,
    pub items: Vec<TranscriptItem>,
    pub has_more: bool,
    pub tasks: Vec<TranscriptTask>,
    #[serde(default)]
    pub interactions: Vec<TranscriptInteraction>,
    #[serde(default)]
    pub attachments: Vec<TranscriptAttachment>,
    #[serde(default)]
    pub todos: Vec<TranscriptTodo>,
    pub meta: TranscriptMeta,
    pub agents: Vec<AgentDescriptor>,
    pub pending_interactions: Vec<InteractionId>,
}

impl WireValidate for TranscriptResponse {
    fn validate_wire(&self) -> Result<(), WireError> {
        non_empty("response.agent_id", self.agent_id.as_ref())?;
        for (index, item) in self.items.iter().enumerate() {
            validate_item(item, &indexed("response.items", index))?;
        }
        for (index, task) in self.tasks.iter().enumerate() {
            validate_task(task, &indexed("response.tasks", index))?;
        }
        for (index, descriptor) in self.agents.iter().enumerate() {
            validate_agent_descriptor(descriptor, &indexed("response.agents", index))?;
        }
        Ok(())
    }
}

fn validate_agent_descriptor(descriptor: &AgentDescriptor, path: &str) -> Result<(), WireError> {
    non_empty(field(path, "agentId"), descriptor.agent_id.as_ref())?;
    if let Some(parent_agent_id) = &descriptor.parent_agent_id {
        non_empty(field(path, "parentAgentId"), parent_agent_id.as_ref())?;
    }
    Ok(())
}

impl WireValidate for AgentDescriptor {
    fn validate_wire(&self) -> Result<(), WireError> {
        validate_agent_descriptor(self, "agent")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptResetPayload {
    pub agent_id: AgentId,
    pub snapshot: AgentTranscriptSnapshot,
    pub has_more_older: bool,
}

impl WireValidate for TranscriptResetPayload {
    fn validate_wire(&self) -> Result<(), WireError> {
        non_empty("reset.agent_id", self.agent_id.as_ref())?;
        validate_snapshot(&self.snapshot, "reset.snapshot")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptOpsPayload {
    pub agent_id: AgentId,
    pub ops: Vec<TranscriptOperation>,
}

impl WireValidate for TranscriptOpsPayload {
    fn validate_wire(&self) -> Result<(), WireError> {
        non_empty("ops.agent_id", self.agent_id.as_ref())?;
        for (index, operation) in self.ops.iter().enumerate() {
            validate_operation(operation, &indexed("ops.ops", index))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn validates_every_operation_variant() {
        let operations = [
            json!({
                "op": "reset", "agentId": "main",
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
                "op": "step.upsert", "turnId": "t1",
                "step": {
                    "kind": "step", "stepId": "t1.0", "turnId": "t1",
                    "ordinal": 0, "state": "running"
                }
            }),
            json!({
                "op": "frame.upsert", "turnId": "t1", "stepId": "t1.0",
                "frame": {"kind": "thinking", "frameId": "f1", "text": ""}
            }),
            json!({
                "op": "append", "target": {"type": "task", "taskId": "task"},
                "offset": 0, "text": ""
            }),
            json!({
                "op": "marker.upsert",
                "item": {"kind": "marker", "markerId": "", "marker": ""}
            }),
            json!({
                "op": "taskref.upsert",
                "item": {"kind": "taskref", "refId": "", "taskId": "task"}
            }),
            json!({
                "op": "task.upsert",
                "task": {
                    "taskId": "task", "kind": "shell", "state": "running",
                    "detached": false, "outputTail": ""
                }
            }),
            json!({
                "op": "interaction.upsert",
                "interaction": {
                    "interactionId": "", "interactionKind": "approval",
                    "toolCallId": "", "state": "pending"
                }
            }),
            json!({
                "op": "attachment.upsert",
                "attachment": {"attachmentId": "", "mediaType": ""}
            }),
            json!({
                "op": "todo.upsert",
                "todo": {"todoId": "", "items": []}
            }),
            json!({"op": "meta.merge", "meta": {}}),
            json!({"op": "items.remove", "ids": [""]}),
        ];
        for operation in operations {
            parse_wire_value::<TranscriptOperation>(operation).unwrap();
        }
    }

    #[test]
    fn defaults_backward_compatible_global_collections() {
        let snapshot: AgentTranscriptSnapshot = parse_wire_value(json!({
            "items": [],
            "tasks": [],
            "meta": {}
        }))
        .unwrap();
        assert!(snapshot.interactions.is_empty());
        assert!(snapshot.attachments.is_empty());
        assert!(snapshot.todos.is_empty());

        let response: TranscriptResponse = parse_wire_value(json!({
            "agent_id": "main",
            "items": [],
            "has_more": false,
            "tasks": [],
            "meta": {},
            "agents": [{"agentId": "main", "type": "main"}],
            "pending_interactions": []
        }))
        .unwrap();
        assert!(response.interactions.is_empty());
        assert!(response.attachments.is_empty());
        assert!(response.todos.is_empty());
    }

    #[test]
    fn rejects_bad_grades_cursors_page_sizes_and_empty_nested_ids() {
        assert!(parse_wire_value::<WireTranscriptGradeSpec>(json!({"*": "stream"})).is_err());
        assert!(
            parse_wire_value::<TranscriptQuery>(json!({
                "agent_id": "main", "before_turn": "t2", "after_turn": "t1"
            }))
            .is_err()
        );
        assert!(
            parse_wire_value::<TranscriptQuery>(json!({
                "agent_id": "main", "page_size": 101
            }))
            .is_err()
        );
        assert!(
            parse_wire_value::<TranscriptOperation>(json!({
                "op": "turn.upsert",
                "turn": {
                    "kind": "turn", "turnId": "", "ordinal": 0,
                    "state": "running", "origin": {"kind": "user"}
                }
            }))
            .is_err()
        );
    }

    #[test]
    fn accepts_plain_agent_ids_and_rejects_hostile_table() {
        for accepted in ["sub-1", "01HF7YAT31J7SMRT1QXGJWKR8D", "a.b_c"] {
            assert!(is_plain_agent_id(accepted));
            assert!(parse_wire_value::<TranscriptQuery>(json!({"agent_id": accepted})).is_ok());
        }
        let overlong = "x".repeat(129);
        for hostile in [
            "../main",
            "..\\main",
            "..",
            "a/b",
            "a\\b",
            ".",
            "a\0b",
            overlong.as_str(),
            "中文",
        ] {
            assert!(!is_plain_agent_id(hostile), "{hostile:?}");
            assert!(
                parse_wire_value::<TranscriptQuery>(json!({"agent_id": hostile})).is_err(),
                "{hostile:?}"
            );
        }
    }
}
