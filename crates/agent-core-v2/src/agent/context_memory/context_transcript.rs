use std::collections::HashMap;

use indexmap::IndexSet;
use serde_json::Value;

use crate::{
    kosong::contract::message::{ContentPart, Message, Role, ToolCall, ToolCallType},
    wire::record::WireRecord,
};

use super::{
    compaction_handoff::{
        COMPACT_USER_MESSAGE_MAX_TOKENS, collect_compactable_user_messages, is_real_user_input,
        select_recent_user_messages,
    },
    loop_event_fold::{LoopRecordedEvent, LoopToolResultOutput},
    types::{ContextMessage, PromptOrigin},
    vacuous_content::is_vacuous_content_part,
};

const TOOL_INTERRUPTED_ON_RESUME_OUTPUT: &str = "Tool execution was interrupted before its result was recorded. Do not assume the tool completed successfully.";

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/contextTranscript.ts
//   ContextTranscript
#[derive(Clone, Debug, PartialEq)]
pub struct ContextTranscript {
    pub entries: Vec<ContextMessage>,
    pub times: Vec<Option<i64>>,
    pub folded_length: u64,
}

#[derive(Clone, Debug)]
struct MutableEntry {
    key: u64,
    message: ContextMessage,
    time: Option<i64>,
}

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/contextTranscript.ts
//   createContextTranscriptReducer()
//
// Rust adaptation:
//   TypeScript uses object identity to find an open entry after intervening
//   vector edits. A private monotonic key provides the same stable identity.
#[derive(Debug, Default)]
pub struct ContextTranscriptReducer {
    transcript: Vec<MutableEntry>,
    folded_length: u64,
    clear_floor: usize,
    open_steps: HashMap<String, u64>,
    pending_tool_result_ids: IndexSet<String>,
    deferred: Vec<MutableEntry>,
    last_open_step_uuid: Option<String>,
    next_key: u64,
}

// Original: contextTranscript.ts, reduceContextTranscript().
pub fn reduce_context_transcript<'a>(
    records: impl IntoIterator<Item = &'a WireRecord>,
) -> ContextTranscript {
    let mut reducer = create_context_transcript_reducer();
    for record in records {
        reducer.add(record);
    }
    reducer.result()
}

pub fn create_context_transcript_reducer() -> ContextTranscriptReducer {
    ContextTranscriptReducer::default()
}

impl ContextTranscriptReducer {
    // Original: ContextTranscriptReducer.add(). Persisted records have already
    // passed wire-op validation; malformed payloads are ignored defensively.
    pub fn add(&mut self, record: &WireRecord) {
        match record.get("type").and_then(Value::as_str) {
            Some("context.append_message") => {
                let Some(message) = record
                    .get("message")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                else {
                    return;
                };
                let entry = self.mutable_entry(
                    message,
                    read_number(record, "time").map(|value| value as i64),
                );
                if self.pending_tool_result_ids.is_empty() {
                    self.push(entry);
                } else {
                    self.deferred.push(entry);
                }
            }
            Some("context.append_loop_event") => {
                let Some(event) = record
                    .get("event")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                else {
                    return;
                };
                self.apply_loop_event(event, read_number(record, "time").map(|value| value as i64));
            }
            Some("context.apply_compaction") => {
                let summary = context_message(
                    Role::User,
                    vec![ContentPart::Text {
                        text: read_compaction_summary_text(record),
                    }],
                    Vec::new(),
                    None,
                    None,
                    Some(PromptOrigin::CompactionSummary),
                );
                let entry = self.mutable_entry(
                    summary,
                    read_number(record, "time").map(|value| value as i64),
                );
                self.transcript.push(entry);
                self.folded_length = recover_folded_length(
                    record,
                    &self.transcript,
                    self.clear_floor,
                    self.folded_length,
                );
                self.reset_open_state();
            }
            Some("context.undo") => {
                if let Some(count) = read_number(record, "count").map(|value| value as u32) {
                    self.apply_undo(count);
                }
            }
            Some("context.clear") => {
                self.clear_floor = self.transcript.len();
                self.folded_length = 0;
                self.reset_open_state();
            }
            _ => {}
        }
    }

    // Original: ContextTranscriptReducer.result().
    pub fn result(&self) -> ContextTranscript {
        ContextTranscript {
            entries: self
                .transcript
                .iter()
                .map(|entry| entry.message.clone())
                .collect(),
            times: self.transcript.iter().map(|entry| entry.time).collect(),
            folded_length: self.folded_length,
        }
    }

    fn mutable_entry(&mut self, message: ContextMessage, time: Option<i64>) -> MutableEntry {
        let key = self.next_key;
        self.next_key = self.next_key.wrapping_add(1);
        MutableEntry {
            key,
            message: transcript_projection(message),
            time,
        }
    }

    fn push(&mut self, entry: MutableEntry) {
        self.transcript.push(entry);
        self.folded_length += 1;
    }

    fn flush_deferred_if_tool_exchange_closed(&mut self) {
        if !self.pending_tool_result_ids.is_empty() || self.deferred.is_empty() {
            return;
        }
        self.folded_length += self.deferred.len() as u64;
        self.transcript.append(&mut self.deferred);
    }

    fn close_pending_tool_results(&mut self, time: Option<i64>) {
        if self.pending_tool_result_ids.is_empty() {
            return;
        }
        let interrupted = std::mem::take(&mut self.pending_tool_result_ids);
        for tool_call_id in interrupted {
            let message = context_message(
                Role::Tool,
                vec![ContentPart::Text {
                    text: TOOL_INTERRUPTED_ON_RESUME_OUTPUT.to_owned(),
                }],
                Vec::new(),
                Some(tool_call_id),
                Some(true),
                None,
            );
            let entry = self.mutable_entry(message, time);
            self.push(entry);
        }
        self.flush_deferred_if_tool_exchange_closed();
    }

    fn reset_open_state(&mut self) {
        self.open_steps.clear();
        self.pending_tool_result_ids.clear();
        self.deferred.clear();
        self.last_open_step_uuid = None;
    }

    fn settle_step(&mut self, uuid: &str) {
        let Some(key) = self.open_steps.remove(uuid) else {
            return;
        };
        let Some(index) = self.transcript.iter().position(|entry| entry.key == key) else {
            return;
        };
        let message = &self.transcript[index].message.message;
        if !message.tool_calls.is_empty() || !message.content.iter().all(is_vacuous_content_part) {
            return;
        }
        self.transcript.remove(index);
        self.folded_length = self.folded_length.saturating_sub(1);
    }

    fn apply_loop_event(&mut self, event: LoopRecordedEvent, time: Option<i64>) {
        match event {
            LoopRecordedEvent::StepBegin { uuid, .. } => {
                self.close_pending_tool_results(time);
                if let Some(previous) = self.last_open_step_uuid.clone() {
                    self.settle_step(&previous);
                }
                let entry = self.mutable_entry(
                    context_message(Role::Assistant, Vec::new(), Vec::new(), None, None, None),
                    time,
                );
                let key = entry.key;
                self.push(entry);
                self.open_steps.insert(uuid.clone(), key);
                self.last_open_step_uuid = Some(uuid);
            }
            LoopRecordedEvent::StepEnd { uuid, .. } => {
                self.settle_step(&uuid);
                if self.last_open_step_uuid.as_deref() == Some(uuid.as_str()) {
                    self.last_open_step_uuid = None;
                }
                self.flush_deferred_if_tool_exchange_closed();
            }
            LoopRecordedEvent::ContentPart {
                step_uuid, part, ..
            } => {
                if let Some(message) = self.open_message_mut(&step_uuid) {
                    message.message.content.push(part);
                }
            }
            LoopRecordedEvent::ToolCall {
                step_uuid,
                tool_call_id,
                name,
                args,
                extras,
                ..
            } => {
                let Some(message) = self.open_message_mut(&step_uuid) else {
                    return;
                };
                message.message.tool_calls.push(ToolCall {
                    call_type: ToolCallType::Function,
                    id: tool_call_id.clone(),
                    name,
                    arguments: args.map(|value| value.to_string()),
                    extras,
                    stream_index: None,
                });
                self.pending_tool_result_ids.insert(tool_call_id);
            }
            LoopRecordedEvent::ToolResult {
                tool_call_id,
                result,
                ..
            } => {
                if !self.pending_tool_result_ids.shift_remove(&tool_call_id) {
                    return;
                }
                let content = match result.output {
                    LoopToolResultOutput::Text(output) => vec![ContentPart::Text { text: output }],
                    LoopToolResultOutput::Parts(parts) => parts,
                };
                let message = context_message(
                    Role::Tool,
                    content,
                    Vec::new(),
                    Some(tool_call_id),
                    result.is_error,
                    None,
                );
                let entry = self.mutable_entry(message, time);
                self.push(entry);
                self.flush_deferred_if_tool_exchange_closed();
            }
        }
    }

    fn open_message_mut(&mut self, uuid: &str) -> Option<&mut ContextMessage> {
        let key = *self.open_steps.get(uuid)?;
        self.transcript
            .iter_mut()
            .find(|entry| entry.key == key)
            .map(|entry| &mut entry.message)
    }

    fn apply_undo(&mut self, count: u32) {
        if count == 0 {
            return;
        }
        let mut removed_user_count = 0u32;
        let mut index = self.transcript.len();
        while index > self.clear_floor {
            index -= 1;
            let message = &self.transcript[index].message;
            if matches!(message.origin, Some(PromptOrigin::Injection { .. })) {
                continue;
            }
            if matches!(message.origin, Some(PromptOrigin::CompactionSummary)) {
                break;
            }
            let is_user = is_real_user_input(message);
            self.transcript.remove(index);
            self.folded_length = self.folded_length.saturating_sub(1);
            if is_user {
                removed_user_count += 1;
                if removed_user_count >= count {
                    break;
                }
            }
        }
        self.reset_open_state();
    }
}

fn transcript_projection(message: ContextMessage) -> ContextMessage {
    let id = message.id;
    let attachments = message.attachments;
    let mut projected = context_message(
        message.message.role,
        message.message.content,
        message.message.tool_calls,
        message.message.tool_call_id,
        message.is_error,
        message.origin,
    );
    projected.id = id;
    projected.attachments = attachments;
    projected
}

fn context_message(
    role: Role,
    content: Vec<ContentPart>,
    tool_calls: Vec<ToolCall>,
    tool_call_id: Option<String>,
    is_error: Option<bool>,
    origin: Option<PromptOrigin>,
) -> ContextMessage {
    let mut message = Message::new(role, content, tool_calls);
    message.tool_call_id = tool_call_id;
    ContextMessage {
        message,
        id: None,
        provider_message_id: None,
        origin,
        is_error,
        note: None,
        attachments: Vec::new(),
    }
}

// Original: contextTranscript.ts, recoverFoldedLength().
fn recover_folded_length(
    record: &WireRecord,
    transcript: &[MutableEntry],
    clear_floor: usize,
    folded_length: u64,
) -> u64 {
    let kept = read_number(record, "keptUserMessageCount").map(|value| value as u64);
    let kept_head = read_number(record, "keptHeadUserMessageCount").map(|value| value as u64);
    let compacted = read_number(record, "compactedCount").map(|value| value as u64);
    if let Some(kept) = kept {
        return kept + if kept_head.is_none() { 1 } else { 2 };
    }
    if let Some(compacted) = compacted
        && compacted < folded_length
    {
        return 1 + (folded_length - compacted);
    }
    let messages = transcript[clear_floor..]
        .iter()
        .map(|entry| entry.message.clone())
        .collect::<Vec<_>>();
    select_recent_user_messages(
        &collect_compactable_user_messages(&messages),
        COMPACT_USER_MESSAGE_MAX_TOKENS,
    )
    .len() as u64
        + 1
}

fn read_compaction_summary_text(record: &WireRecord) -> String {
    if let Some(summary) = record.get("summary").and_then(Value::as_str) {
        return summary.to_owned();
    }
    if let Some(summary) = record.get("contextSummary").and_then(Value::as_str) {
        return summary.to_owned();
    }
    record
        .get("summary")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| {
                    let part = part.as_object()?;
                    (part.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| part.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn read_number(record: &WireRecord, key: &str) -> Option<f64> {
    record.get(key).and_then(Value::as_f64)
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;

    fn record(value: Value) -> WireRecord {
        value.as_object().cloned().unwrap_or_else(Map::new)
    }

    fn append(text: &str) -> WireRecord {
        record(json!({
            "type": "context.append_message",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }], "toolCalls": [] }
        }))
    }

    fn loop_event(event: Value) -> WireRecord {
        record(json!({ "type": "context.append_loop_event", "event": event }))
    }

    fn texts(result: &ContextTranscript) -> Vec<String> {
        result
            .entries
            .iter()
            .map(|message| {
                message
                    .message
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn builds_transcript_and_keeps_full_history_at_compaction() {
        let records = [
            append("u1"),
            loop_event(json!({ "type": "step.begin", "uuid": "s1" })),
            loop_event(
                json!({ "type": "content.part", "stepUuid": "s1", "part": { "type": "text", "text": "a1" } }),
            ),
            loop_event(json!({ "type": "step.end", "uuid": "s1" })),
            record(
                json!({ "type": "context.apply_compaction", "summary": "SUM", "compactedCount": 2 }),
            ),
            append("u2"),
        ];
        let result = reduce_context_transcript(&records);
        assert_eq!(texts(&result), ["u1", "a1", "SUM", "u2"]);
        assert_eq!(
            result.entries[2].origin,
            Some(PromptOrigin::CompactionSummary)
        );
        assert_eq!(result.folded_length, 3);
    }

    #[test]
    fn recorded_kept_counts_include_summary_and_elision_marker() {
        let records = [
            append("u1"),
            append("u2"),
            record(json!({
                "type": "context.apply_compaction",
                "summary": "SUM",
                "compactedCount": 2,
                "keptUserMessageCount": 2.5,
                "keptHeadUserMessageCount": 1
            })),
        ];
        assert_eq!(reduce_context_transcript(&records).folded_length, 4);
    }

    #[test]
    fn carries_record_times_and_folds_tool_exchange() {
        let records = [
            record(
                json!({ "type": "context.append_message", "message": { "role": "user", "content": [], "toolCalls": [] }, "time": 100 }),
            ),
            record(
                json!({ "type": "context.append_loop_event", "event": { "type": "step.begin", "uuid": "s" }, "time": 200 }),
            ),
            loop_event(
                json!({ "type": "tool.call", "stepUuid": "s", "toolCallId": "c", "name": "Bash", "args": { "command": "echo hi" } }),
            ),
            record(
                json!({ "type": "context.append_loop_event", "event": { "type": "tool.result", "toolCallId": "c", "result": { "output": "ok", "isError": false } }, "time": 220 }),
            ),
            loop_event(json!({ "type": "step.end", "uuid": "s" })),
        ];
        let result = reduce_context_transcript(&records);
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.message.role)
                .collect::<Vec<_>>(),
            [Role::User, Role::Assistant, Role::Tool]
        );
        assert_eq!(result.entries[1].message.tool_calls[0].id, "c");
        assert_eq!(result.entries[2].message.tool_call_id.as_deref(), Some("c"));
        assert_eq!(result.times, [Some(100), Some(200), Some(220)]);
    }

    #[test]
    fn drops_vacuous_attempt_when_step_settles() {
        let records = [
            append("q"),
            loop_event(json!({ "type": "step.begin", "uuid": "s1" })),
            loop_event(
                json!({ "type": "content.part", "stepUuid": "s1", "part": { "type": "think", "think": "" } }),
            ),
            loop_event(json!({ "type": "step.begin", "uuid": "s2" })),
            loop_event(
                json!({ "type": "content.part", "stepUuid": "s2", "part": { "type": "text", "text": "recovered" } }),
            ),
            loop_event(json!({ "type": "step.end", "uuid": "s2" })),
        ];
        let result = reduce_context_transcript(&records);
        assert_eq!(texts(&result), ["q", "recovered"]);
        assert_eq!(result.folded_length, 2);
    }

    #[test]
    fn next_step_interrupts_pending_tools_then_flushes_deferred_messages() {
        let records = [
            loop_event(json!({ "type": "step.begin", "uuid": "s1" })),
            loop_event(
                json!({ "type": "tool.call", "stepUuid": "s1", "toolCallId": "c", "name": "Bash" }),
            ),
            append("deferred"),
            loop_event(json!({ "type": "step.begin", "uuid": "s2" })),
        ];
        let result = reduce_context_transcript(&records);
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.message.role)
                .collect::<Vec<_>>(),
            [Role::Assistant, Role::Tool, Role::User, Role::Assistant]
        );
        assert_eq!(result.entries[1].is_error, Some(true));
        assert_eq!(texts(&result)[2], "deferred");
    }

    #[test]
    fn undo_stops_at_compaction_summary_and_skips_injections() {
        let records = [
            append("old"),
            record(
                json!({ "type": "context.apply_compaction", "summary": "SUM", "compactedCount": 1, "keptUserMessageCount": 1 }),
            ),
            append("recent"),
            record(
                json!({ "type": "context.append_message", "message": { "role": "user", "content": [{ "type": "text", "text": "injected" }], "toolCalls": [], "origin": { "kind": "injection", "variant": "x" } } }),
            ),
            record(
                json!({ "type": "context.append_message", "message": { "role": "assistant", "content": [{ "type": "text", "text": "answer" }], "toolCalls": [] } }),
            ),
            record(json!({ "type": "context.undo", "count": 2 })),
        ];
        let result = reduce_context_transcript(&records);
        assert_eq!(texts(&result), ["old", "SUM", "injected"]);
        assert_eq!(result.folded_length, 3);
    }

    #[test]
    fn clear_preserves_transcript_but_sets_undo_floor() {
        let records = [
            append("u1"),
            record(json!({ "type": "context.clear" })),
            append("u2"),
            record(
                json!({ "type": "context.append_message", "message": { "role": "assistant", "content": [], "toolCalls": [] } }),
            ),
            record(json!({ "type": "context.undo", "count": 1 })),
        ];
        let result = reduce_context_transcript(&records);
        assert_eq!(texts(&result), ["u1"]);
        assert_eq!(result.folded_length, 0);
    }

    #[test]
    fn reads_legacy_context_message_summary_text() {
        let records = [record(json!({
            "type": "context.apply_compaction",
            "summary": { "role": "assistant", "content": [
                { "type": "text", "text": "one" },
                { "type": "think", "think": "hidden" },
                { "type": "text", "text": "two" }
            ] }
        }))];
        assert_eq!(texts(&reduce_context_transcript(&records)), ["onetwo"]);
    }

    #[test]
    fn append_projection_drops_non_transcript_fields() {
        let records = [record(json!({
            "type": "context.append_message",
            "message": {
                "role": "user", "name": "name", "content": [], "toolCalls": [],
                "id": "id", "providerMessageId": "provider", "partial": true,
                "isError": true, "note": "note", "origin": { "kind": "user" }
            }
        }))];
        let message = &reduce_context_transcript(&records).entries[0];
        assert_eq!(message.id.as_deref(), Some("id"));
        assert_eq!(message.provider_message_id, None);
        assert_eq!(message.message.name, None);
        assert_eq!(message.message.partial, None);
        assert_eq!(message.is_error, Some(true));
        assert_eq!(message.note, None);
        assert_eq!(message.origin, Some(PromptOrigin::User));
    }
}
