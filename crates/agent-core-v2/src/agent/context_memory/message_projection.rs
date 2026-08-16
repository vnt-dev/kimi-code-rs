use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};

use crate::kosong::contract::message::{ContentPart, Role, content_text_parts};

use super::{
    protocol_message::{ImageSource, MessageContent, MessageRole, ProtocolMessage},
    types::ContextMessage,
};

#[derive(Debug, thiserror::Error)]
pub enum MessageProjectionError {
    #[error("message timestamp is outside the supported ISO date range")]
    InvalidTimestamp,
    #[error("failed to serialize projected message content")]
    Serialize(#[from] serde_json::Error),
}

// Original: messageProjection.ts, deriveMessageId().
fn derive_message_id(session_id: &str, index: u64) -> String {
    format!("msg_{session_id}_{index:06}")
}

// Original: messageProjection.ts, toProtocolRole().
fn to_protocol_role(role: Role) -> MessageRole {
    match role {
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
        Role::Tool => MessageRole::Tool,
        Role::System => MessageRole::System,
    }
}

// Original: messageProjection.ts, mapContentPart().
fn map_content_part(part: &ContentPart) -> MessageContent {
    match part {
        ContentPart::Text { text } => MessageContent::Text { text: text.clone() },
        ContentPart::Think { think, encrypted } => MessageContent::Thinking {
            thinking: think.clone(),
            signature: encrypted.clone(),
        },
        ContentPart::ImageUrl { image_url } => MessageContent::Image {
            source: ImageSource::Url {
                url: image_url.url.clone(),
            },
        },
        ContentPart::AudioUrl { audio_url } => MessageContent::Text {
            text: format!("[audio:{}]", audio_url.url),
        },
        ContentPart::VideoUrl { video_url } => MessageContent::Video {
            source: ImageSource::Url {
                url: video_url.url.clone(),
            },
        },
    }
}

fn map_message_content(message: &ContextMessage, part: &ContentPart) -> MessageContent {
    if let ContentPart::Text { text } = part
        && let Some(file) = message
            .attachments
            .iter()
            .find(|attachment| attachment.model_text == *text)
    {
        return MessageContent::File {
            file_id: file.file_id.clone(),
            name: file.name.clone(),
            media_type: file.media_type.clone(),
            size: file.size,
        };
    }
    map_content_part(part)
}

// Original: messageProjection.ts, buildProtocolContent().
pub(crate) fn to_protocol_message_content(
    message: &ContextMessage,
) -> Result<Vec<MessageContent>, serde_json::Error> {
    if message.message.role == Role::Tool {
        let Some(tool_call_id) = &message.message.tool_call_id else {
            return Ok(message
                .message
                .content
                .iter()
                .map(|part| map_message_content(message, part))
                .collect());
        };
        let has_media_part = message.message.content.iter().any(|part| {
            matches!(
                part,
                ContentPart::ImageUrl { .. }
                    | ContentPart::VideoUrl { .. }
                    | ContentPart::AudioUrl { .. }
            )
        });
        let output = if has_media_part {
            serde_json::to_value(&message.message.content)?
        } else {
            Value::String(content_text_parts(&message.message.content))
        };
        return Ok(vec![MessageContent::ToolResult {
            tool_call_id: tool_call_id.clone(),
            output,
            is_error: (message.is_error == Some(true)).then_some(true),
        }]);
    }

    let mut content = message
        .message
        .content
        .iter()
        .map(|part| map_message_content(message, part))
        .collect::<Vec<_>>();
    if message.message.role == Role::Assistant {
        content.extend(message.message.tool_calls.iter().map(|call| {
            let input = match &call.arguments {
                Some(arguments) => serde_json::from_str(arguments)
                    .unwrap_or_else(|_| Value::String(arguments.clone())),
                None => Value::Null,
            };
            MessageContent::ToolUse {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                input,
            }
        }));
    }
    Ok(content)
}

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/messageProjection.ts
//   toProtocolMessage()
//
// Pure in-memory projection; it remains synchronous like the original.
pub fn to_protocol_message(
    session_id: &str,
    index: u64,
    message: &ContextMessage,
    session_created_at_ms: i64,
    created_at_ms_override: Option<i64>,
) -> Result<ProtocolMessage, MessageProjectionError> {
    let created_at_ms = match created_at_ms_override {
        Some(created_at_ms) => created_at_ms,
        None => session_created_at_ms
            .checked_add(
                i64::try_from(index).map_err(|_| MessageProjectionError::InvalidTimestamp)?,
            )
            .ok_or(MessageProjectionError::InvalidTimestamp)?,
    };
    let created_at = DateTime::<Utc>::from_timestamp_millis(created_at_ms)
        .ok_or(MessageProjectionError::InvalidTimestamp)?
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let created_at = serde_json::from_value(Value::String(created_at))?;
    let metadata = message
        .origin
        .as_ref()
        .map(|origin| {
            serde_json::to_value(origin)
                .map(|origin| Map::from_iter([("origin".to_owned(), origin)]))
        })
        .transpose()?;

    Ok(ProtocolMessage {
        id: message
            .id
            .clone()
            .unwrap_or_else(|| derive_message_id(session_id, index)),
        session_id: session_id.to_owned(),
        role: to_protocol_role(message.message.role),
        content: to_protocol_message_content(message)?,
        created_at,
        prompt_id: None,
        parent_message_id: None,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        agent::context_memory::types::PromptOrigin,
        kosong::contract::message::{MediaUrl, Message, ToolCall, ToolCallType},
    };
    use serde_json::json;

    fn context_message(role: Role, content: Vec<ContentPart>) -> ContextMessage {
        ContextMessage {
            message: Message::new(role, content, Vec::new()),
            id: None,
            provider_message_id: None,
            origin: None,
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn derives_stable_id_time_and_origin_metadata() {
        let mut message = context_message(
            Role::User,
            vec![ContentPart::Text {
                text: "hello".into(),
            }],
        );
        message.origin = Some(PromptOrigin::User);
        let projected = to_protocol_message("session_1", 7, &message, 0, None).unwrap();

        assert_eq!(projected.id, "msg_session_1_000007");
        assert_eq!(projected.created_at.as_str(), "1970-01-01T00:00:00.007Z");
        assert_eq!(
            projected.metadata,
            Some(Map::from_iter([(
                "origin".into(),
                json!({ "kind": "user" })
            )]))
        );
    }

    #[test]
    fn projects_model_attachment_notice_as_structured_file_content() {
        let notice = "Attached file \"data.xml\" (application/xml, 8 bytes): \
                      C:\\session\\attachments\\f_1-data.xml — open it with the Read tool";
        let mut message = context_message(
            Role::User,
            vec![ContentPart::Text {
                text: notice.into(),
            }],
        );
        message
            .attachments
            .push(crate::agent::context_memory::ContextFileAttachment {
                file_id: "f_1".into(),
                name: "data.xml".into(),
                media_type: "application/xml".into(),
                size: 8,
                model_text: notice.into(),
            });

        let projected = to_protocol_message("session_1", 0, &message, 0, None).unwrap();
        assert_eq!(
            projected.content,
            vec![MessageContent::File {
                file_id: "f_1".into(),
                name: "data.xml".into(),
                media_type: "application/xml".into(),
                size: 8,
            }]
        );
    }

    #[test]
    fn maps_thinking_and_media_content() {
        let message = context_message(
            Role::Assistant,
            vec![
                ContentPart::Think {
                    think: "reason".into(),
                    encrypted: Some("signature".into()),
                },
                ContentPart::ImageUrl {
                    image_url: MediaUrl {
                        url: "image.png".into(),
                        id: None,
                    },
                },
                ContentPart::AudioUrl {
                    audio_url: MediaUrl {
                        url: "audio.wav".into(),
                        id: None,
                    },
                },
                ContentPart::VideoUrl {
                    video_url: MediaUrl {
                        url: "video.mp4".into(),
                        id: None,
                    },
                },
            ],
        );
        let value =
            serde_json::to_value(to_protocol_message("s", 0, &message, 0, None).unwrap()).unwrap();
        assert_eq!(
            value["content"],
            json!([
                { "type": "thinking", "thinking": "reason", "signature": "signature" },
                { "type": "image", "source": { "kind": "url", "url": "image.png" } },
                { "type": "text", "text": "[audio:audio.wav]" },
                { "type": "video", "source": { "kind": "url", "url": "video.mp4" } }
            ])
        );
    }

    #[test]
    fn projects_tool_results_as_text_or_raw_media_parts() {
        let mut text = context_message(
            Role::Tool,
            vec![
                ContentPart::Text { text: "a".into() },
                ContentPart::Think {
                    think: "hidden".into(),
                    encrypted: None,
                },
                ContentPart::Text { text: "b".into() },
            ],
        );
        text.message.tool_call_id = Some("call-text".into());
        text.is_error = Some(true);
        let projected = to_protocol_message("s", 0, &text, 0, None).unwrap();
        assert_eq!(
            serde_json::to_value(&projected.content).unwrap(),
            json!([{ "type": "tool_result", "tool_call_id": "call-text", "output": "ab", "is_error": true }])
        );

        let mut media = context_message(
            Role::Tool,
            vec![ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "data:image/png;base64,AAAA".into(),
                    id: None,
                },
            }],
        );
        media.message.tool_call_id = Some("call-media".into());
        let projected = to_protocol_message("s", 0, &media, 0, None).unwrap();
        assert_eq!(
            serde_json::to_value(&projected.content).unwrap(),
            json!([{
                "type": "tool_result",
                "tool_call_id": "call-media",
                "output": [{ "type": "image_url", "imageUrl": { "url": "data:image/png;base64,AAAA" } }]
            }])
        );
    }

    #[test]
    fn parses_valid_tool_arguments_and_preserves_invalid_or_null_arguments() {
        let mut message = context_message(Role::Assistant, Vec::new());
        message.message.tool_calls = Arc::new(vec![
            ToolCall {
                call_type: ToolCallType::Function,
                id: "one".into(),
                name: "valid".into(),
                arguments: Some("{\"x\":1}".into()),
                extras: None,
                stream_index: None,
            },
            ToolCall {
                call_type: ToolCallType::Function,
                id: "two".into(),
                name: "invalid".into(),
                arguments: Some("{".into()),
                extras: None,
                stream_index: None,
            },
            ToolCall {
                call_type: ToolCallType::Function,
                id: "three".into(),
                name: "null".into(),
                arguments: None,
                extras: None,
                stream_index: None,
            },
        ]);
        let projected = to_protocol_message("s", 0, &message, 0, Some(5_000)).unwrap();
        let value = serde_json::to_value(projected).unwrap();
        assert_eq!(value["created_at"], "1970-01-01T00:00:05.000Z");
        assert_eq!(value["content"][0]["input"], json!({ "x": 1 }));
        assert_eq!(value["content"][1]["input"], "{");
        assert!(value["content"][2]["input"].is_null());
    }
}
