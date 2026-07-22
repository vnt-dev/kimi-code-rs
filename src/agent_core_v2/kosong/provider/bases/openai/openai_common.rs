use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agent_core_v2::kosong::contract::capability::ModelCapability;
use crate::agent_core_v2::kosong::contract::message::{
    ContentPart, MediaUrl, Message, extract_text,
};
use crate::agent_core_v2::kosong::contract::provider::FinishReason;
use crate::agent_core_v2::kosong::contract::tool::Tool;
use crate::agent_core_v2::kosong::contract::usage::TokenUsage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAiContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: MediaUrl },
    #[serde(rename = "audio_url")]
    AudioUrl { audio_url: MediaUrl },
    #[serde(rename = "video_url")]
    VideoUrl { video_url: MediaUrl },
}

// Original: openai-common.ts, convertContentPart()
pub fn convert_content_part(part: &ContentPart) -> Option<OpenAiContentPart> {
    match part {
        ContentPart::Text { text } => Some(OpenAiContentPart::Text { text: text.clone() }),
        ContentPart::Think { .. } => None,
        ContentPart::ImageUrl { image_url } => Some(OpenAiContentPart::ImageUrl {
            image_url: image_url.clone(),
        }),
        ContentPart::AudioUrl { audio_url } => Some(OpenAiContentPart::AudioUrl {
            audio_url: audio_url.clone(),
        }),
        ContentPart::VideoUrl { video_url } => Some(OpenAiContentPart::VideoUrl {
            video_url: video_url.clone(),
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiToolParam {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAiFunctionDefinition,
}

// Original: openai-common.ts, toolToOpenAI()
pub fn tool_to_openai(tool: &Tool) -> OpenAiToolParam {
    OpenAiToolParam {
        tool_type: "function".to_owned(),
        function: OpenAiFunctionDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        },
    }
}

// MIGRATION-TODO:
// Original: openai-common.ts, convertOpenAIError()
// Missing dependency: the selected Rust OpenAI transport has not been wired,
// so its SDK-specific timeout/connection/status error types are unavailable.
// Temporary behavior: none; callers must not substitute a generic converter.
// Completion condition: select the transport crate, map its typed errors to
// ChatProviderError with the abort guard first, and port errors.test.ts.

pub fn is_function_tool_call(call_type: &str) -> bool {
    call_type == "function"
}

// Original: openai-common.ts, extractUsage()
pub fn extract_usage(usage: &Value) -> Option<TokenUsage> {
    let usage = usage.as_object()?;
    let prompt_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let completion_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let cached = usage
        .get("cached_tokens")
        .and_then(Value::as_f64)
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_f64)
        })
        .unwrap_or(0.0);
    Some(TokenUsage {
        input_other: prompt_tokens - cached,
        output: completion_tokens,
        input_cache_read: cached,
        input_cache_creation: 0.0,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedFinishReason {
    pub finish_reason: Option<FinishReason>,
    pub raw_finish_reason: Option<String>,
}

// Original: openai-common.ts, normalizeOpenAIFinishReason()
pub fn normalize_openai_finish_reason(raw: Option<&str>) -> NormalizedFinishReason {
    let Some(raw) = raw else {
        return NormalizedFinishReason {
            finish_reason: None,
            raw_finish_reason: None,
        };
    };
    let finish_reason = match raw {
        "stop" => FinishReason::Completed,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "length" => FinishReason::Truncated,
        "content_filter" => FinishReason::Filtered,
        _ => FinishReason::Other,
    };
    NormalizedFinishReason {
        finish_reason: Some(finish_reason),
        raw_finish_reason: Some(raw.to_owned()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMessageConversion {
    ExtractText,
    Parts,
}

pub const TOOL_RESULT_MEDIA_PROMPT: &str = "Attached media from tool result:";
pub const TOOL_RESULT_MEDIA_PLACEHOLDER: &str = "(see attached media)";

pub fn is_media_part(part: &ContentPart) -> bool {
    !matches!(part, ContentPart::Text { .. } | ContentPart::Think { .. })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertedToolMessageContent {
    Text(String),
    Parts(Vec<OpenAiContentPart>),
}

// Original: openai-common.ts, convertToolMessageContent()
pub fn convert_tool_message_content(
    message: &Message,
    conversion: ToolMessageConversion,
) -> ConvertedToolMessageContent {
    match conversion {
        ToolMessageConversion::ExtractText => {
            ConvertedToolMessageContent::Text(extract_text(message, ""))
        }
        ToolMessageConversion::Parts => ConvertedToolMessageContent::Parts(
            message
                .content
                .iter()
                .filter_map(convert_content_part)
                .collect(),
        ),
    }
}

pub const OPENAI_REASONING_CAPABILITY: ModelCapability = ModelCapability {
    image_in: false,
    video_in: false,
    audio_in: false,
    thinking: true,
    tool_use: true,
    max_context_tokens: 0,
    dynamically_loaded_tools: None,
};

pub const OPENAI_VISION_TOOL_CAPABILITY: ModelCapability = ModelCapability {
    image_in: true,
    video_in: false,
    audio_in: false,
    thinking: false,
    tool_use: true,
    max_context_tokens: 0,
    dynamically_loaded_tools: None,
};

pub const OPENAI_TEXT_TOOL_CAPABILITY: ModelCapability = ModelCapability {
    image_in: false,
    video_in: false,
    audio_in: false,
    thinking: false,
    tool_use: true,
    max_context_tokens: 0,
    dynamically_loaded_tools: None,
};

pub const OPENAI_VISION_TOOL_PREFIXES: [&str; 4] = ["gpt-4o", "gpt-4-turbo", "gpt-4.1", "gpt-4.5"];

pub fn is_openai_reasoning_model(normalized_model_name: &str) -> bool {
    let bytes = normalized_model_name.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'o' && bytes[1].is_ascii_digit()
}

pub fn has_model_prefix(model_name: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| model_name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::message::Role;
    use serde_json::json;

    #[test]
    fn content_conversion_drops_thinking_and_preserves_media_ids() {
        assert_eq!(
            convert_content_part(&ContentPart::Think {
                think: "hidden".to_owned(),
                encrypted: None,
            }),
            None
        );
        assert_eq!(
            serde_json::to_value(convert_content_part(&ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "https://example.test/image.png".to_owned(),
                    id: Some("img-1".to_owned()),
                },
            }))
            .unwrap(),
            json!({"type":"image_url","image_url":{"url":"https://example.test/image.png","id":"img-1"}})
        );
    }

    #[test]
    fn tool_conversion_preserves_function_wire_shape() {
        let tool = Tool {
            name: "read".to_owned(),
            description: "Read a file".to_owned(),
            parameters: json!({"type":"object"}).as_object().unwrap().clone(),
            deferred: Some(true),
        };
        assert_eq!(
            serde_json::to_value(tool_to_openai(&tool)).unwrap(),
            json!({"type":"function","function":{"name":"read","description":"Read a file","parameters":{"type":"object"}}})
        );
    }

    #[test]
    fn usage_prefers_top_level_cached_tokens_then_nested_details() {
        assert_eq!(extract_usage(&Value::Null), None);
        assert_eq!(
            extract_usage(&json!({"prompt_tokens":10,"completion_tokens":3,"cached_tokens":4})),
            Some(TokenUsage {
                input_other: 6.0,
                output: 3.0,
                input_cache_read: 4.0,
                input_cache_creation: 0.0
            })
        );
        assert_eq!(
            extract_usage(&json!({"prompt_tokens":10,"prompt_tokens_details":{"cached_tokens":2}}))
                .unwrap()
                .input_cache_read,
            2.0
        );
    }

    #[test]
    fn finish_reason_mapping_preserves_raw_values() {
        for (raw, expected) in [
            ("stop", FinishReason::Completed),
            ("tool_calls", FinishReason::ToolCalls),
            ("function_call", FinishReason::ToolCalls),
            ("length", FinishReason::Truncated),
            ("content_filter", FinishReason::Filtered),
            ("vendor_reason", FinishReason::Other),
        ] {
            let normalized = normalize_openai_finish_reason(Some(raw));
            assert_eq!(normalized.finish_reason, Some(expected));
            assert_eq!(normalized.raw_finish_reason.as_deref(), Some(raw));
        }
        assert_eq!(normalize_openai_finish_reason(None).finish_reason, None);
    }

    #[test]
    fn tool_message_conversion_and_media_predicate_match_source() {
        let message = Message::new(
            Role::Tool,
            vec![
                ContentPart::Text {
                    text: "a".to_owned(),
                },
                ContentPart::Think {
                    think: "hidden".to_owned(),
                    encrypted: None,
                },
                ContentPart::Text {
                    text: "b".to_owned(),
                },
            ],
            Vec::new(),
        );
        assert_eq!(
            convert_tool_message_content(&message, ToolMessageConversion::ExtractText),
            ConvertedToolMessageContent::Text("ab".to_owned())
        );
        let ConvertedToolMessageContent::Parts(parts) =
            convert_tool_message_content(&message, ToolMessageConversion::Parts)
        else {
            panic!("expected parts")
        };
        assert_eq!(parts.len(), 2);
        assert!(is_media_part(&ContentPart::VideoUrl {
            video_url: MediaUrl {
                url: "v".to_owned(),
                id: None
            }
        }));
    }

    #[test]
    fn capability_helpers_match_model_name_rules() {
        assert!(is_openai_reasoning_model("o1"));
        assert!(is_openai_reasoning_model("o3-mini"));
        assert!(!is_openai_reasoning_model("o-mini"));
        assert!(!is_openai_reasoning_model("gpt-4o"));
        assert!(has_model_prefix(
            "gpt-4.1-mini",
            &OPENAI_VISION_TOOL_PREFIXES
        ));
        assert!(!has_model_prefix(
            "gpt-3.5-turbo",
            &OPENAI_VISION_TOOL_PREFIXES
        ));
        assert_eq!(
            serde_json::to_value(&OPENAI_REASONING_CAPABILITY).unwrap(),
            json!({
                "image_in": false,
                "video_in": false,
                "audio_in": false,
                "thinking": true,
                "tool_use": true,
                "max_context_tokens": 0
            })
        );
        assert_eq!(
            serde_json::to_value([&OPENAI_VISION_TOOL_CAPABILITY, &OPENAI_TEXT_TOOL_CAPABILITY,])
                .unwrap(),
            json!([
                {"image_in":true,"video_in":false,"audio_in":false,"thinking":false,"tool_use":true,"max_context_tokens":0},
                {"image_in":false,"video_in":false,"audio_in":false,"thinking":false,"tool_use":true,"max_context_tokens":0}
            ])
        );
    }
}
