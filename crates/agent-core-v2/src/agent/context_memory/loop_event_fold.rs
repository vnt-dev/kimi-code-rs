use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::kosong::contract::{
    message::{
        ContentPart, Message, Role, ToolCall, ToolCallType, ToolOutput, create_tool_message,
    },
    provider::FinishReason,
    usage::TokenUsage,
};

use super::{types::ContextMessage, vacuous_content::is_vacuous_content_part};

const TOOL_INTERRUPTED_ON_RESUME_OUTPUT: &str = "Tool execution was interrupted before its result was recorded. Do not assume the tool completed successfully.";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum LoopToolResultOutput {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopToolResult {
    pub output: LoopToolResultOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/loopEventFold.ts
//   LoopRecordedEvent
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum LoopRecordedEvent {
    #[serde(rename = "step.begin", rename_all = "camelCase")]
    StepBegin {
        uuid: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
    },
    #[serde(rename = "step.end", rename_all = "camelCase")]
    StepEnd {
        uuid: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_first_token_latency_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_stream_duration_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_request_build_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_server_first_token_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_server_decode_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_client_consume_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_finish_reason: Option<FinishReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_finish_reason: Option<String>,
    },
    #[serde(rename = "content.part", rename_all = "camelCase")]
    ContentPart {
        step_uuid: String,
        part: ContentPart,
        #[serde(skip_serializing_if = "Option::is_none")]
        uuid: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
    },
    #[serde(rename = "tool.call", rename_all = "camelCase")]
    ToolCall {
        step_uuid: String,
        tool_call_id: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        extras: Option<Map<String, Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        uuid: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
    },
    #[serde(rename = "tool.result", rename_all = "camelCase")]
    ToolResult {
        tool_call_id: String,
        result: LoopToolResult,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_uuid: Option<String>,
    },
}

// Rust adaptation of the source WeakMap-backed FoldCtx. Ownership makes fold
// state explicit and naturally isolates concurrent agent/replay scopes while
// the public history remains a plain ContextMessage slice.
#[derive(Clone, Debug, Default)]
pub struct LoopEventFold {
    messages: Vec<ContextMessage>,
    open_step_uuid: Option<String>,
    pending: IndexSet<String>,
    deferred: Vec<ContextMessage>,
}

impl LoopEventFold {
    pub fn new(messages: Vec<ContextMessage>) -> Self {
        Self {
            messages,
            ..Self::default()
        }
    }

    pub fn messages(&self) -> &[ContextMessage] {
        &self.messages
    }

    pub fn into_messages(self) -> Vec<ContextMessage> {
        self.messages
    }

    // Original: loopEventFold.ts, foldAppendMessage().
    pub fn fold_append_message(&mut self, message: ContextMessage) {
        if self.pending.is_empty() {
            self.messages.push(message);
        } else {
            self.deferred.push(message);
        }
    }

    // Original: loopEventFold.ts, foldLoopEvent().
    pub fn fold_loop_event(&mut self, event: LoopRecordedEvent) {
        match event {
            LoopRecordedEvent::StepBegin { uuid, .. } => {
                self.settle_open_step();
                let mut assistant =
                    context_message(Message::new(Role::Assistant, Vec::new(), Vec::new()));
                assistant.message.partial = Some(true);
                self.open_step_uuid = Some(uuid);
                self.messages.push(assistant);
            }
            LoopRecordedEvent::StepEnd { .. } => {
                self.open_step_uuid = None;
                self.settle_open_step();
                self.flush_deferred();
            }
            LoopRecordedEvent::ContentPart { part, .. } => {
                self.update_open_assistant(|message| message.message.content.push(part));
            }
            LoopRecordedEvent::ToolCall {
                tool_call_id,
                name,
                args,
                extras,
                ..
            } => {
                let arguments = args.map(|args| args.to_string());
                self.pending.insert(tool_call_id.clone());
                self.update_open_assistant(|message| {
                    message.message.tool_calls.push(ToolCall {
                        call_type: ToolCallType::Function,
                        id: tool_call_id,
                        name,
                        arguments,
                        extras,
                        stream_index: None,
                    });
                });
            }
            LoopRecordedEvent::ToolResult {
                tool_call_id,
                result,
                ..
            } => {
                if !self.pending.shift_remove(&tool_call_id) {
                    return;
                }
                let output = match result.output {
                    LoopToolResultOutput::Text(output) => ToolOutput::Text(output),
                    LoopToolResultOutput::Parts(parts) => ToolOutput::Parts(parts),
                };
                let mut message = context_message(create_tool_message(&tool_call_id, output));
                message.is_error = result.is_error;
                message.note = result.note;
                self.messages.push(message);
                self.flush_deferred();
            }
        }
    }

    // Original: loopEventFold.ts, resetFold().
    pub fn reset_fold(&mut self) {
        self.open_step_uuid = None;
        self.pending.clear();
        self.deferred.clear();
    }

    fn update_open_assistant(&mut self, update: impl FnOnce(&mut ContextMessage)) {
        if let Some(index) = self.find_open_assistant_index() {
            update(&mut self.messages[index]);
        }
    }

    // Original: loopEventFold.ts, settleOpenStep().
    fn settle_open_step(&mut self) {
        self.close_pending();
        let Some(index) = self.find_open_assistant_index() else {
            return;
        };
        let open = &self.messages[index];
        if open.message.tool_calls.is_empty()
            && open.message.content.iter().all(is_vacuous_content_part)
        {
            self.messages.remove(index);
        } else {
            self.messages[index].message.partial = None;
        }
    }

    fn find_open_assistant_index(&self) -> Option<usize> {
        self.messages
            .iter()
            .rposition(|message| message.message.partial == Some(true))
    }

    // Original: loopEventFold.ts, closePending().
    fn close_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        for tool_call_id in std::mem::take(&mut self.pending) {
            self.messages.push(interrupted_tool_message(&tool_call_id));
        }
        self.flush_deferred();
    }

    fn flush_deferred(&mut self) {
        if self.pending.is_empty() && !self.deferred.is_empty() {
            self.messages.append(&mut self.deferred);
        }
    }
}

fn context_message(message: Message) -> ContextMessage {
    ContextMessage {
        message,
        id: None,
        provider_message_id: None,
        origin: None,
        is_error: None,
        note: None,
    }
}

fn interrupted_tool_message(tool_call_id: &str) -> ContextMessage {
    let mut message = context_message(create_tool_message(
        tool_call_id,
        ToolOutput::Text(TOOL_INTERRUPTED_ON_RESUME_OUTPUT.to_owned()),
    ));
    message.is_error = Some(true);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(uuid: &str) -> LoopRecordedEvent {
        LoopRecordedEvent::StepBegin {
            uuid: uuid.into(),
            turn_id: None,
            step: None,
        }
    }

    fn end(uuid: &str) -> LoopRecordedEvent {
        LoopRecordedEvent::StepEnd {
            uuid: uuid.into(),
            turn_id: None,
            step: None,
            finish_reason: None,
            usage: None,
            llm_first_token_latency_ms: None,
            llm_stream_duration_ms: None,
            llm_request_build_ms: None,
            llm_server_first_token_ms: None,
            llm_server_decode_ms: None,
            llm_client_consume_ms: None,
            message_id: None,
            provider_finish_reason: None,
            raw_finish_reason: None,
        }
    }

    fn content(part: ContentPart) -> LoopRecordedEvent {
        LoopRecordedEvent::ContentPart {
            step_uuid: "s".into(),
            part,
            uuid: None,
            turn_id: None,
            step: None,
        }
    }

    #[test]
    fn event_serialization_preserves_record_field_names() {
        assert_eq!(
            serde_json::to_value(LoopRecordedEvent::ToolCall {
                step_uuid: "s1".into(),
                tool_call_id: "c1".into(),
                name: "Lookup".into(),
                args: Some(serde_json::json!({ "q": "moon" })),
                extras: None,
                uuid: None,
                turn_id: Some("1".into()),
                step: Some(2.0),
            })
            .unwrap(),
            serde_json::json!({
                "type": "tool.call",
                "stepUuid": "s1",
                "toolCallId": "c1",
                "name": "Lookup",
                "args": { "q": "moon" },
                "turnId": "1",
                "step": 2.0
            })
        );
    }

    #[test]
    fn folds_text_tool_call_and_result() {
        let mut fold = LoopEventFold::default();
        fold.fold_loop_event(begin("s1"));
        fold.fold_loop_event(content(ContentPart::Text {
            text: "I will call.".into(),
        }));
        fold.fold_loop_event(LoopRecordedEvent::ToolCall {
            step_uuid: "s1".into(),
            tool_call_id: "c1".into(),
            name: "Lookup".into(),
            args: Some(serde_json::json!({ "q": "moon" })),
            extras: None,
            uuid: None,
            turn_id: None,
            step: None,
        });
        fold.fold_loop_event(LoopRecordedEvent::ToolResult {
            tool_call_id: "c1".into(),
            result: LoopToolResult {
                output: LoopToolResultOutput::Text("lookup result".into()),
                is_error: Some(false),
                note: None,
            },
            parent_uuid: None,
        });
        fold.fold_loop_event(end("s1"));

        assert_eq!(fold.messages().len(), 2);
        assert_eq!(fold.messages()[0].message.partial, None);
        assert_eq!(
            fold.messages()[0].message.tool_calls[0]
                .arguments
                .as_deref(),
            Some("{\"q\":\"moon\"}")
        );
        assert_eq!(
            fold.messages()[1].message.tool_call_id.as_deref(),
            Some("c1")
        );
        assert_eq!(fold.messages()[1].is_error, Some(false));
    }

    #[test]
    fn retry_drops_vacuous_partial_and_keeps_recovered_output() {
        let mut fold = LoopEventFold::default();
        fold.fold_loop_event(begin("s1"));
        fold.fold_loop_event(content(ContentPart::Think {
            think: "   ".into(),
            encrypted: None,
        }));
        fold.fold_loop_event(begin("s2"));
        fold.fold_loop_event(content(ContentPart::Text {
            text: "recovered".into(),
        }));
        fold.fold_loop_event(end("s2"));
        assert_eq!(fold.messages().len(), 1);
        assert!(matches!(
            &fold.messages()[0].message.content[..],
            [ContentPart::Text { text }] if text == "recovered"
        ));
    }

    #[test]
    fn closes_pending_in_call_order_and_then_flushes_deferred() {
        let mut fold = LoopEventFold::default();
        fold.fold_loop_event(begin("s1"));
        for id in ["c1", "c2"] {
            fold.fold_loop_event(LoopRecordedEvent::ToolCall {
                step_uuid: "s1".into(),
                tool_call_id: id.into(),
                name: "Tool".into(),
                args: None,
                extras: None,
                uuid: None,
                turn_id: None,
                step: None,
            });
        }
        fold.fold_append_message(context_message(Message::new(
            Role::User,
            vec![ContentPart::Text {
                text: "deferred".into(),
            }],
            Vec::new(),
        )));
        fold.fold_loop_event(end("s1"));

        let ids = fold
            .messages()
            .iter()
            .filter_map(|message| message.message.tool_call_id.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["c1", "c2"]);
        assert_eq!(fold.messages().last().unwrap().message.role, Role::User);
    }

    #[test]
    fn ignores_result_without_a_pending_call() {
        let mut fold = LoopEventFold::default();
        fold.fold_loop_event(LoopRecordedEvent::ToolResult {
            tool_call_id: "missing".into(),
            result: LoopToolResult {
                output: LoopToolResultOutput::Text("result".into()),
                is_error: None,
                note: None,
            },
            parent_uuid: None,
        });
        assert!(fold.messages().is_empty());
    }
}
