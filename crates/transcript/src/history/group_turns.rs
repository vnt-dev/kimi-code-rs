//! Rebuild a turn tree from a flat persisted message list.
//!
//! Original:
//!   `packages/transcript/src/history/groupTurns.ts`
//!
//! This intentionally remains a best-effort cold path: one assistant message
//! becomes one completed step, approvals are absent, and turn ordinals are
//! assigned from zero because persisted messages carry no turn ids.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::{
    AttachmentId, AttachmentSource, FrameId, MarkerId, OptionalJsonValue, StepId, StepState,
    TaskId, TextFrame, TextRole, ThinkingFrame, ToolCallFrame, ToolFrameState,
    TranscriptAttachment, TranscriptFrame, TranscriptItem, TranscriptMarker, TranscriptStep,
    TranscriptTurn, TurnId, TurnOrigin, TurnState,
};
use crate::ops::AgentTranscriptSnapshot;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryMediaSource {
    Url { url: String },
    Base64 { media_type: String, data: String },
    File { file_id: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum HistoryContentPart {
    Text {
        text: String,
    },
    Think {
        think: String,
    },
    Image {
        source: HistoryMediaSource,
    },
    Video {
        source: HistoryMediaSource,
    },
    Audio {
        source: HistoryMediaSource,
    },
    File {
        file_id: String,
        name: String,
        media_type: String,
        size: u64,
    },
    Other {
        part_type: String,
    },
}

impl HistoryContentPart {
    fn media(&self) -> Option<(&'static str, &HistoryMediaSource)> {
        match self {
            Self::Image { source } => Some(("image", source)),
            Self::Video { source } => Some(("video", source)),
            Self::Audio { source } => Some(("audio", source)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryOrigin {
    pub kind: String,
    pub fields: IndexMap<String, Value>,
}

impl HistoryOrigin {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            fields: IndexMap::new(),
        }
    }

    pub fn with_field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.fields.insert(key.into(), value);
        self
    }

    fn string_field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }

    fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("kind".to_owned(), Value::String(self.kind.clone()));
        object.extend(
            self.fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        Value::Object(object)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryMessage {
    pub role: String,
    pub content: Option<Vec<HistoryContentPart>>,
    pub tool_calls: Option<Vec<HistoryToolCall>>,
    pub tool_call_id: Option<String>,
    pub is_error: Option<bool>,
    pub origin: Option<HistoryOrigin>,
}

impl HistoryMessage {
    pub fn new(role: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: None,
            tool_calls: None,
            tool_call_id: None,
            is_error: None,
            origin: None,
        }
    }
}

#[derive(Clone)]
struct TurnDraft {
    turn_id: TurnId,
    ordinal: i64,
    origin: TurnOrigin,
    prompt: Option<String>,
    attachment_ids: Option<Vec<AttachmentId>>,
    steps: Vec<StepDraft>,
}

#[derive(Clone)]
struct StepDraft {
    step_id: StepId,
    ordinal: i64,
    frames: Vec<TranscriptFrame>,
}

/// Cold-rebuild a materialized transcript snapshot.
pub fn group_messages_into_snapshot(messages: &[HistoryMessage]) -> AgentTranscriptSnapshot {
    let mut items = Vec::new();
    let mut attachments = Vec::new();
    let mut current_turn: Option<TurnDraft> = None;
    let mut next_ordinal = 0_i64;
    let mut marker_count = 0_i64;

    for message in messages {
        if message.role == "system" {
            continue;
        }
        let origin_kind = message.origin.as_ref().map(|origin| origin.kind.as_str());

        if message.role == "user" {
            if origin_kind.is_some_and(is_hidden_user_origin) {
                if opens_own_turn(message) {
                    start_turn(
                        &mut items,
                        &mut current_turn,
                        &mut next_ordinal,
                        map_origin(message),
                        None,
                        None,
                    );
                }
                continue;
            }

            if let Some(marker) = origin_kind.and_then(marker_for_origin) {
                marker_count += 1;
                items.push(TranscriptItem::Marker(TranscriptMarker {
                    marker_id: MarkerId::new(format!("m{marker_count}")),
                    marker: marker.to_owned(),
                    payload: Some(Some(json!({
                        "text": text_of(message),
                        "origin": message.origin.as_ref().map(HistoryOrigin::to_json)
                    }))),
                    at: None,
                }));
                if is_user_slash_prompt(message) {
                    start_turn(
                        &mut items,
                        &mut current_turn,
                        &mut next_ordinal,
                        map_origin(message),
                        Some(text_of(message)),
                        None,
                    );
                }
                continue;
            }

            let attachment_ids = collect_attachments(message, &mut attachments);
            start_turn(
                &mut items,
                &mut current_turn,
                &mut next_ordinal,
                map_origin(message),
                Some(text_of(message)),
                attachment_ids,
            );
            continue;
        }

        if message.role == "assistant" {
            let turn = ensure_turn(&mut items, &mut current_turn, &mut next_ordinal);
            let step_ordinal = turn.steps.len() as i64 + 1;
            let step_id = StepId::new(format!("{}.{}", turn.turn_id, step_ordinal));
            let mut step = StepDraft {
                step_id: step_id.clone(),
                ordinal: step_ordinal,
                frames: Vec::new(),
            };
            let mut frame_count = 0_i64;

            for part in message.content.as_deref().unwrap_or_default() {
                match part {
                    HistoryContentPart::Text { text } if !text.is_empty() => {
                        frame_count += 1;
                        step.frames.push(TranscriptFrame::Text(TextFrame {
                            frame_id: FrameId::new(format!("{step_id}.f{frame_count}")),
                            role: TextRole::Assistant,
                            text: text.clone(),
                            attachment_ids: None,
                            task_id: None,
                        }));
                    }
                    HistoryContentPart::Think { think } if !think.is_empty() => {
                        frame_count += 1;
                        step.frames.push(TranscriptFrame::Thinking(ThinkingFrame {
                            frame_id: FrameId::new(format!("{step_id}.f{frame_count}")),
                            text: think.clone(),
                        }));
                    }
                    _ => {}
                }
            }
            for call in message.tool_calls.as_deref().unwrap_or_default() {
                step.frames
                    .push(TranscriptFrame::Tool(Box::new(ToolCallFrame {
                        frame_id: FrameId::new(format!("{step_id}.{}", call.id)),
                        tool_call_id: call.id.clone(),
                        name: call.name.clone(),
                        view: None,
                        state: ToolFrameState::Running,
                        input: parse_arguments(call.arguments.as_deref()),
                        output: None,
                        display: None,
                        error: None,
                        task_id: None,
                        approval_id: None,
                        todo_id: None,
                        agent_refs: None,
                    })));
            }
            turn.steps.push(step);
            sync_turn_item(&mut items, turn);
            continue;
        }

        if message.role == "tool"
            && let (Some(turn), Some(tool_call_id)) =
                (current_turn.as_mut(), message.tool_call_id.as_deref())
            && let Some((step_index, frame_index)) = current_turn_tool_frame(turn, tool_call_id)
        {
            let output = text_of(message);
            let TranscriptFrame::Tool(frame) = &mut turn.steps[step_index].frames[frame_index]
            else {
                continue;
            };
            frame.state = if message.is_error.unwrap_or(false) {
                ToolFrameState::Error
            } else {
                ToolFrameState::Done
            };
            frame.output = Some(Some(Value::String(output.clone())));
            frame.error = message.is_error.unwrap_or(false).then_some(output);
            sync_turn_item(&mut items, turn);
        }
    }

    AgentTranscriptSnapshot {
        items,
        tasks: Vec::new(),
        interactions: Vec::new(),
        attachments,
        todos: Vec::new(),
        meta: Default::default(),
        has_more_older: None,
    }
}

fn collect_attachments(
    message: &HistoryMessage,
    attachments: &mut Vec<TranscriptAttachment>,
) -> Option<Vec<AttachmentId>> {
    let mut ids = Vec::new();
    for part in message.content.as_deref().unwrap_or_default() {
        let attachment = if let Some((media_kind, source)) = part.media() {
            let (media_type, source) = match source {
                HistoryMediaSource::Url { url } => (
                    format!("{media_kind}/*"),
                    Some(AttachmentSource::Url { url: url.clone() }),
                ),
                HistoryMediaSource::File { file_id } => (
                    format!("{media_kind}/*"),
                    Some(AttachmentSource::File {
                        file_id: file_id.clone(),
                    }),
                ),
                HistoryMediaSource::Base64 { media_type, .. } => (media_type.clone(), None),
            };
            Some(TranscriptAttachment {
                attachment_id: AttachmentId::new(format!("att_{}", attachments.len() + 1)),
                media_type,
                name: None,
                size: None,
                source,
                placeholder: None,
            })
        } else if let HistoryContentPart::File {
            file_id,
            name,
            media_type,
            size,
        } = part
        {
            Some(TranscriptAttachment {
                attachment_id: AttachmentId::new(format!("att_{}", attachments.len() + 1)),
                media_type: media_type.clone(),
                name: Some(name.clone()),
                size: Some(*size),
                source: Some(AttachmentSource::File {
                    file_id: file_id.clone(),
                }),
                placeholder: None,
            })
        } else {
            None
        };
        if let Some(attachment) = attachment {
            ids.push(attachment.attachment_id.clone());
            attachments.push(attachment);
        }
    }
    (!ids.is_empty()).then_some(ids)
}

fn ensure_turn<'a>(
    items: &mut Vec<TranscriptItem>,
    current_turn: &'a mut Option<TurnDraft>,
    next_ordinal: &mut i64,
) -> &'a mut TurnDraft {
    current_turn.get_or_insert_with(|| {
        let ordinal = *next_ordinal;
        *next_ordinal += 1;
        let draft = TurnDraft {
            turn_id: TurnId::new(format!("t{ordinal}")),
            ordinal,
            origin: TurnOrigin::other(),
            prompt: None,
            attachment_ids: None,
            steps: Vec::new(),
        };
        items.push(draft_to_turn_item(&draft));
        draft
    })
}

fn start_turn(
    items: &mut Vec<TranscriptItem>,
    current_turn: &mut Option<TurnDraft>,
    next_ordinal: &mut i64,
    origin: TurnOrigin,
    prompt: Option<String>,
    attachment_ids: Option<Vec<AttachmentId>>,
) {
    let ordinal = *next_ordinal;
    *next_ordinal += 1;
    let draft = TurnDraft {
        turn_id: TurnId::new(format!("t{ordinal}")),
        ordinal,
        origin,
        prompt,
        attachment_ids,
        steps: Vec::new(),
    };
    items.push(draft_to_turn_item(&draft));
    *current_turn = Some(draft);
}

fn is_hidden_user_origin(kind: &str) -> bool {
    matches!(kind, "injection" | "system_trigger" | "retry")
}

fn marker_for_origin(kind: &str) -> Option<&'static str> {
    match kind {
        "skill_activation" | "plugin_command" => Some("skill"),
        "compaction_summary" => Some("compaction"),
        _ => None,
    }
}

fn opens_own_turn(message: &HistoryMessage) -> bool {
    message.origin.as_ref().is_some_and(|origin| {
        origin.kind == "system_trigger"
            && matches!(
                origin.string_field("name"),
                Some("goal_continuation" | "subagent")
            )
    })
}

fn is_user_slash_prompt(message: &HistoryMessage) -> bool {
    message.origin.as_ref().is_some_and(|origin| {
        matches!(origin.kind.as_str(), "skill_activation" | "plugin_command")
            && origin.string_field("trigger") == Some("user-slash")
    })
}

fn map_origin(message: &HistoryMessage) -> TurnOrigin {
    let Some(origin) = &message.origin else {
        return TurnOrigin::User { payload: None };
    };
    match origin.kind.as_str() {
        "cron_job" | "cron_missed" => TurnOrigin::Cron {
            task_id: origin.string_field("jobId").map(TaskId::from),
            payload: json_payload(origin),
        },
        "task" | "background_task" => {
            if let Some(task_id) = origin.string_field("taskId") {
                TurnOrigin::Task {
                    task_id: TaskId::from(task_id),
                    payload: json_payload(origin),
                }
            } else {
                TurnOrigin::Other {
                    payload: json_payload(origin),
                }
            }
        }
        "hook_result" => TurnOrigin::Hook {
            payload: json_payload(origin),
        },
        "shell_command" => TurnOrigin::User {
            payload: json_payload(origin),
        },
        "user" => TurnOrigin::User { payload: None },
        _ => TurnOrigin::Other {
            payload: json_payload(origin),
        },
    }
}

fn json_payload(origin: &HistoryOrigin) -> OptionalJsonValue {
    Some(Some(origin.to_json()))
}

fn text_of(message: &HistoryMessage) -> String {
    message
        .content
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|part| match part {
            HistoryContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn parse_arguments(raw: Option<&str>) -> OptionalJsonValue {
    let raw = raw.filter(|raw| !raw.is_empty())?;
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Null) => Some(None),
        Ok(value) => Some(Some(value)),
        Err(_) => Some(Some(Value::String(raw.to_owned()))),
    }
}

fn draft_to_turn_item(draft: &TurnDraft) -> TranscriptItem {
    TranscriptItem::Turn(TranscriptTurn {
        turn_id: draft.turn_id.clone(),
        ordinal: draft.ordinal,
        state: TurnState::Completed,
        origin: draft.origin.clone(),
        prompt: draft.prompt.clone(),
        attachment_ids: draft.attachment_ids.clone(),
        steps: draft
            .steps
            .iter()
            .map(|step| TranscriptStep {
                step_id: step.step_id.clone(),
                turn_id: draft.turn_id.clone(),
                ordinal: step.ordinal,
                state: StepState::Completed,
                frames: step.frames.clone(),
                started_at: None,
                ended_at: None,
            })
            .collect(),
        started_at: None,
        ended_at: None,
        usage: None,
    })
}

fn sync_turn_item(items: &mut [TranscriptItem], draft: &TurnDraft) {
    if let Some(index) = items.iter().position(
        |item| matches!(item, TranscriptItem::Turn(turn) if turn.turn_id == draft.turn_id),
    ) {
        items[index] = draft_to_turn_item(draft);
    }
}

fn current_turn_tool_frame(turn: &TurnDraft, tool_call_id: &str) -> Option<(usize, usize)> {
    for (step_index, step) in turn.steps.iter().enumerate().rev() {
        for (frame_index, frame) in step.frames.iter().enumerate().rev() {
            if matches!(
                frame,
                TranscriptFrame::Tool(frame) if frame.tool_call_id == tool_call_id
            ) {
                return Some((step_index, frame_index));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> HistoryContentPart {
        HistoryContentPart::Text {
            text: text.to_owned(),
        }
    }

    fn message(role: &str, content: Vec<HistoryContentPart>) -> HistoryMessage {
        HistoryMessage {
            role: role.to_owned(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
            is_error: None,
            origin: None,
        }
    }

    fn user(text_value: &str, origin: HistoryOrigin) -> HistoryMessage {
        let mut message = message("user", vec![text(text_value)]);
        message.origin = Some(origin);
        message
    }

    #[test]
    fn groups_turns_and_folds_persisted_tool_results() {
        let mut assistant = message(
            "assistant",
            vec![
                HistoryContentPart::Think {
                    think: "hmm".to_owned(),
                },
                text("checking"),
            ],
        );
        assistant.tool_calls = Some(vec![HistoryToolCall {
            id: "c1".to_owned(),
            name: "Read".to_owned(),
            arguments: Some("{\"path\":\"/a\"}".to_owned()),
        }]);
        let mut tool = message("tool", vec![text("file body")]);
        tool.tool_call_id = Some("c1".to_owned());

        let snapshot = group_messages_into_snapshot(&[
            message("system", vec![text("hidden")]),
            user("hello", HistoryOrigin::new("user")),
            assistant,
            tool,
            message("assistant", vec![text("done")]),
            user("next", HistoryOrigin::new("user")),
            user("summary", HistoryOrigin::new("compaction_summary")),
            user("after", HistoryOrigin::new("user")),
        ]);
        assert_eq!(
            snapshot
                .items
                .iter()
                .map(|item| match item {
                    TranscriptItem::Turn(_) => "turn",
                    TranscriptItem::Marker(_) => "marker",
                    TranscriptItem::TaskRef(_) => "taskref",
                })
                .collect::<Vec<_>>(),
            ["turn", "turn", "marker", "turn"]
        );
        let TranscriptItem::Turn(first) = &snapshot.items[0] else {
            panic!("expected turn");
        };
        assert_eq!(first.steps.len(), 2);
        let tool = first.steps[0]
            .frames
            .iter()
            .find_map(|frame| match frame {
                TranscriptFrame::Tool(frame) => Some(frame),
                _ => None,
            })
            .unwrap();
        assert_eq!(tool.state, ToolFrameState::Done);
        assert_eq!(
            tool.output,
            Some(Some(Value::String("file body".to_owned())))
        );
        assert_eq!(tool.input, Some(Some(json!({"path": "/a"}))));
    }

    #[test]
    fn extracts_media_metadata_without_base64_bytes() {
        let mut opening = user("what?", HistoryOrigin::new("user"));
        opening.content = Some(vec![
            text("what?"),
            HistoryContentPart::Image {
                source: HistoryMediaSource::Base64 {
                    media_type: "image/png".to_owned(),
                    data: "secret-bytes".to_owned(),
                },
            },
            HistoryContentPart::Image {
                source: HistoryMediaSource::Url {
                    url: "https://example.test/pic.png".to_owned(),
                },
            },
            HistoryContentPart::File {
                file_id: "file-9".to_owned(),
                name: "notes.txt".to_owned(),
                media_type: "text/plain".to_owned(),
                size: 128,
            },
        ]);
        let snapshot = group_messages_into_snapshot(&[opening]);
        assert_eq!(snapshot.attachments.len(), 3);
        assert_eq!(snapshot.attachments[0].media_type, "image/png");
        assert!(snapshot.attachments[0].source.is_none());
        assert!(matches!(
            snapshot.attachments[1].source,
            Some(AttachmentSource::Url { .. })
        ));
        assert_eq!(snapshot.attachments[2].name.as_deref(), Some("notes.txt"));
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("secret-bytes")
        );
        let TranscriptItem::Turn(turn) = &snapshot.items[0] else {
            panic!("expected turn");
        };
        assert_eq!(turn.attachment_ids.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn hidden_triggers_and_skill_activation_preserve_turn_boundaries() {
        let slash =
            HistoryOrigin::new("skill_activation").with_field("trigger", json!("user-slash"));
        let nested =
            HistoryOrigin::new("skill_activation").with_field("trigger", json!("model-tool"));
        let continuation =
            HistoryOrigin::new("system_trigger").with_field("name", json!("goal_continuation"));
        let snapshot = group_messages_into_snapshot(&[
            user("hi", HistoryOrigin::new("user")),
            message("assistant", vec![text("first")]),
            user("continue internally", continuation),
            message("assistant", vec![text("continued")]),
            user("skill body", slash),
            message("assistant", vec![text("skill result")]),
            user("nested", nested),
            message("assistant", vec![text("same turn")]),
            user("hidden", HistoryOrigin::new("injection")),
            message("assistant", vec![text("still same")]),
        ]);
        let turns: Vec<_> = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                TranscriptItem::Turn(turn) => Some(turn),
                _ => None,
            })
            .collect();
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].ordinal, 0);
        assert_eq!(turns[1].ordinal, 1);
        assert!(turns[1].prompt.is_none());
        assert_eq!(turns[2].prompt.as_deref(), Some("skill body"));
        assert_eq!(turns[2].steps.len(), 3);
        assert_eq!(
            snapshot
                .items
                .iter()
                .filter(|item| matches!(item, TranscriptItem::Marker(_)))
                .count(),
            2
        );
    }

    #[test]
    fn maps_cron_task_hook_and_shell_origins() {
        let snapshot = group_messages_into_snapshot(&[
            user(
                "cron",
                HistoryOrigin::new("cron_job").with_field("jobId", json!("job-1")),
            ),
            user(
                "task",
                HistoryOrigin::new("background_task").with_field("taskId", json!("task-1")),
            ),
            user("hook", HistoryOrigin::new("hook_result")),
            user("shell", HistoryOrigin::new("shell_command")),
        ]);
        let origins: Vec<_> = snapshot
            .items
            .iter()
            .filter_map(|item| match item {
                TranscriptItem::Turn(turn) => Some(&turn.origin),
                _ => None,
            })
            .collect();
        assert!(matches!(
            origins[0],
            TurnOrigin::Cron { task_id: Some(id), .. } if id.as_ref() == "job-1"
        ));
        assert!(matches!(
            origins[1],
            TurnOrigin::Task { task_id, .. } if task_id.as_ref() == "task-1"
        ));
        assert!(matches!(origins[2], TurnOrigin::Hook { .. }));
        assert!(matches!(origins[3], TurnOrigin::User { payload: Some(_) }));
    }

    #[test]
    fn invalid_tool_arguments_fall_back_to_raw_string_and_errors_are_folded() {
        let mut assistant = message("assistant", Vec::new());
        assistant.tool_calls = Some(vec![HistoryToolCall {
            id: "c1".to_owned(),
            name: "Bash".to_owned(),
            arguments: Some("{bad".to_owned()),
        }]);
        let mut tool = message("tool", vec![text("failed")]);
        tool.tool_call_id = Some("c1".to_owned());
        tool.is_error = Some(true);
        let snapshot = group_messages_into_snapshot(&[assistant, tool]);
        let TranscriptItem::Turn(turn) = &snapshot.items[0] else {
            panic!("expected turn");
        };
        let TranscriptFrame::Tool(frame) = &turn.steps[0].frames[0] else {
            panic!("expected tool");
        };
        assert_eq!(frame.input, Some(Some(Value::String("{bad".to_owned()))));
        assert_eq!(frame.state, ToolFrameState::Error);
        assert_eq!(frame.error.as_deref(), Some("failed"));
    }
}
