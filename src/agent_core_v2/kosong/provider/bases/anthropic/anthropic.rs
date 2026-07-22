use serde_json::{Map, Value};

use crate::agent_core_v2::kosong::contract::errors::ChatProviderError;
use crate::agent_core_v2::kosong::contract::message::{ContentPart, Message, Role};
use crate::agent_core_v2::kosong::contract::provider::{FinishReason, ResponseFormat};
use crate::agent_core_v2::kosong::contract::tool::Tool;
use crate::agent_core_v2::kosong::provider::bases::anthropic::anthropic_profile::{
    AnthropicModelFamily, AnthropicModelVersion, match_known_anthropic_model_profile,
    parse_anthropic_model_version,
};
use crate::agent_core_v2::kosong::provider::bases::openai::openai_common::NormalizedFinishReason;

pub type AnthropicGenerationKwargs = Map<String, Value>;

pub const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
pub const CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";
pub const CLEAR_THINKING_EDIT: &str = "clear_thinking_20251015";
pub const FALLBACK_MAX_TOKENS: f64 = 128_000.0;

// Original: anthropic.ts, normalizeAnthropicStopReason()
pub fn normalize_anthropic_stop_reason(raw: Option<&str>) -> NormalizedFinishReason {
    let Some(raw) = raw else {
        return NormalizedFinishReason {
            finish_reason: None,
            raw_finish_reason: None,
        };
    };
    let finish_reason = match raw {
        "end_turn" | "stop_sequence" => FinishReason::Completed,
        "max_tokens" => FinishReason::Truncated,
        "tool_use" => FinishReason::ToolCalls,
        "pause_turn" => FinishReason::Paused,
        "refusal" => FinishReason::Filtered,
        _ => FinishReason::Other,
    };
    NormalizedFinishReason {
        finish_reason: Some(finish_reason),
        raw_finish_reason: Some(raw.to_owned()),
    }
}

// Original: anthropic.ts, applyResponseFormat()
pub fn apply_response_format(
    kwargs: &mut AnthropicGenerationKwargs,
    format: Option<&ResponseFormat>,
) -> Result<(), ChatProviderError> {
    let Some(format) = format else {
        return Ok(());
    };
    let ResponseFormat::JsonSchema { json_schema } = format else {
        return Err(ChatProviderError::ChatProvider {
            message: "Anthropic provider requires a JSON schema for structured response output."
                .to_owned(),
        });
    };
    let mut output_config = kwargs
        .get("output_config")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    output_config.insert(
        "format".to_owned(),
        serde_json::json!({
            "type": "json_schema",
            "schema": json_schema.schema,
        }),
    );
    kwargs.insert("output_config".to_owned(), Value::Object(output_config));
    Ok(())
}

fn family_name(family: AnthropicModelFamily) -> &'static str {
    match family {
        AnthropicModelFamily::Opus => "opus",
        AnthropicModelFamily::Sonnet => "sonnet",
        AnthropicModelFamily::Haiku => "haiku",
        AnthropicModelFamily::Fable => "fable",
        AnthropicModelFamily::Mythos => "mythos",
    }
}

fn ceiling_for_key(key: &str) -> Option<f64> {
    Some(match key {
        "fable-5" | "mythos-5" | "opus-4-8" | "opus-4-7" | "opus-4-6" | "sonnet-5"
        | "sonnet-4-6" => 128_000.0,
        "opus-4-5" | "sonnet-4-5" | "sonnet-4-0" | "sonnet-4" | "haiku-4-5" | "haiku-4" => 64_000.0,
        "opus-4-1" | "opus-4-0" | "opus-4" => 32_000.0,
        "opus-3-5" | "sonnet-3-5" | "sonnet-3-7" | "haiku-3-5" => 8_192.0,
        "opus-3" | "sonnet-3" | "haiku-3" => 4_096.0,
        _ => return None,
    })
}

// Original: anthropic.ts, lookupClaudeCeiling()
pub fn lookup_claude_ceiling(version: AnthropicModelVersion) -> Option<f64> {
    let family = family_name(version.family);
    if let Some(minor) = version.minor {
        for candidate in (0..=minor).rev() {
            if let Some(ceiling) =
                ceiling_for_key(&format!("{family}-{}-{candidate}", version.major))
            {
                return Some(ceiling);
            }
        }
    }
    ceiling_for_key(&format!("{family}-{}", version.major))
}

// Original: anthropic.ts, resolveDefaultMaxTokens()
pub fn resolve_default_max_tokens(model: &str, override_tokens: Option<f64>) -> f64 {
    let ceiling = parse_anthropic_model_version(model, true).and_then(lookup_claude_ceiling);
    let Some(ceiling) = ceiling else {
        return override_tokens.unwrap_or(FALLBACK_MAX_TOKENS);
    };
    match override_tokens {
        None => ceiling,
        Some(value) if value.is_nan() => value,
        Some(value) => value.min(ceiling),
    }
}

const SUPPORTED_B64_MEDIA_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const SUPPORTED_B64_VIDEO_TYPES: [&str; 8] = [
    "video/mp4",
    "video/mpeg",
    "video/quicktime",
    "video/webm",
    "video/x-matroska",
    "video/x-msvideo",
    "video/x-flv",
    "video/3gpp",
];
pub const OMITTED_AUDIO_PLACEHOLDER: &str = "(audio omitted: not supported by this provider)";

fn media_url_to_anthropic(
    url: &str,
    block_type: &str,
    supported: &[&str],
) -> Result<Value, ChatProviderError> {
    let source = if let Some(data_url) = url.strip_prefix("data:") {
        let mut parts = data_url.split(";base64,");
        let (Some(media_type), Some(data)) = (parts.next(), parts.next()) else {
            return Err(ChatProviderError::ChatProvider {
                message: format!("Invalid data URL for {block_type}: {url}"),
            });
        };
        if !supported.contains(&media_type) {
            return Err(ChatProviderError::ChatProvider {
                message: format!(
                    "Unsupported media type for base64 {block_type}: {media_type}, url: {url}"
                ),
            });
        }
        serde_json::json!({"type":"base64","data":data,"media_type":media_type})
    } else {
        serde_json::json!({"type":"url","url":url})
    };
    Ok(serde_json::json!({"type":block_type,"source":source}))
}

// Original: anthropic.ts, imageUrlPartToAnthropic()
pub fn image_url_part_to_anthropic(url: &str) -> Result<Value, ChatProviderError> {
    media_url_to_anthropic(url, "image", &SUPPORTED_B64_MEDIA_TYPES)
}

// Original: anthropic.ts, videoUrlPartToAnthropic()
pub fn video_url_part_to_anthropic(url: &str) -> Result<Value, ChatProviderError> {
    media_url_to_anthropic(url, "video", &SUPPORTED_B64_VIDEO_TYPES)
}

// Original: anthropic.ts, convertTool()
pub fn convert_tool(tool: &Tool) -> Value {
    serde_json::json!({
        "name":tool.name,
        "description":tool.description,
        "input_schema":tool.parameters,
    })
}

fn push_audio_placeholder(blocks: &mut Vec<Value>) {
    let duplicate = blocks.last().is_some_and(|block| {
        block.get("type").and_then(Value::as_str) == Some("text")
            && block.get("text").and_then(Value::as_str) == Some(OMITTED_AUDIO_PLACEHOLDER)
    });
    if !duplicate {
        blocks.push(serde_json::json!({"type":"text","text":OMITTED_AUDIO_PLACEHOLDER}));
    }
}

// Original: anthropic.ts, toolResultToBlock()
pub fn tool_result_to_block(
    tool_call_id: &str,
    content: &[ContentPart],
) -> Result<Value, ChatProviderError> {
    let mut blocks = Vec::new();
    for part in content {
        match part {
            ContentPart::Text { text } if !text.is_empty() => {
                blocks.push(serde_json::json!({"type":"text","text":text}));
            }
            ContentPart::ImageUrl { image_url } => {
                blocks.push(image_url_part_to_anthropic(&image_url.url)?);
            }
            ContentPart::VideoUrl { video_url } => {
                blocks.push(video_url_part_to_anthropic(&video_url.url)?);
            }
            ContentPart::AudioUrl { .. } => push_audio_placeholder(&mut blocks),
            ContentPart::Text { .. } | ContentPart::Think { .. } => {}
        }
    }
    Ok(serde_json::json!({
        "type":"tool_result",
        "tool_use_id":tool_call_id,
        "content":blocks,
    }))
}

fn should_preserve_unsigned_thinking(model: &str) -> bool {
    parse_anthropic_model_version(model, false).is_none()
        && match_known_anthropic_model_profile(model).is_none()
}

// Original: anthropic.ts, convertMessage()
pub fn convert_message(message: &Message, model: &str) -> Result<Value, ChatProviderError> {
    if message.role == Role::System {
        let text = message
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(serde_json::json!({
            "role":"user",
            "content":[{"type":"text","text":format!("<system>{text}</system>")}],
        }));
    }
    if message.role == Role::Tool {
        let tool_call_id =
            message
                .tool_call_id
                .as_deref()
                .ok_or_else(|| ChatProviderError::ChatProvider {
                    message: "Tool message missing `toolCallId`.".to_owned(),
                })?;
        return Ok(serde_json::json!({
            "role":"user",
            "content":[tool_result_to_block(tool_call_id, &message.content)?],
        }));
    }

    let mut blocks = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Text { text } => {
                blocks.push(serde_json::json!({"type":"text","text":text}));
            }
            ContentPart::ImageUrl { image_url } => {
                blocks.push(image_url_part_to_anthropic(&image_url.url)?);
            }
            ContentPart::VideoUrl { video_url } => {
                blocks.push(video_url_part_to_anthropic(&video_url.url)?);
            }
            ContentPart::AudioUrl { .. } => push_audio_placeholder(&mut blocks),
            ContentPart::Think { think, encrypted } => {
                if let Some(signature) = encrypted {
                    blocks.push(serde_json::json!({
                        "type":"thinking","thinking":think,"signature":signature,
                    }));
                } else if should_preserve_unsigned_thinking(model) {
                    blocks.push(serde_json::json!({"type":"thinking","thinking":think}));
                }
            }
        }
    }
    for tool_call in &message.tool_calls {
        let input = match tool_call.arguments.as_deref() {
            None | Some("") => Map::new(),
            Some(arguments) => match serde_json::from_str::<Value>(arguments) {
                Ok(Value::Object(input)) => input,
                Ok(_) => {
                    return Err(ChatProviderError::ChatProvider {
                        message: "Tool call arguments must be a JSON object.".to_owned(),
                    });
                }
                Err(_) => {
                    return Err(ChatProviderError::ChatProvider {
                        message: "Tool call arguments must be valid JSON.".to_owned(),
                    });
                }
            },
        };
        blocks.push(serde_json::json!({
            "type":"tool_use",
            "id":tool_call.id,
            "name":tool_call.name,
            "input":input,
        }));
    }
    Ok(serde_json::json!({"role":message.role.as_str(),"content":blocks}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::message::{MediaUrl, ToolCall, ToolCallType};
    use crate::agent_core_v2::kosong::contract::provider::JsonSchemaDefinition;

    #[test]
    fn request_policy_preserves_finish_schema_and_token_rules() {
        for (raw, expected) in [
            ("end_turn", FinishReason::Completed),
            ("max_tokens", FinishReason::Truncated),
            ("tool_use", FinishReason::ToolCalls),
            ("pause_turn", FinishReason::Paused),
            ("refusal", FinishReason::Filtered),
            ("future_reason", FinishReason::Other),
        ] {
            assert_eq!(
                normalize_anthropic_stop_reason(Some(raw)).finish_reason,
                Some(expected)
            );
        }

        let mut kwargs = Map::from_iter([(
            "output_config".to_owned(),
            serde_json::json!({"effort":"high"}),
        )]);
        apply_response_format(
            &mut kwargs,
            Some(&ResponseFormat::JsonSchema {
                json_schema: JsonSchemaDefinition {
                    name: "ignored-by-anthropic".to_owned(),
                    schema: serde_json::json!({"type":"object"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    strict: Some(true),
                    description: Some("ignored".to_owned()),
                },
            }),
        )
        .unwrap();
        assert_eq!(kwargs["output_config"]["effort"], "high");
        assert_eq!(kwargs["output_config"]["format"]["type"], "json_schema");
        assert!(apply_response_format(&mut kwargs, Some(&ResponseFormat::JsonObject)).is_err());

        assert_eq!(
            resolve_default_max_tokens("claude-opus-4-8", None),
            128_000.0
        );
        assert_eq!(
            resolve_default_max_tokens("claude-opus-4-9", Some(200_000.0)),
            128_000.0
        );
        assert_eq!(
            resolve_default_max_tokens("vendor-model", Some(12_345.0)),
            12_345.0
        );
        assert!(resolve_default_max_tokens("claude-opus-4-8", Some(f64::NAN)).is_nan());
    }

    #[test]
    fn wire_conversion_validates_media_and_tool_arguments() {
        assert_eq!(
            image_url_part_to_anthropic("data:image/png;base64,cG5n").unwrap(),
            serde_json::json!({
                "type":"image",
                "source":{"type":"base64","data":"cG5n","media_type":"image/png"}
            })
        );
        assert!(image_url_part_to_anthropic("data:image/svg+xml;base64,svg").is_err());

        let mut message = Message::new(
            Role::Assistant,
            vec![
                ContentPart::Think {
                    think: "hidden".to_owned(),
                    encrypted: Some("signature".to_owned()),
                },
                ContentPart::AudioUrl {
                    audio_url: MediaUrl {
                        url: "https://example/audio.mp3".to_owned(),
                        id: None,
                    },
                },
                ContentPart::AudioUrl {
                    audio_url: MediaUrl {
                        url: "https://example/audio-2.mp3".to_owned(),
                        id: None,
                    },
                },
            ],
            vec![ToolCall {
                call_type: ToolCallType::Function,
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                arguments: Some("{\"path\":\"a\"}".to_owned()),
                extras: None,
                stream_index: None,
            }],
        );
        let converted = convert_message(&message, "claude-opus-4-6").unwrap();
        assert_eq!(converted["content"].as_array().unwrap().len(), 3);
        assert_eq!(converted["content"][0]["signature"], "signature");
        assert_eq!(converted["content"][2]["input"]["path"], "a");

        message.tool_calls[0].arguments = Some("[]".to_owned());
        assert_eq!(
            convert_message(&message, "claude-opus-4-6")
                .unwrap_err()
                .message(),
            "Tool call arguments must be a JSON object."
        );
    }
}
