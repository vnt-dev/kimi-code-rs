use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::{
    agent::swarm::SwarmExitPayload,
    wire::{
        model::{
            ModelBlobCodec, ModelCrossReducer, ModelDef, ModelOptions, PartsTransformer,
            define_model,
        },
        op::{DefineOpOptions, DefinedOp, Op},
        record::WireRecord,
    },
};

use super::{
    compaction_handoff::{
        ContextCompactionShapeInput, build_context_compaction_shape,
        create_compaction_summary_message,
    },
    loop_event_fold::{LoopEventFold, LoopRecordedEvent, LoopToolResultOutput},
    types::{ContextMessage, PromptOrigin},
    undo::{compute_undo_cut, is_fully_undoable},
};

pub type ContextMemoryState = LoopEventFold;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextAppendMessagePayload {
    pub message: ContextMessage,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextAppendLoopEventPayload {
    pub event: LoopRecordedEvent,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContextClearPayload {}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextUndoPayload {
    #[serde(deserialize_with = "kimi_code_protocol::lenient::lenient_u32")]
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionPayload {
    fields: Map<String, Value>,
    shape: ContextCompactionShapeInput,
}

impl Serialize for ContextCompactionPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.fields.serialize(serializer)
    }
}

struct ContextBlobCodec;

pub static CONTEXT_MODEL: LazyLock<ModelDef<ContextMemoryState>> = LazyLock::new(|| {
    define_model(
        "contextMemory",
        LoopEventFold::default,
        ModelOptions {
            blobs: Some(Arc::new(ContextBlobCodec)),
            reducers: vec![ModelCrossReducer::typed(
                "swarm_mode.exit",
                pop_swarm_mode_reminder,
            )],
        },
    )
});

pub static CONTEXT_APPEND_MESSAGE: LazyLock<
    DefinedOp<ContextMemoryState, ContextAppendMessagePayload>,
> = LazyLock::new(|| {
    CONTEXT_MODEL
        .define_op(
            "context.append_message",
            DefineOpOptions::new(
                |mut state: ContextMemoryState, payload: &ContextAppendMessagePayload| {
                    state.fold_append_message(payload.message.clone());
                    state
                },
            ),
        )
        .expect("context.append_message must have one global definition")
});

pub static CONTEXT_APPEND_LOOP_EVENT: LazyLock<
    DefinedOp<ContextMemoryState, ContextAppendLoopEventPayload>,
> = LazyLock::new(|| {
    CONTEXT_MODEL
        .define_op(
            "context.append_loop_event",
            DefineOpOptions::new(
                |mut state: ContextMemoryState, payload: &ContextAppendLoopEventPayload| {
                    state.fold_loop_event(payload.event.clone());
                    state
                },
            ),
        )
        .expect("context.append_loop_event must have one global definition")
});

pub static CONTEXT_CLEAR: LazyLock<DefinedOp<ContextMemoryState, ContextClearPayload>> =
    LazyLock::new(|| {
        CONTEXT_MODEL
            .define_op(
                "context.clear",
                DefineOpOptions::new(
                    |state: ContextMemoryState, _payload: &ContextClearPayload| {
                        if state.messages().is_empty() {
                            state
                        } else {
                            LoopEventFold::default()
                        }
                    },
                ),
            )
            .expect("context.clear must have one global definition")
    });

pub static CONTEXT_APPLY_COMPACTION: LazyLock<
    DefinedOp<ContextMemoryState, ContextCompactionPayload>,
> = LazyLock::new(|| {
    let options = DefineOpOptions {
        parse_payload: Arc::new(parse_context_compaction_payload),
        apply: Arc::new(
            |state: ContextMemoryState, payload: &ContextCompactionPayload| {
                let result =
                    build_context_compaction_shape(state.messages(), payload.shape.clone());
                LoopEventFold::new(result.messages)
            },
        ),
        validate_apply: None,
        to_event: None,
        persist: None,
    };
    CONTEXT_MODEL
        .define_op("context.apply_compaction", options)
        .expect("context.apply_compaction must have one global definition")
});

pub static CONTEXT_UNDO: LazyLock<DefinedOp<ContextMemoryState, ContextUndoPayload>> =
    LazyLock::new(|| {
        CONTEXT_MODEL
            .define_op(
                "context.undo",
                DefineOpOptions::new(|state: ContextMemoryState, payload: &ContextUndoPayload| {
                    if payload.count == 0 || state.messages().is_empty() {
                        return state;
                    }
                    let cut = compute_undo_cut(state.messages(), payload.count);
                    if !is_fully_undoable(cut, payload.count) {
                        return state;
                    }
                    let cut_index = usize::try_from(cut.cut_index).unwrap_or(0);
                    LoopEventFold::new(state.messages()[..cut_index].to_vec())
                }),
            )
            .expect("context.undo must have one global definition")
    });

// Original: contextOps.ts, popSwarmModeReminder().
fn pop_swarm_mode_reminder(
    mut state: ContextMemoryState,
    _payload: &SwarmExitPayload,
) -> ContextMemoryState {
    let should_pop = state.messages().last().is_some_and(|message| {
        matches!(
            &message.origin,
            Some(PromptOrigin::Injection { variant }) if variant == "swarm_mode"
        )
    });
    if should_pop {
        state.pop_message();
        state.reset_fold();
    }
    state
}

pub fn context_append_message(message: ContextMessage) -> Result<Op, serde_json::Error> {
    CONTEXT_APPEND_MESSAGE.create(ContextAppendMessagePayload { message })
}

pub fn context_append_loop_event(event: LoopRecordedEvent) -> Result<Op, serde_json::Error> {
    CONTEXT_APPEND_LOOP_EVENT.create(ContextAppendLoopEventPayload { event })
}

pub fn context_clear() -> Result<Op, serde_json::Error> {
    CONTEXT_CLEAR.create(ContextClearPayload {})
}

pub fn context_undo(count: u32) -> Result<Op, serde_json::Error> {
    CONTEXT_UNDO.create(ContextUndoPayload { count })
}

pub fn context_apply_compaction(
    input: ContextCompactionShapeInput,
) -> Result<Op, serde_json::Error> {
    let fields = compaction_shape_input_to_fields(&input)?;
    CONTEXT_APPLY_COMPACTION.create(ContextCompactionPayload {
        fields,
        shape: input,
    })
}

pub fn apply_context_compaction_record(
    state: &[ContextMessage],
    record: &Map<String, Value>,
) -> Result<Vec<ContextMessage>, ContextCompactionRecordError> {
    let input = read_context_compaction_shape_input(record)?;
    Ok(build_context_compaction_shape(state, input).messages)
}

// Original: contextOps.ts, readContextCompactionShapeInput().
pub fn read_context_compaction_shape_input(
    record: &Map<String, Value>,
) -> Result<ContextCompactionShapeInput, ContextCompactionRecordError> {
    let kept_user_message_count = read_optional_number(record, "keptUserMessageCount");
    Ok(ContextCompactionShapeInput {
        summary: read_context_compaction_raw_summary(record)?,
        legacy_summary_message: read_legacy_summary_message(record),
        context_summary: read_optional_string(record, "contextSummary"),
        compacted_count: read_context_compacted_count(record)?,
        tokens_before: read_optional_number(record, "tokensBefore").unwrap_or(0),
        tokens_after: read_optional_number(record, "tokensAfter"),
        kept_user_message_count,
        kept_head_user_message_count: read_optional_number(record, "keptHeadUserMessageCount"),
        dropped_count: read_optional_number(record, "droppedCount"),
        legacy_tail: read_optional_boolean(record, "legacyTail")
            .or(Some(kept_user_message_count.is_none())),
    })
}

pub fn read_context_compacted_count(
    record: &Map<String, Value>,
) -> Result<u64, ContextCompactionRecordError> {
    record
        .get("compactedCount")
        .and_then(Value::as_f64)
        .or_else(|| record.get("count").and_then(Value::as_f64))
        .map(|value| value as u64)
        .ok_or(ContextCompactionRecordError::MissingCompactedCount)
}

pub fn read_context_compaction_summary(
    record: &Map<String, Value>,
) -> Result<ContextMessage, ContextCompactionRecordError> {
    if let Some(summary) = record.get("contextSummary").and_then(Value::as_str) {
        return Ok(create_compaction_summary_message(summary));
    }
    if let Some(summary) = record.get("summary").and_then(Value::as_str) {
        return Ok(create_compaction_summary_message(summary));
    }
    read_legacy_summary_message(record).ok_or(ContextCompactionRecordError::MissingSummary)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ContextCompactionRecordError {
    #[error("Invalid context.apply_compaction record: missing compactedCount")]
    MissingCompactedCount,
    #[error("Invalid context.apply_compaction record: missing summary")]
    MissingSummary,
    #[error("Invalid context.apply_compaction payload")]
    InvalidPayload,
}

fn parse_context_compaction_payload(value: &Value) -> Result<ContextCompactionPayload, String> {
    let fields = value
        .as_object()
        .ok_or_else(|| ContextCompactionRecordError::InvalidPayload.to_string())?;
    validate_compaction_payload(fields).map_err(|error| error.to_string())?;
    let shape = read_context_compaction_shape_input(fields).map_err(|error| error.to_string())?;
    Ok(ContextCompactionPayload {
        fields: strip_compaction_unknown_fields(fields),
        shape,
    })
}

fn validate_compaction_payload(
    fields: &Map<String, Value>,
) -> Result<(), ContextCompactionRecordError> {
    for key in [
        "tokensBefore",
        "tokensAfter",
        "keptUserMessageCount",
        "keptHeadUserMessageCount",
        "droppedCount",
    ] {
        if fields.get(key).is_some_and(|value| !value.is_number()) {
            return Err(ContextCompactionRecordError::InvalidPayload);
        }
    }
    if fields
        .get("legacyTail")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(ContextCompactionRecordError::InvalidPayload);
    }
    let compacted = fields.get("compactedCount");
    let summary = fields.get("summary");
    let context_summary = fields.get("contextSummary");
    let current_summary = summary.is_some_and(Value::is_string)
        && compacted.is_some_and(Value::is_number)
        && context_summary.is_none_or(Value::is_string);
    let current_context = context_summary.is_some_and(Value::is_string)
        && compacted.is_some_and(Value::is_number)
        && summary.is_none_or(Value::is_string);
    let legacy = summary
        .and_then(|value| serde_json::from_value::<ContextMessage>(value.clone()).ok())
        .is_some()
        && fields.get("count").is_some_and(Value::is_number)
        && compacted.is_none_or(Value::is_number);
    if current_summary || current_context || legacy {
        Ok(())
    } else {
        Err(ContextCompactionRecordError::InvalidPayload)
    }
}

fn strip_compaction_unknown_fields(fields: &Map<String, Value>) -> Map<String, Value> {
    const KNOWN: &[&str] = &[
        "tokensBefore",
        "tokensAfter",
        "keptUserMessageCount",
        "keptHeadUserMessageCount",
        "droppedCount",
        "legacyTail",
        "summary",
        "contextSummary",
        "compactedCount",
        "count",
    ];
    fields
        .iter()
        .filter(|(key, _)| KNOWN.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn compaction_shape_input_to_fields(
    input: &ContextCompactionShapeInput,
) -> Result<Map<String, Value>, serde_json::Error> {
    let mut fields = Map::new();
    fields.insert("summary".into(), Value::String(input.summary.clone()));
    fields.insert(
        "compactedCount".into(),
        serde_json::to_value(input.compacted_count)?,
    );
    fields.insert(
        "tokensBefore".into(),
        serde_json::to_value(input.tokens_before)?,
    );
    for (key, value) in [
        ("tokensAfter", input.tokens_after),
        ("keptUserMessageCount", input.kept_user_message_count),
        (
            "keptHeadUserMessageCount",
            input.kept_head_user_message_count,
        ),
        ("droppedCount", input.dropped_count),
    ] {
        if let Some(value) = value {
            fields.insert(key.into(), serde_json::to_value(value)?);
        }
    }
    if let Some(value) = &input.context_summary {
        fields.insert("contextSummary".into(), Value::String(value.clone()));
    }
    if let Some(value) = input.legacy_tail {
        fields.insert("legacyTail".into(), Value::Bool(value));
    }
    Ok(fields)
}

fn read_context_compaction_raw_summary(
    record: &Map<String, Value>,
) -> Result<String, ContextCompactionRecordError> {
    if let Some(summary) = record.get("summary").and_then(Value::as_str) {
        return Ok(summary.to_owned());
    }
    if let Some(summary) = record.get("contextSummary").and_then(Value::as_str) {
        return Ok(summary.to_owned());
    }
    read_legacy_summary_message(record)
        .map(|message| text_of(&message))
        .ok_or(ContextCompactionRecordError::MissingSummary)
}

fn read_legacy_summary_message(record: &Map<String, Value>) -> Option<ContextMessage> {
    serde_json::from_value(record.get("summary")?.clone()).ok()
}

fn read_optional_number(record: &Map<String, Value>, key: &str) -> Option<u64> {
    record
        .get(key)
        .and_then(Value::as_f64)
        .map(|value| value as u64)
}

fn read_optional_string(record: &Map<String, Value>, key: &str) -> Option<String> {
    record.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn read_optional_boolean(record: &Map<String, Value>, key: &str) -> Option<bool> {
    record.get(key).and_then(Value::as_bool)
}

fn text_of(message: &ContextMessage) -> String {
    message
        .message
        .content
        .iter()
        .filter_map(|part| match part {
            crate::kosong::contract::message::ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[async_trait]
impl ModelBlobCodec<ContextMemoryState> for ContextBlobCodec {
    // Original: contextOps.ts, dehydrateRecord().
    async fn dehydrate(
        &self,
        mut record: WireRecord,
        transform: &dyn PartsTransformer,
    ) -> Result<WireRecord, String> {
        match record.get("type").and_then(Value::as_str) {
            Some("context.append_message") => {
                let Some(value) = record.get("message").cloned() else {
                    return Ok(record);
                };
                let Ok(mut message) = serde_json::from_value::<ContextMessage>(value) else {
                    return Ok(record);
                };
                if transform_message_content(&mut message, transform).await? {
                    record.insert(
                        "message".into(),
                        serde_json::to_value(message).map_err(|error| error.to_string())?,
                    );
                }
            }
            Some("context.append_loop_event") => {
                let Some(value) = record.get("event").cloned() else {
                    return Ok(record);
                };
                let Ok(mut event) = serde_json::from_value::<LoopRecordedEvent>(value) else {
                    return Ok(record);
                };
                if transform_loop_event(&mut event, transform).await? {
                    record.insert(
                        "event".into(),
                        serde_json::to_value(event).map_err(|error| error.to_string())?,
                    );
                }
            }
            _ => {}
        }
        Ok(record)
    }

    // Original: contextOps.ts, dehydrateMessages()/ContextModel.blobs.rehydrate().
    async fn rehydrate(
        &self,
        mut state: ContextMemoryState,
        transform: &dyn PartsTransformer,
    ) -> Result<ContextMemoryState, String> {
        for message in state.messages_mut() {
            transform_message_content(message, transform).await?;
        }
        Ok(state)
    }
}

async fn transform_message_content(
    message: &mut ContextMessage,
    transform: &dyn PartsTransformer,
) -> Result<bool, String> {
    let original = serde_json::to_value(&message.message.content)
        .map_err(|error| error.to_string())?
        .as_array()
        .cloned()
        .unwrap_or_default();
    let transformed = transform.transform(original.clone()).await?;
    if transformed == original {
        return Ok(false);
    }
    let content =
        serde_json::from_value(Value::Array(transformed)).map_err(|error| error.to_string())?;
    let mut replaced = message.clone();
    replaced.message.content = content;
    *message = replaced;
    Ok(true)
}

async fn transform_loop_event(
    event: &mut LoopRecordedEvent,
    transform: &dyn PartsTransformer,
) -> Result<bool, String> {
    match event {
        LoopRecordedEvent::ContentPart { part, .. } => {
            let original = serde_json::to_value(&*part).map_err(|error| error.to_string())?;
            let transformed = transform.transform(vec![original.clone()]).await?;
            if transformed.first() == Some(&original) {
                return Ok(false);
            }
            let first = transformed
                .into_iter()
                .next()
                .ok_or_else(|| "content part transformer removed the event part".to_owned())?;
            *part = serde_json::from_value(first).map_err(|error| error.to_string())?;
            Ok(true)
        }
        LoopRecordedEvent::ToolResult { result, .. } => {
            let LoopToolResultOutput::Parts(parts) = &mut result.output else {
                return Ok(false);
            };
            let original = serde_json::to_value(&*parts)
                .map_err(|error| error.to_string())?
                .as_array()
                .cloned()
                .unwrap_or_default();
            let transformed = transform.transform(original.clone()).await?;
            if transformed == original {
                return Ok(false);
            }
            *parts = serde_json::from_value(Value::Array(transformed))
                .map_err(|error| error.to_string())?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::context_memory::types::PromptOrigin,
        kosong::contract::message::{ContentPart, MediaUrl, Message, Role},
        wire::{model::model_cross_reducers, op::ErasedOpDescriptor, record::op_to_wire_record_at},
    };

    struct ReplaceUrl {
        from: &'static str,
        to: &'static str,
    }

    #[async_trait]
    impl PartsTransformer for ReplaceUrl {
        async fn transform(&self, mut parts: Vec<Value>) -> Result<Vec<Value>, String> {
            for part in &mut parts {
                if part.pointer("/imageUrl/url").and_then(Value::as_str) == Some(self.from) {
                    part["imageUrl"]["url"] = Value::String(self.to.into());
                }
            }
            Ok(parts)
        }
    }

    fn user(text: &str) -> ContextMessage {
        ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text { text: text.into() }],
                Vec::new(),
            ),
            id: None,
            provider_message_id: None,
            origin: Some(PromptOrigin::User),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn ops_preserve_flat_payloads_and_fold_state() {
        let append = context_append_message(user("hello")).unwrap();
        assert_eq!(append.payload_value["message"]["role"], "user");
        let state = CONTEXT_APPEND_MESSAGE
            .descriptor()
            .apply(Box::new(CONTEXT_MODEL.initial()), append.payload())
            .unwrap()
            .downcast::<ContextMemoryState>()
            .unwrap();
        assert_eq!(state.messages().len(), 1);

        let undo = context_undo(1).unwrap();
        let state = CONTEXT_UNDO
            .descriptor()
            .apply(state, undo.payload())
            .unwrap()
            .downcast::<ContextMemoryState>()
            .unwrap();
        assert!(state.messages().is_empty());
    }

    #[test]
    fn parses_current_and_legacy_compaction_records() {
        let current = serde_json::json!({
            "summary": "sum",
            "compactedCount": 1.5,
            "tokensBefore": 4,
            "unknown": true
        });
        let payload = parse_context_compaction_payload(&current).unwrap();
        assert_eq!(payload.shape.summary, "sum");
        assert_eq!(payload.shape.compacted_count, 1);
        assert!(!payload.fields.contains_key("unknown"));

        let legacy_message = user("old summary");
        let legacy = serde_json::json!({
            "summary": legacy_message,
            "count": 2,
        });
        let payload = parse_context_compaction_payload(&legacy).unwrap();
        assert_eq!(payload.shape.summary, "old summary");
        assert_eq!(payload.shape.compacted_count, 2);
        assert_eq!(payload.shape.legacy_tail, Some(true));
    }

    #[test]
    fn swarm_exit_reducer_pops_only_matching_last_injection() {
        LazyLock::force(&CONTEXT_MODEL);
        let reducers = model_cross_reducers("swarm_mode.exit");
        let reducer = reducers
            .iter()
            .find(|entry| entry.model.id() == CONTEXT_MODEL.id())
            .unwrap();
        let mut reminder = user("reminder");
        reminder.origin = Some(PromptOrigin::Injection {
            variant: "swarm_mode".into(),
        });
        let state = reducer
            .apply(
                Box::new(LoopEventFold::new(vec![user("keep"), reminder])),
                &SwarmExitPayload {},
            )
            .unwrap()
            .downcast::<ContextMemoryState>()
            .unwrap();
        assert_eq!(state.messages().len(), 1);
    }

    #[tokio::test]
    async fn blob_codec_transforms_persisted_records_and_surviving_state() {
        let mut message = user("image");
        message.message.content = vec![ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: "data:image/png;base64,AAAA".into(),
                id: None,
            },
        }];
        let op = context_append_message(message.clone()).unwrap();
        let record = op_to_wire_record_at(&op, 1);
        let dehydrated = ContextBlobCodec
            .dehydrate(
                record,
                &ReplaceUrl {
                    from: "data:image/png;base64,AAAA",
                    to: "blobref:image-1",
                },
            )
            .await
            .unwrap();
        assert_eq!(
            dehydrated["message"]["content"][0]["imageUrl"]["url"],
            "blobref:image-1"
        );

        message.message.content = vec![ContentPart::ImageUrl {
            image_url: MediaUrl {
                url: "blobref:image-1".into(),
                id: None,
            },
        }];
        let state = ContextBlobCodec
            .rehydrate(
                LoopEventFold::new(vec![message]),
                &ReplaceUrl {
                    from: "blobref:image-1",
                    to: "data:image/png;base64,AAAA",
                },
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&state.messages()[0].message.content[0]).unwrap()["imageUrl"]["url"],
            "data:image/png;base64,AAAA"
        );
    }
}
