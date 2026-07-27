//! Tool-executor domain event payloads.
//!
//! Original: `packages/agent-core-v2/src/agent/toolExecutor/toolExecutorEvents.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    app::event::event_bus::DomainEventPayload,
    tool::{ToolInputDisplay, ToolUpdate},
};

/// Payload published when a tool call has been prepared for execution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallStartedEvent {
    pub turn_id: i64,
    pub tool_call_id: String,
    pub name: String,
    pub args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<ToolInputDisplay>,
}

impl ToolCallStartedEvent {
    pub const EVENT_TYPE: &'static str = "tool.call.started";
}

impl DomainEventPayload for ToolCallStartedEvent {
    const TYPE: &'static str = Self::EVENT_TYPE;
}

/// Payload published for a live update emitted by a runnable tool.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolProgressEvent {
    pub turn_id: i64,
    pub tool_call_id: String,
    pub update: ToolUpdate,
}

impl ToolProgressEvent {
    pub const EVENT_TYPE: &'static str = "tool.progress";
}

impl DomainEventPayload for ToolProgressEvent {
    const TYPE: &'static str = Self::EVENT_TYPE;
}

/// Payload published after a tool result has completed finalization.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEvent {
    pub turn_id: i64,
    pub tool_call_id: String,
    pub output: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
}

impl ToolResultEvent {
    pub const EVENT_TYPE: &'static str = "tool.result";
}

impl DomainEventPayload for ToolResultEvent {
    const TYPE: &'static str = Self::EVENT_TYPE;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tool::{ToolUpdate, ToolUpdateKind};

    #[test]
    fn lifecycle_event_payloads_preserve_source_field_names_and_optional_values() {
        let started = ToolCallStartedEvent {
            turn_id: 4,
            tool_call_id: "call-1".into(),
            name: "Read".into(),
            args: json!({"path": "src/lib.rs"}),
            description: None,
            display: None,
        };
        assert_eq!(ToolCallStartedEvent::EVENT_TYPE, "tool.call.started");
        assert_eq!(
            serde_json::to_value(started).unwrap(),
            json!({
                "turnId": 4,
                "toolCallId": "call-1",
                "name": "Read",
                "args": {"path": "src/lib.rs"},
            })
        );

        let progress = ToolProgressEvent {
            turn_id: 4,
            tool_call_id: "call-1".into(),
            update: ToolUpdate {
                kind: ToolUpdateKind::Progress,
                text: Some("reading".into()),
                percent: Some(50.0),
                custom_kind: None,
                custom_data: None,
            },
        };
        assert_eq!(ToolProgressEvent::EVENT_TYPE, "tool.progress");
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            json!({
                "turnId": 4,
                "toolCallId": "call-1",
                "update": {"kind": "progress", "text": "reading", "percent": 50.0},
            })
        );

        let result = ToolResultEvent {
            turn_id: 4,
            tool_call_id: "call-1".into(),
            output: json!("done"),
            is_error: Some(false),
            synthetic: Some(true),
        };
        assert_eq!(ToolResultEvent::EVENT_TYPE, "tool.result");
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "turnId": 4,
                "toolCallId": "call-1",
                "output": "done",
                "isError": false,
                "synthetic": true,
            })
        );
    }
}
