use serde_json::{Map, Value};
use std::sync::{Arc, LazyLock};

use crate::agent_core_v2::kosong::contract::message::{
    ContentPart, Message, Role, is_tool_declaration_only_message,
};
use crate::agent_core_v2::kosong::contract::provider::{
    GenerateOptions, ResponseFormat, ThinkingEffort, ToolCallIdPolicy,
};
use crate::agent_core_v2::kosong::provider::bases::openai::openai_common::{
    ConvertedToolMessageContent, OpenAiContentPart, TOOL_RESULT_MEDIA_PLACEHOLDER,
    TOOL_RESULT_MEDIA_PROMPT, ToolMessageConversion, convert_content_part,
    convert_tool_message_content,
};
use crate::agent_core_v2::kosong::provider::bases::openai::openai_hooks::OpenAiChatHooks;
use crate::agent_core_v2::kosong::provider::bases::tool_call_id::sanitize_tool_call_id;

pub const KNOWN_REASONING_KEYS: [&str; 3] = ["reasoning_content", "reasoning_details", "reasoning"];
pub const DEFAULT_OUTBOUND_REASONING_KEY: &str = KNOWN_REASONING_KEYS[0];
pub const CHAT_COMPLETIONS_MAX_OUTPUT_TOKENS_CEILING: f64 = 128.0 * 1024.0;

pub static OPENAI_CHAT_TOOL_CALL_ID_POLICY: LazyLock<ToolCallIdPolicy> = LazyLock::new(|| {
    ToolCallIdPolicy::new(Arc::new(|id| sanitize_tool_call_id(id, Some(64))), Some(64))
});

pub type OpenAiLegacyGenerationKwargs = Map<String, Value>;

// Original: openai-legacy.ts, extractReasoningContent()
pub fn extract_reasoning_content(source: &Value, explicit_key: Option<&str>) -> Option<String> {
    let record = source.as_object()?;
    match explicit_key {
        Some(key) => record.get(key).and_then(Value::as_str).map(str::to_owned),
        None => KNOWN_REASONING_KEYS
            .iter()
            .find_map(|key| record.get(*key).and_then(Value::as_str).map(str::to_owned)),
    }
}

// Original: openai-legacy.ts, usesMaxCompletionTokens()
pub fn uses_max_completion_tokens(model: &str) -> bool {
    let normalized = model.to_ascii_lowercase();
    let bytes = normalized.as_bytes();
    let reasoning_model = bytes.len() >= 2
        && bytes[0] == b'o'
        && bytes[1].is_ascii_digit()
        && (bytes.len() == 2 || matches!(bytes[2], b'-' | b'.'));
    let gpt_5 = normalized
        .strip_prefix("gpt-5")
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(['-', '.']));
    reasoning_model || gpt_5
}

// Original: openai-legacy.ts, completionTokenKwargs()
pub fn completion_token_kwargs(
    model: &str,
    max_completion_tokens: f64,
) -> OpenAiLegacyGenerationKwargs {
    let key = if uses_max_completion_tokens(model) {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    Map::from_iter([(key.to_owned(), Value::from(max_completion_tokens))])
}

// Original: openai-legacy.ts, normalizeGenerationKwargs()
pub fn normalize_generation_kwargs(
    model: &str,
    source: &OpenAiLegacyGenerationKwargs,
) -> OpenAiLegacyGenerationKwargs {
    let mut kwargs = source.clone();
    if uses_max_completion_tokens(model) {
        if !kwargs.contains_key("max_completion_tokens")
            && let Some(max_tokens) = kwargs.get("max_tokens").cloned()
        {
            kwargs.insert("max_completion_tokens".to_owned(), max_tokens);
        }
        kwargs.remove("max_tokens");
    }
    kwargs
}

// Original: openai-legacy.ts, responseFormatToOpenAI()
pub fn response_format_to_openai(format: &ResponseFormat) -> Map<String, Value> {
    match format {
        ResponseFormat::JsonObject => {
            Map::from_iter([("type".to_owned(), Value::String("json_object".to_owned()))])
        }
        ResponseFormat::JsonSchema { json_schema } => {
            let mut schema = Map::from_iter([
                ("name".to_owned(), Value::String(json_schema.name.clone())),
                (
                    "schema".to_owned(),
                    Value::Object(json_schema.schema.clone()),
                ),
            ]);
            if let Some(strict) = json_schema.strict {
                schema.insert("strict".to_owned(), Value::Bool(strict));
            }
            if let Some(description) = json_schema.description.as_ref() {
                schema.insert("description".to_owned(), Value::String(description.clone()));
            }
            Map::from_iter([
                ("type".to_owned(), Value::String("json_schema".to_owned())),
                ("json_schema".to_owned(), Value::Object(schema)),
            ])
        }
    }
}

pub const OMITTED_AUDIO_PLACEHOLDER: &str = "(audio omitted: not supported by this provider)";
pub const OMITTED_VIDEO_PLACEHOLDER: &str = "(video omitted: not supported by this provider)";

fn openai_content_part_value(part: OpenAiContentPart) -> Value {
    match part {
        OpenAiContentPart::Text { text } => serde_json::json!({"type":"text","text":text}),
        OpenAiContentPart::ImageUrl { image_url } => {
            serde_json::json!({"type":"image_url","image_url":image_url})
        }
        OpenAiContentPart::AudioUrl { audio_url } => {
            serde_json::json!({"type":"audio_url","audio_url":audio_url})
        }
        OpenAiContentPart::VideoUrl { video_url } => {
            serde_json::json!({"type":"video_url","video_url":video_url})
        }
    }
}

fn converted_parts_value(parts: Vec<OpenAiContentPart>) -> Value {
    Value::Array(parts.into_iter().map(openai_content_part_value).collect())
}

fn convert_tool_message_content_for_chat(
    message: &Message,
    conversion: ToolMessageConversion,
) -> Value {
    match convert_tool_message_content(message, conversion) {
        ConvertedToolMessageContent::Parts(parts) => converted_parts_value(parts),
        ConvertedToolMessageContent::Text(content) => {
            let mut lines = if content.is_empty() {
                Vec::new()
            } else {
                vec![content]
            };
            if message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::AudioUrl { .. }))
            {
                lines.push(OMITTED_AUDIO_PLACEHOLDER.to_owned());
            }
            if message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::VideoUrl { .. }))
            {
                lines.push(OMITTED_VIDEO_PLACEHOLDER.to_owned());
            }
            if lines.is_empty()
                && message
                    .content
                    .iter()
                    .any(|part| matches!(part, ContentPart::ImageUrl { .. }))
            {
                Value::String(TOOL_RESULT_MEDIA_PLACEHOLDER.to_owned())
            } else {
                Value::String(lines.join("\n"))
            }
        }
    }
}

// Original: openai-legacy.ts, convertMessage()
pub fn convert_message(
    message: &Message,
    reasoning_key: Option<&str>,
    tool_message_conversion: ToolMessageConversion,
    preserve_thinking: bool,
    allow_tool_result_extraction: bool,
) -> Map<String, Value> {
    let mut reasoning_content = String::new();
    let mut has_reasoning_part = false;
    let mut non_think_parts = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Think { think, .. } => {
                has_reasoning_part = true;
                reasoning_content.push_str(think);
            }
            _ => non_think_parts.push(part),
        }
    }

    let mut result = Map::from_iter([(
        "role".to_owned(),
        Value::String(message.role.as_str().to_owned()),
    )]);
    if message.role == Role::Tool {
        let has_non_text_part = message
            .content
            .iter()
            .any(|part| !matches!(part, ContentPart::Text { .. } | ContentPart::Think { .. }));
        let conversion = if allow_tool_result_extraction && has_non_text_part {
            ToolMessageConversion::ExtractText
        } else {
            tool_message_conversion
        };
        result.insert(
            "content".to_owned(),
            convert_tool_message_content_for_chat(message, conversion),
        );
    } else if let [ContentPart::Text { text }] = non_think_parts.as_slice() {
        result.insert("content".to_owned(), Value::String(text.clone()));
    } else if !non_think_parts.is_empty() {
        result.insert(
            "content".to_owned(),
            converted_parts_value(
                non_think_parts
                    .into_iter()
                    .filter_map(convert_content_part)
                    .collect(),
            ),
        );
    }

    if let Some(name) = message.name.as_ref() {
        result.insert("name".to_owned(), Value::String(name.clone()));
    }
    if !message.tool_calls.is_empty() {
        result.insert(
            "tool_calls".to_owned(),
            Value::Array(
                message
                    .tool_calls
                    .iter()
                    .map(|tool_call| {
                        serde_json::json!({
                            "type": "function",
                            "id": tool_call.id,
                            "function": {
                                "name": tool_call.name,
                                "arguments": tool_call.arguments,
                            }
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(tool_call_id) = message.tool_call_id.as_ref() {
        result.insert(
            "tool_call_id".to_owned(),
            Value::String(tool_call_id.clone()),
        );
    }
    if has_reasoning_part || (preserve_thinking && message.role == Role::Assistant) {
        result.insert(
            reasoning_key
                .unwrap_or(DEFAULT_OUTBOUND_REASONING_KEY)
                .to_owned(),
            Value::String(reasoning_content),
        );
    }
    result
}

fn tool_result_image_parts(message: &Message) -> Vec<OpenAiContentPart> {
    message
        .content
        .iter()
        .filter(|part| matches!(part, ContentPart::ImageUrl { .. }))
        .filter_map(convert_content_part)
        .collect()
}

fn append_tool_result_media_message(
    messages: &mut Vec<Map<String, Value>>,
    pending: &mut Vec<OpenAiContentPart>,
) {
    if pending.is_empty() {
        return;
    }
    let mut content = vec![serde_json::json!({
        "type": "text",
        "text": TOOL_RESULT_MEDIA_PROMPT,
    })];
    content.extend(pending.drain(..).map(openai_content_part_value));
    messages.push(Map::from_iter([
        ("role".to_owned(), Value::String("user".to_owned())),
        ("content".to_owned(), Value::Array(content)),
    ]));
}

// Original: openai-legacy.ts, convertHistoryMessages()
pub fn convert_history_messages(
    history: &[Message],
    reasoning_key: Option<&str>,
    tool_message_conversion: ToolMessageConversion,
    preserve_thinking: bool,
) -> Vec<Map<String, Value>> {
    let mut messages = Vec::new();
    let mut pending_tool_result_media = Vec::new();
    for message in history {
        if is_tool_declaration_only_message(message) {
            continue;
        }
        if message.role != Role::Tool {
            append_tool_result_media_message(&mut messages, &mut pending_tool_result_media);
        }
        messages.push(convert_message(
            message,
            reasoning_key,
            tool_message_conversion,
            preserve_thinking,
            true,
        ));
        if message.role == Role::Tool {
            pending_tool_result_media.extend(tool_result_image_parts(message));
        }
    }
    append_tool_result_media_message(&mut messages, &mut pending_tool_result_media);
    messages
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRequestKwargs {
    pub kwargs: OpenAiLegacyGenerationKwargs,
    pub reasoning_effort: Option<String>,
}

fn javascript_min(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else {
        left.min(right)
    }
}

fn javascript_max(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else {
        left.max(right)
    }
}

// Original: openai-legacy.ts, OpenAILegacyChatProvider._resolveRequestKwargs()
pub fn resolve_request_kwargs(
    model: &str,
    generation_kwargs: &OpenAiLegacyGenerationKwargs,
    default_thinking_effort: Option<&ThinkingEffort>,
    hooks: Option<&OpenAiChatHooks>,
    history: &[Message],
    options: Option<&GenerateOptions>,
) -> ResolvedRequestKwargs {
    let mut kwargs = generation_kwargs.clone();

    if let Some(cache_key) = options.and_then(|options| options.cache_key.as_deref()) {
        let hooked = hooks
            .and_then(|hooks| hooks.cache_key.as_ref())
            .and_then(|hook| hook(cache_key));
        kwargs.extend(hooked.unwrap_or_else(|| {
            Map::from_iter([(
                "prompt_cache_key".to_owned(),
                Value::String(cache_key.to_owned()),
            )])
        }));
    }
    if let Some(temperature) = options
        .and_then(|options| options.sampling.as_ref())
        .and_then(|sampling| sampling.temperature)
    {
        kwargs.insert("temperature".to_owned(), Value::from(temperature));
    }
    if let Some(top_p) = options
        .and_then(|options| options.sampling.as_ref())
        .and_then(|sampling| sampling.top_p)
    {
        kwargs.insert("top_p".to_owned(), Value::from(top_p));
    }

    let thinking = options
        .and_then(|options| options.thinking.as_ref())
        .map(|thinking| (thinking.effort.clone(), thinking.keep.clone()))
        .or_else(|| {
            default_thinking_effort
                .cloned()
                .map(|effort| (effort, None))
        });
    let mut explicit_thinking_effort = None;
    if let Some((effort, keep)) = thinking {
        let hooked = hooks
            .and_then(|hooks| hooks.with_thinking.as_ref())
            .and_then(|hook| {
                hook(
                    &effort,
                    &crate::agent_core_v2::kosong::protocol::protocol_trait::ThinkingHookOptions {
                        keep,
                    },
                    &kwargs,
                )
            });
        if let Some(hooked) = hooked {
            kwargs.extend(hooked);
        } else {
            explicit_thinking_effort = Some(effort);
        }
    }

    let mut reasoning_effort = explicit_thinking_effort
        .as_ref()
        .filter(|effort| !matches!(effort.as_str(), "off" | "on"))
        .map(ToString::to_string);
    let has_thinking_hook = hooks.is_some_and(|hooks| hooks.with_thinking.is_some());
    if reasoning_effort.is_none()
        && !explicit_thinking_effort
            .as_ref()
            .is_some_and(|effort| effort.is_off())
        && !kwargs.contains_key("reasoning_effort")
        && !has_thinking_hook
        && history.iter().any(|message| {
            message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::Think { .. }))
        })
    {
        reasoning_effort = Some("medium".to_owned());
    }

    if let Some(mut cap) = options.and_then(|options| options.max_completion_tokens) {
        if let Some((used, max)) =
            options.and_then(|options| options.used_context_tokens.zip(options.max_context_tokens))
            && max > 0.0
        {
            cap = javascript_min(cap, max - used);
        }
        cap = javascript_max(1.0, cap);
        let hooked = hooks
            .and_then(|hooks| hooks.with_max_completion_tokens.as_ref())
            .and_then(|hook| hook(cap));
        if let Some(hooked) = hooked {
            kwargs.extend(hooked);
        } else {
            kwargs.extend(completion_token_kwargs(
                model,
                javascript_max(
                    1.0,
                    javascript_min(cap, CHAT_COMPLETIONS_MAX_OUTPUT_TOKENS_CEILING),
                ),
            ));
        }
    }

    ResolvedRequestKwargs {
        kwargs,
        reasoning_effort,
    }
}

// MIGRATION-TODO:
// Original: openai-legacy.ts, OpenAILegacyStreamedMessage,
// OpenAILegacyChatProvider network/stream methods.
// Missing dependency: the selected async OpenAI HTTP transport and its stream
// event types. Temporary behavior: none; this module exposes only completed
// pure request-shaping methods. Completion condition: port message/history
// shaping next, then implement the provider and stream over reqwest or a
// maintained SDK while preserving request order, cancellation and errors.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::message::{MediaUrl, ToolCall, ToolCallType};
    use crate::agent_core_v2::kosong::contract::provider::JsonSchemaDefinition;
    use crate::agent_core_v2::kosong::contract::provider::{
        SamplingOptions, ThinkingRequestOptions,
    };
    use crate::agent_core_v2::kosong::contract::tool::Tool;
    use serde_json::json;

    #[test]
    fn reasoning_extraction_honors_explicit_key_or_known_key_order() {
        let source = json!({
            "reasoning_details": "details",
            "reasoning": "fallback",
            "custom": "explicit"
        });
        assert_eq!(
            extract_reasoning_content(&source, None).as_deref(),
            Some("details")
        );
        assert_eq!(
            extract_reasoning_content(&source, Some("custom")).as_deref(),
            Some("explicit")
        );
        assert_eq!(extract_reasoning_content(&source, Some("missing")), None);
        assert_eq!(extract_reasoning_content(&Value::Null, None), None);
    }

    #[test]
    fn max_completion_field_model_rules_match_anchored_source_patterns() {
        for model in ["o1", "O3-mini", "o4.1", "gpt-5", "GPT-5-mini"] {
            assert!(uses_max_completion_tokens(model), "{model}");
        }
        for model in ["o", "o1preview", "gpt-50", "gpt-5x", "gpt-4o"] {
            assert!(!uses_max_completion_tokens(model), "{model}");
        }
        assert_eq!(
            completion_token_kwargs("o1", 8192.0),
            json!({"max_completion_tokens":8192.0})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(
            completion_token_kwargs("gpt-4o", 4096.0),
            json!({"max_tokens":4096.0}).as_object().unwrap().clone()
        );
    }

    #[test]
    fn generation_normalization_moves_legacy_field_only_when_needed() {
        let source = json!({"max_tokens":100,"temperature":0.5})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(
            normalize_generation_kwargs("o1", &source),
            json!({"max_completion_tokens":100,"temperature":0.5})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(normalize_generation_kwargs("gpt-4o", &source), source);

        let explicit = json!({"max_tokens":100,"max_completion_tokens":200})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(
            normalize_generation_kwargs("gpt-5", &explicit),
            json!({"max_completion_tokens":200})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn response_formats_match_openai_request_wire_shape() {
        assert_eq!(
            response_format_to_openai(&ResponseFormat::JsonObject),
            json!({"type":"json_object"}).as_object().unwrap().clone()
        );
        let format = ResponseFormat::JsonSchema {
            json_schema: JsonSchemaDefinition {
                name: "answer".to_owned(),
                schema: json!({"type":"object"}).as_object().unwrap().clone(),
                strict: Some(true),
                description: Some("Structured answer".to_owned()),
            },
        };
        assert_eq!(
            response_format_to_openai(&format),
            json!({
                "type":"json_schema",
                "json_schema":{
                    "name":"answer",
                    "schema":{"type":"object"},
                    "strict":true,
                    "description":"Structured answer"
                }
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn chat_tool_call_policy_sanitizes_and_truncates_to_64_utf16_units() {
        let normalized =
            OPENAI_CHAT_TOOL_CALL_ID_POLICY.normalize(&format!("call/{}", "a".repeat(80)));
        assert_eq!(normalized.len(), 64);
        assert!(normalized.starts_with("call_"));
        assert_eq!(OPENAI_CHAT_TOOL_CALL_ID_POLICY.max_length, Some(64));
        assert_eq!(CHAT_COMPLETIONS_MAX_OUTPUT_TOKENS_CEILING, 131_072.0);
    }

    #[test]
    fn message_and_history_conversion_preserve_reasoning_tools_and_media_projection() {
        let mut assistant = Message::new(
            Role::Assistant,
            vec![
                ContentPart::Think {
                    think: "reason".to_owned(),
                    encrypted: None,
                },
                ContentPart::Text {
                    text: "answer".to_owned(),
                },
            ],
            vec![ToolCall {
                call_type: ToolCallType::Function,
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                arguments: Some("{}".to_owned()),
                extras: None,
                stream_index: None,
            }],
        );
        assistant.name = Some("assistant-name".to_owned());
        let converted = convert_message(
            &assistant,
            Some("reasoning_content"),
            ToolMessageConversion::Parts,
            false,
            true,
        );
        assert_eq!(converted["content"], "answer");
        assert_eq!(converted["reasoning_content"], "reason");
        assert_eq!(converted["tool_calls"][0]["function"]["name"], "read");

        let mut tool_result = Message::new(
            Role::Tool,
            vec![
                ContentPart::ImageUrl {
                    image_url: MediaUrl {
                        url: "https://example.test/image.png".to_owned(),
                        id: None,
                    },
                },
                ContentPart::AudioUrl {
                    audio_url: MediaUrl {
                        url: "https://example.test/audio.wav".to_owned(),
                        id: None,
                    },
                },
            ],
            Vec::new(),
        );
        tool_result.tool_call_id = Some("call-1".to_owned());
        let mut declaration = Message::new(Role::User, Vec::new(), Vec::new());
        declaration.tools = Some(vec![Tool {
            name: "ignored".to_owned(),
            description: "ignored".to_owned(),
            parameters: Map::new(),
            deferred: None,
        }]);
        let user = Message::new(
            Role::User,
            vec![ContentPart::Text {
                text: "next".to_owned(),
            }],
            Vec::new(),
        );
        let history = convert_history_messages(
            &[tool_result, declaration, user],
            None,
            ToolMessageConversion::Parts,
            false,
        );
        assert_eq!(history.len(), 3);
        assert_eq!(
            history[0]["content"],
            Value::String(OMITTED_AUDIO_PLACEHOLDER.to_owned())
        );
        assert_eq!(history[1]["role"], "user");
        assert_eq!(history[1]["content"][0]["text"], TOOL_RESULT_MEDIA_PROMPT);
        assert_eq!(history[1]["content"][1]["type"], "image_url");
        assert_eq!(history[2]["content"], "next");
    }

    #[test]
    fn request_kwargs_preserve_overlay_order_clamps_and_thinking_ownership() {
        let history = vec![Message::new(
            Role::Assistant,
            vec![ContentPart::Think {
                think: "reason".to_owned(),
                encrypted: None,
            }],
            Vec::new(),
        )];
        let options = GenerateOptions {
            cache_key: Some("session".to_owned()),
            sampling: Some(SamplingOptions {
                temperature: Some(0.4),
                top_p: Some(0.8),
            }),
            max_completion_tokens: Some(100.0),
            used_context_tokens: Some(95.0),
            max_context_tokens: Some(100.0),
            ..GenerateOptions::default()
        };
        let resolved =
            resolve_request_kwargs("gpt-4o", &Map::new(), None, None, &history, Some(&options));
        assert_eq!(resolved.kwargs["prompt_cache_key"], "session");
        assert_eq!(resolved.kwargs["temperature"], 0.4);
        assert_eq!(resolved.kwargs["top_p"], 0.8);
        assert_eq!(resolved.kwargs["max_tokens"], 5.0);
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("medium"));

        let hooks = OpenAiChatHooks {
            cache_key: Some(Arc::new(|_| {
                Some(Map::from_iter([("cache".to_owned(), Value::from(1))]))
            })),
            with_thinking: Some(Arc::new(|_, _, kwargs| {
                assert_eq!(kwargs["cache"], 1);
                assert_eq!(kwargs["temperature"], 0.4);
                Some(Map::from_iter([("thinking".to_owned(), Value::from(1))]))
            })),
            with_max_completion_tokens: Some(Arc::new(|cap| {
                assert_eq!(cap, 1.0);
                Some(Map::from_iter([(
                    "custom_max".to_owned(),
                    Value::from(cap),
                )]))
            })),
            ..OpenAiChatHooks::default()
        };
        let hooked_options = GenerateOptions {
            cache_key: Some("session".to_owned()),
            sampling: options.sampling,
            thinking: Some(ThinkingRequestOptions {
                effort: ThinkingEffort::from("high"),
                keep: Some("all".to_owned()),
            }),
            max_completion_tokens: Some(100.0),
            used_context_tokens: Some(120.0),
            max_context_tokens: Some(100.0),
            ..GenerateOptions::default()
        };
        let resolved = resolve_request_kwargs(
            "o1",
            &Map::new(),
            None,
            Some(&hooks),
            &history,
            Some(&hooked_options),
        );
        assert_eq!(resolved.kwargs["thinking"], 1);
        assert_eq!(resolved.kwargs["custom_max"], 1.0);
        assert_eq!(resolved.reasoning_effort, None);

        let off_effort = ThinkingEffort::from("off");
        let off =
            resolve_request_kwargs("o1", &Map::new(), Some(&off_effort), None, &history, None);
        assert_eq!(off.reasoning_effort, None);
    }
}
