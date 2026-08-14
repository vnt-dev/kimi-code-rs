use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};

use crate::kosong::contract::capability::ModelCapability;
use crate::kosong::contract::errors::{
    ChatProviderError, classify_base_api_error, normalize_api_status_error, parse_retry_after_ms,
};
use crate::kosong::contract::message::is_tool_declaration_only_message;
use crate::kosong::contract::message::{
    ContentPart, Message, Role, StreamIndex, StreamedMessagePart, ToolCall, ToolCallPart,
    ToolCallPartType, ToolCallType,
};
use crate::kosong::contract::provider::{
    ChatProvider, FinishReason, GenerateOptions, ProviderError, ProviderRequestAuth,
    ResponseFormat, StreamedMessage, ThinkingEffort, ToolCallIdPolicy, TraceId,
};
use crate::kosong::contract::tool::Tool;
use crate::kosong::contract::usage::{TokenUsage, counter_from_json};
use crate::kosong::provider::bases::anthropic::anthropic_hooks::AnthropicHooks;
use crate::kosong::provider::bases::anthropic::anthropic_profile::{
    AnthropicModelFamily, AnthropicModelVersion, AnthropicThinkingMode,
    infer_anthropic_model_profile, match_known_anthropic_model_profile,
    parse_anthropic_model_version,
};
use crate::kosong::provider::bases::anthropic::anthropic_transport::{
    AnthropicClient, AnthropicHttpResponse, ReqwestAnthropicClient,
};
use crate::kosong::provider::bases::http_client::default_provider_http_client;
use crate::kosong::provider::bases::merge_user_messages::{
    ConsecutiveUserMessageMergePolicy, merge_consecutive_user_messages,
};
use crate::kosong::provider::bases::openai::openai_common::NormalizedFinishReason;
use crate::kosong::provider::bases::tool_call_id::{
    normalize_tool_call_ids_for_provider, sanitize_tool_call_id,
};

pub type AnthropicGenerationKwargs = Map<String, Value>;

pub const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
pub const CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";
pub const CLEAR_THINKING_EDIT: &str = "clear_thinking_20251015";
pub const FALLBACK_MAX_TOKENS: u64 = 128_000;

pub static ANTHROPIC_TOOL_CALL_ID_POLICY: LazyLock<ToolCallIdPolicy> = LazyLock::new(|| {
    ToolCallIdPolicy::new(Arc::new(|id| sanitize_tool_call_id(id, Some(64))), Some(64))
});

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

// Original: anthropic.ts, convertAnthropicError()
// Cancellation is selected before the reqwest future, preserving the source
// method's abort-first guard.
pub fn convert_anthropic_error(error: reqwest::Error) -> ChatProviderError {
    if error.is_timeout() {
        ChatProviderError::timeout(error.to_string())
    } else if error.is_connect() || error.is_request() || error.is_body() {
        ChatProviderError::connection(error.to_string())
    } else if error.is_decode() {
        ChatProviderError::ChatProvider {
            message: format!("Anthropic error: {error}"),
        }
    } else {
        classify_base_api_error(&error.to_string())
    }
}

pub fn convert_anthropic_status_error(
    status_code: u16,
    message: &str,
    headers: &reqwest::header::HeaderMap,
) -> ChatProviderError {
    let request_id = headers
        .get("request-id")
        .or_else(|| headers.get("x-request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    normalize_api_status_error(
        i32::from(status_code),
        message,
        request_id,
        parse_retry_after_ms(Some(headers)),
        None,
    )
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

fn ceiling_for_key(key: &str) -> Option<u64> {
    Some(match key {
        "fable-5" | "mythos-5" | "opus-4-8" | "opus-4-7" | "opus-4-6" | "sonnet-5"
        | "sonnet-4-6" => 128_000,
        "opus-4-5" | "sonnet-4-5" | "sonnet-4-0" | "sonnet-4" | "haiku-4-5" | "haiku-4" => 64_000,
        "opus-4-1" | "opus-4-0" | "opus-4" => 32_000,
        "opus-3-5" | "sonnet-3-5" | "sonnet-3-7" | "haiku-3-5" => 8_192,
        "opus-3" | "sonnet-3" | "haiku-3" => 4_096,
        _ => return None,
    })
}

// Original: anthropic.ts, lookupClaudeCeiling()
pub fn lookup_claude_ceiling(version: AnthropicModelVersion) -> Option<u64> {
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
pub fn resolve_default_max_tokens(model: &str, override_tokens: Option<u64>) -> u64 {
    let ceiling = parse_anthropic_model_version(model, true).and_then(lookup_claude_ceiling);
    let Some(ceiling) = ceiling else {
        return override_tokens.unwrap_or(FALLBACK_MAX_TOKENS);
    };
    match override_tokens {
        None => ceiling,
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

const CACHEABLE_TYPES: [&str; 8] = [
    "text",
    "image",
    "document",
    "search_result",
    "tool_use",
    "tool_result",
    "server_tool_use",
    "web_search_tool_result",
];

// Original: anthropic.ts, injectCacheControlOnLastBlock()
pub fn inject_cache_control_on_last_block(messages: &mut [Value]) {
    let Some(block) = messages
        .last_mut()
        .and_then(|message| message.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|content| content.last_mut())
    else {
        return;
    };
    let Some(block_type) = block.get("type").and_then(Value::as_str) else {
        return;
    };
    if CACHEABLE_TYPES.contains(&block_type)
        && let Some(block) = block.as_object_mut()
    {
        block.insert(
            "cache_control".to_owned(),
            serde_json::json!({"type":"ephemeral"}),
        );
    }
}

pub fn is_tool_result_only(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("user")
        && message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|content| {
                !content.is_empty()
                    && content.iter().all(|block| {
                        block.get("type").and_then(Value::as_str) == Some("tool_result")
                    })
            })
}

pub fn should_keep_converted_message(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) != Some("assistant")
        || message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|content| !content.is_empty())
}

#[derive(Debug, Clone, Copy)]
struct ResolvedThinkingProfile {
    mode: AnthropicThinkingMode,
    supports_effort_param: bool,
}

fn resolve_thinking_profile(
    model: &str,
    support_efforts: Option<&[String]>,
    adaptive_thinking: Option<bool>,
) -> ResolvedThinkingProfile {
    let inferred = infer_anthropic_model_profile(model);
    match adaptive_thinking {
        Some(false) => ResolvedThinkingProfile {
            mode: AnthropicThinkingMode::Budget,
            supports_effort_param: false,
        },
        Some(true) => ResolvedThinkingProfile {
            mode: AnthropicThinkingMode::Adaptive,
            supports_effort_param: true,
        },
        None => {
            let requires_adaptive = support_efforts.is_some_and(|efforts| {
                efforts
                    .iter()
                    .any(|effort| !matches!(effort.as_str(), "low" | "medium" | "high"))
            });
            ResolvedThinkingProfile {
                mode: if requires_adaptive {
                    AnthropicThinkingMode::Adaptive
                } else {
                    inferred.mode
                },
                supports_effort_param: requires_adaptive || inferred.supports_effort_param,
            }
        }
    }
}

fn beta_features(kwargs: &AnthropicGenerationKwargs) -> Vec<String> {
    kwargs
        .get("betaFeatures")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn set_beta_features(kwargs: &mut AnthropicGenerationKwargs, betas: Vec<String>) {
    kwargs.insert(
        "betaFeatures".to_owned(),
        Value::Array(betas.into_iter().map(Value::String).collect()),
    );
}

fn budget_tokens_for_effort(effort: &ThinkingEffort) -> Option<u64> {
    match effort.as_str() {
        "low" => Some(1_024),
        "medium" => Some(4_096),
        "on" | "high" => Some(32_000),
        _ => None,
    }
}

// Original: anthropic.ts, AnthropicChatProvider._encodeThinking()
pub fn encode_thinking(
    model: &str,
    support_efforts: Option<&[String]>,
    adaptive_thinking: Option<bool>,
    effort: &ThinkingEffort,
    kwargs: &mut AnthropicGenerationKwargs,
) {
    let profile = resolve_thinking_profile(model, support_efforts, adaptive_thinking);
    let mut betas = beta_features(kwargs);
    if profile.mode == AnthropicThinkingMode::Adaptive {
        betas.retain(|beta| beta != INTERLEAVED_THINKING_BETA);
    }
    set_beta_features(kwargs, betas);

    if effort.is_off() {
        kwargs.insert(
            "thinking".to_owned(),
            serde_json::json!({"type":"disabled"}),
        );
        kwargs.remove("output_config");
        return;
    }
    if profile.mode == AnthropicThinkingMode::Adaptive {
        kwargs.insert(
            "thinking".to_owned(),
            serde_json::json!({"type":"adaptive","display":"summarized"}),
        );
        if effort.as_str() == "on" {
            kwargs.remove("output_config");
        } else {
            kwargs.insert(
                "output_config".to_owned(),
                serde_json::json!({"effort":effort.as_str()}),
            );
        }
        return;
    }

    let budget = budget_tokens_for_effort(effort);
    kwargs.insert(
        "thinking".to_owned(),
        budget.map_or_else(
            || serde_json::json!({"type":"enabled"}),
            |budget| serde_json::json!({"type":"enabled","budget_tokens":budget}),
        ),
    );
    if (profile.supports_effort_param || budget.is_none()) && effort.as_str() != "on" {
        kwargs.insert(
            "output_config".to_owned(),
            serde_json::json!({"effort":effort.as_str()}),
        );
    } else {
        kwargs.remove("output_config");
    }
}

// Original: anthropic.ts, applyThinkingKeep()
pub fn apply_thinking_keep(kwargs: &mut AnthropicGenerationKwargs, keep: &str) {
    let mut betas = beta_features(kwargs);
    if !betas.iter().any(|beta| beta == CONTEXT_MANAGEMENT_BETA) {
        betas.push(CONTEXT_MANAGEMENT_BETA.to_owned());
    }
    set_beta_features(kwargs, betas);

    let existing = kwargs
        .get("contextManagement")
        .and_then(|value| value.get("edits"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut edits = vec![serde_json::json!({"type":CLEAR_THINKING_EDIT,"keep":keep})];
    edits.extend(
        existing
            .into_iter()
            .filter(|edit| edit.get("type").and_then(Value::as_str) != Some(CLEAR_THINKING_EDIT)),
    );
    kwargs.insert(
        "contextManagement".to_owned(),
        serde_json::json!({"edits":edits}),
    );
}

pub const ANTHROPIC_VISION_TOOL_CAPABILITY: ModelCapability = ModelCapability {
    image_in: true,
    video_in: false,
    audio_in: false,
    thinking: false,
    tool_use: true,
    max_context_tokens: 0,
    dynamically_loaded_tools: None,
};

pub const ANTHROPIC_THINKING_VISION_TOOL_CAPABILITY: ModelCapability = ModelCapability {
    thinking: true,
    ..ANTHROPIC_VISION_TOOL_CAPABILITY
};

// Original: anthropic.ts, getAnthropicModelCapability()
pub fn get_anthropic_model_capability(model_name: &str) -> Option<&'static ModelCapability> {
    let normalized = model_name.to_ascii_lowercase();
    if ["claude-3-", "claude-3.5-", "claude-3.7-"]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        Some(&ANTHROPIC_VISION_TOOL_CAPABILITY)
    } else if [
        "claude-opus-4",
        "claude-sonnet-4",
        "claude-haiku-4",
        "claude-fable",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
    {
        Some(&ANTHROPIC_THINKING_VISION_TOOL_CAPABILITY)
    } else {
        None
    }
}

struct AnthropicMessageMergePolicy;

impl ConsecutiveUserMessageMergePolicy<Value> for AnthropicMessageMergePolicy {
    fn is_user(&self, message: &Value) -> bool {
        message.get("role").and_then(Value::as_str) == Some("user")
    }

    fn is_tool_result_only(&self, message: &Value) -> bool {
        is_tool_result_only(message)
    }

    fn merge(&self, mut last: Value, next: Value) -> Value {
        let next_content = next
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
            content.extend(next_content);
        }
        last
    }
}

#[derive(Debug)]
pub struct AnthropicPreparedRequest {
    pub params: Map<String, Value>,
    pub extra_headers: IndexMap<String, String>,
    pub use_beta_api: bool,
}

fn extend_hook_patch(target: &mut Map<String, Value>, patch: Map<String, Value>) {
    // A trait's JavaScript `undefined` is represented as JSON null inside the
    // Rust-only hook boundary. Keep it in kwargs so it shadows a seeded value;
    // request serialization below already omits null values.
    target.extend(patch);
}

// Original:
//   anthropic.ts, AnthropicChatProvider.generate() request construction
//   before client selection and I/O.
#[allow(clippy::too_many_arguments)]
pub fn build_anthropic_request(
    model: &str,
    generation_kwargs: &AnthropicGenerationKwargs,
    explicit_max_tokens: bool,
    default_metadata: Option<&IndexMap<String, String>>,
    adaptive_thinking: Option<bool>,
    support_efforts: Option<&[String]>,
    beta_api: bool,
    default_thinking_effort: Option<&ThinkingEffort>,
    hooks: Option<&AnthropicHooks>,
    system_prompt: &str,
    tools: &[Tool],
    history: &[Message],
    options: Option<&GenerateOptions>,
) -> Result<AnthropicPreparedRequest, ProviderError> {
    let normalized = normalize_tool_call_ids_for_provider(
        history
            .iter()
            .filter(|message| !is_tool_declaration_only_message(message))
            .cloned()
            .collect(),
        &ANTHROPIC_TOOL_CALL_ID_POLICY,
    )?;
    let converted = normalized
        .iter()
        .map(|message| convert_message(message, model))
        .collect::<Result<Vec<_>, _>>()?;
    let mut messages = merge_consecutive_user_messages(
        &converted
            .into_iter()
            .filter(should_keep_converted_message)
            .collect::<Vec<_>>(),
        &AnthropicMessageMergePolicy,
    );
    inject_cache_control_on_last_block(&mut messages);

    let mut kwargs = generation_kwargs.clone();
    let mut use_beta_api = beta_api;
    let mut metadata = default_metadata.cloned();
    if let Some(cache_key) = options.and_then(|options| options.cache_key.as_ref()) {
        metadata
            .get_or_insert_default()
            .insert("user_id".to_owned(), cache_key.clone());
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
        .map(|thinking| (&thinking.effort, thinking.keep.as_deref()))
        .or_else(|| default_thinking_effort.map(|effort| (effort, None)));
    if let Some((effort, keep)) = thinking {
        let hooked = hooks.and_then(|hooks| {
            (hooks.with_thinking)(
                effort,
                &crate::kosong::protocol::protocol_trait::ThinkingHookOptions {
                    keep: keep.map(str::to_owned),
                },
                &kwargs,
            )
        });
        if let Some(hooked) = hooked {
            extend_hook_patch(&mut kwargs, hooked);
        } else {
            encode_thinking(
                model,
                support_efforts,
                adaptive_thinking,
                effort,
                &mut kwargs,
            );
        }
        if let Some(keep) = keep {
            apply_thinking_keep(&mut kwargs, keep);
            use_beta_api = true;
        }
    }

    if let Some(mut cap) = options.and_then(|options| options.max_completion_tokens) {
        if let Some((used, max)) =
            options.and_then(|options| options.used_context_tokens.zip(options.max_context_tokens))
            && max > 0
        {
            cap = cap.min(max.saturating_sub(used));
        }
        cap = cap.max(1);
        let requested_cap = resolve_default_max_tokens(model, Some(cap));
        let existing_cap = kwargs.get("max_tokens").and_then(Value::as_u64);
        let max_tokens = if existing_cap.is_none() || explicit_max_tokens {
            existing_cap.unwrap_or(requested_cap)
        } else {
            existing_cap.unwrap_or(requested_cap).min(requested_cap)
        };
        kwargs.insert("max_tokens".to_owned(), Value::from(max_tokens));
    }

    let mut request_kwargs = Map::new();
    for key in [
        "max_tokens",
        "temperature",
        "top_k",
        "top_p",
        "thinking",
        "output_config",
    ] {
        if let Some(value) = kwargs.get(key).filter(|value| !value.is_null()) {
            request_kwargs.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(context_management) = kwargs
        .get("contextManagement")
        .filter(|value| !value.is_null())
    {
        request_kwargs.insert("context_management".to_owned(), context_management.clone());
    }
    apply_response_format(
        &mut request_kwargs,
        options.and_then(|options| options.response_format.as_ref()),
    )?;

    let betas = beta_features(&kwargs);
    let mut extra_headers = IndexMap::new();
    if !use_beta_api && !betas.is_empty() {
        extra_headers.insert("anthropic-beta".to_owned(), betas.join(","));
    }
    let mut anthropic_tools = tools.iter().map(convert_tool).collect::<Vec<_>>();
    if let Some(last_tool) = anthropic_tools.last_mut().and_then(Value::as_object_mut) {
        last_tool.insert(
            "cache_control".to_owned(),
            serde_json::json!({"type":"ephemeral"}),
        );
    }

    let mut params = Map::from_iter([
        ("model".to_owned(), Value::String(model.to_owned())),
        ("messages".to_owned(), Value::Array(messages)),
    ]);
    params.extend(request_kwargs);
    if !system_prompt.is_empty() {
        params.insert(
            "system".to_owned(),
            serde_json::json!([{
                "type":"text",
                "text":system_prompt,
                "cache_control":{"type":"ephemeral"}
            }]),
        );
    }
    if !anthropic_tools.is_empty() {
        params.insert("tools".to_owned(), Value::Array(anthropic_tools));
    }
    if let Some(metadata) = metadata {
        params.insert("metadata".to_owned(), serde_json::to_value(metadata)?);
    }
    if use_beta_api && !betas.is_empty() {
        params.insert(
            "betas".to_owned(),
            Value::Array(betas.into_iter().map(Value::String).collect()),
        );
    }
    Ok(AnthropicPreparedRequest {
        params,
        extra_headers,
        use_beta_api,
    })
}

enum AnthropicResponseEvent {
    Message(Value),
    Stream(Value),
}

// Original: anthropic.ts, AnthropicStreamedMessage
pub struct AnthropicStreamedMessage {
    source: Pin<Box<dyn Stream<Item = Result<AnthropicResponseEvent, ProviderError>> + Send>>,
    pending: VecDeque<StreamedMessagePart>,
    id: Option<String>,
    usage: TokenUsage,
    finish_reason: Option<FinishReason>,
    raw_finish_reason: Option<String>,
}

impl AnthropicStreamedMessage {
    pub fn from_response(response: Value) -> Self {
        Self::new(futures_util::stream::iter([Ok(
            AnthropicResponseEvent::Message(response),
        )]))
    }

    pub fn from_stream<S>(response: S) -> Self
    where
        S: Stream<Item = Result<Value, ProviderError>> + Send + 'static,
    {
        Self::new(response.map(|event| event.map(AnthropicResponseEvent::Stream)))
    }

    fn new<S>(source: S) -> Self
    where
        S: Stream<Item = Result<AnthropicResponseEvent, ProviderError>> + Send + 'static,
    {
        Self {
            source: Box::pin(source),
            pending: VecDeque::new(),
            id: None,
            usage: TokenUsage {
                input_other: 0,
                output: 0,
                input_cache_read: 0,
                input_cache_creation: 0,
            },
            finish_reason: None,
            raw_finish_reason: None,
        }
    }

    fn capture_stop_reason(&mut self, raw: Option<&str>) {
        let normalized = normalize_anthropic_stop_reason(raw);
        self.finish_reason = normalized.finish_reason;
        self.raw_finish_reason = normalized.raw_finish_reason;
    }

    fn extract_usage(&mut self, usage: Option<&Value>) {
        let number = |key| counter_from_json(usage.and_then(|usage| usage.get(key)));
        self.usage = TokenUsage {
            input_other: number("input_tokens"),
            output: number("output_tokens"),
            input_cache_read: number("cache_read_input_tokens"),
            input_cache_creation: number("cache_creation_input_tokens"),
        };
    }

    fn push_content_block(&mut self, block: &Value, streaming: bool, index: Option<i64>) {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    self.pending
                        .push_back(StreamedMessagePart::Content(ContentPart::Text {
                            text: text.to_owned(),
                        }));
                }
            }
            Some("thinking") => {
                let think = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let encrypted = (!streaming)
                    .then(|| {
                        block
                            .get("signature")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten();
                self.pending
                    .push_back(StreamedMessagePart::Content(ContentPart::Think {
                        think,
                        encrypted,
                    }));
            }
            Some("redacted_thinking") => {
                self.pending
                    .push_back(StreamedMessagePart::Content(ContentPart::Think {
                        think: String::new(),
                        encrypted: block.get("data").and_then(Value::as_str).map(str::to_owned),
                    }));
            }
            Some("tool_use") => {
                let arguments = if streaming {
                    Some(String::new())
                } else {
                    block
                        .get("input")
                        .map(serde_json::to_string)
                        .transpose()
                        .ok()
                        .flatten()
                };
                self.pending
                    .push_back(StreamedMessagePart::ToolCall(ToolCall {
                        call_type: ToolCallType::Function,
                        id: block
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        arguments,
                        extras: None,
                        stream_index: index.map(StreamIndex::Number),
                    }));
            }
            _ => {}
        }
    }

    fn process_message(&mut self, response: Value) {
        self.id = response
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.extract_usage(response.get("usage"));
        self.capture_stop_reason(response.get("stop_reason").and_then(Value::as_str));
        if let Some(content) = response.get("content").and_then(Value::as_array) {
            for block in content {
                self.push_content_block(block, false, None);
            }
        }
    }

    fn process_stream_event(&mut self, event: Value) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(message) = event.get("message") {
                    self.id = message.get("id").and_then(Value::as_str).map(str::to_owned);
                    self.extract_usage(message.get("usage"));
                }
            }
            Some("content_block_start") => {
                let index = event.get("index").and_then(Value::as_i64);
                if let Some(block) = event.get("content_block") {
                    self.push_content_block(block, true, index);
                }
            }
            Some("content_block_delta") => {
                let Some(delta) = event.get("delta") else {
                    return;
                };
                let index = event.get("index").and_then(Value::as_i64);
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        self.pending
                            .push_back(StreamedMessagePart::Content(ContentPart::Text {
                                text: delta
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                            }))
                    }
                    Some("thinking_delta") => {
                        self.pending
                            .push_back(StreamedMessagePart::Content(ContentPart::Think {
                                think: delta
                                    .get("thinking")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                encrypted: None,
                            }));
                    }
                    Some("input_json_delta") => {
                        self.pending
                            .push_back(StreamedMessagePart::ToolCallPart(ToolCallPart {
                                part_type: ToolCallPartType::ToolCallPart,
                                arguments_part: delta
                                    .get("partial_json")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                index: index.map(StreamIndex::Number),
                            }))
                    }
                    Some("signature_delta") => {
                        self.pending
                            .push_back(StreamedMessagePart::Content(ContentPart::Think {
                                think: String::new(),
                                encrypted: delta
                                    .get("signature")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            }));
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(usage) = event.get("usage") {
                    for (key, target) in [
                        ("output_tokens", &mut self.usage.output),
                        ("cache_read_input_tokens", &mut self.usage.input_cache_read),
                        (
                            "cache_creation_input_tokens",
                            &mut self.usage.input_cache_creation,
                        ),
                        ("input_tokens", &mut self.usage.input_other),
                    ] {
                        if let Some(value) = usage.get(key) {
                            *target = counter_from_json(Some(value));
                        }
                    }
                }
                if let Some(delta) = event.get("delta")
                    && delta.get("stop_reason").is_some()
                {
                    self.capture_stop_reason(delta.get("stop_reason").and_then(Value::as_str));
                }
            }
            _ => {}
        }
    }
}

impl Stream for AnthropicStreamedMessage {
    type Item = Result<StreamedMessagePart, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(part) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(part)));
            }
            match self.source.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(AnthropicResponseEvent::Message(response)))) => {
                    self.process_message(response);
                }
                Poll::Ready(Some(Ok(AnthropicResponseEvent::Stream(event)))) => {
                    self.process_stream_event(event);
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl StreamedMessage for AnthropicStreamedMessage {
    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn usage(&self) -> Option<&TokenUsage> {
        Some(&self.usage)
    }

    fn finish_reason(&self) -> Option<FinishReason> {
        self.finish_reason
    }

    fn raw_finish_reason(&self) -> Option<&str> {
        self.raw_finish_reason.as_deref()
    }

    fn trace_id(&self) -> TraceId<'_> {
        TraceId::Absent
    }
}

pub struct AnthropicOptions {
    pub model: String,
    pub stream: Option<bool>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub default_max_tokens: Option<u64>,
    pub beta_features: Option<Vec<String>>,
    pub default_headers: Option<IndexMap<String, String>>,
    pub metadata: Option<IndexMap<String, String>>,
    pub adaptive_thinking: Option<bool>,
    pub support_efforts: Option<Vec<String>>,
    pub beta_api: Option<bool>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub hooks: Option<AnthropicHooks>,
    pub http_client: Option<reqwest::Client>,
    pub client_factory: Option<AnthropicClientFactory>,
}

pub type AnthropicClientFactory = Arc<
    dyn Fn(ProviderRequestAuth) -> Result<Arc<dyn AnthropicClient>, ProviderError> + Send + Sync,
>;

impl AnthropicOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            stream: None,
            api_key: None,
            base_url: None,
            default_max_tokens: None,
            beta_features: None,
            default_headers: None,
            metadata: None,
            adaptive_thinking: None,
            support_efforts: None,
            beta_api: None,
            thinking_effort: None,
            hooks: None,
            http_client: None,
            client_factory: None,
        }
    }
}

// Original: anthropic.ts, AnthropicChatProvider
pub struct AnthropicChatProvider {
    model: String,
    stream: bool,
    api_key: Option<String>,
    base_url: String,
    default_headers: Option<IndexMap<String, String>>,
    generation_kwargs: AnthropicGenerationKwargs,
    metadata: Option<IndexMap<String, String>>,
    adaptive_thinking: Option<bool>,
    support_efforts: Option<Vec<String>>,
    beta_api: bool,
    thinking_effort: Option<ThinkingEffort>,
    explicit_max_tokens: bool,
    hooks: Option<AnthropicHooks>,
    http_client: reqwest::Client,
    cached_client: Option<Arc<dyn AnthropicClient>>,
    client_factory: Option<AnthropicClientFactory>,
}

impl AnthropicChatProvider {
    pub fn new(options: AnthropicOptions) -> Self {
        let explicit_max_tokens = options.default_max_tokens.is_some();
        let max_tokens = options
            .default_max_tokens
            .unwrap_or_else(|| resolve_default_max_tokens(&options.model, None));
        let betas = options
            .beta_features
            .unwrap_or_else(|| vec![INTERLEAVED_THINKING_BETA.to_owned()]);
        let generation_kwargs = Map::from_iter([
            ("max_tokens".to_owned(), Value::from(max_tokens)),
            (
                "betaFeatures".to_owned(),
                Value::Array(betas.into_iter().map(Value::String).collect()),
            ),
        ]);
        let api_key = options.api_key.filter(|api_key| !api_key.is_empty());
        let base_url = options
            .base_url
            .unwrap_or_else(|| "https://api.anthropic.com".to_owned());
        let http_client = options
            .http_client
            .unwrap_or_else(default_provider_http_client);
        let cached_client = api_key.as_ref().map(|api_key| {
            Arc::new(ReqwestAnthropicClient::new(
                http_client.clone(),
                base_url.clone(),
                api_key.clone(),
                options.default_headers.clone(),
            )) as Arc<dyn AnthropicClient>
        });
        Self {
            model: options.model,
            stream: options.stream.unwrap_or(true),
            api_key,
            base_url,
            default_headers: options.default_headers,
            generation_kwargs,
            metadata: options.metadata,
            adaptive_thinking: options.adaptive_thinking,
            support_efforts: options.support_efforts,
            beta_api: options.beta_api.unwrap_or(false),
            thinking_effort: options.thinking_effort,
            explicit_max_tokens,
            hooks: options.hooks,
            http_client,
            cached_client,
            client_factory: options.client_factory,
        }
    }

    fn require_api_key(&self, auth: Option<&ProviderRequestAuth>) -> Result<String, ProviderError> {
        let api_key = auth
            .and_then(|auth| auth.api_key.as_deref())
            .or(self.api_key.as_deref());
        match api_key {
            Some(api_key) if !api_key.is_empty() => Ok(api_key.to_owned()),
            _ => Err(Box::new(ChatProviderError::ChatProvider {
                message: "AnthropicChatProvider: apiKey is required. Provide it via constructor options, options.auth.apiKey on each request, or an OAuth login. The Anthropic adapter does not read shell API-key environment variables.".to_owned(),
            })),
        }
    }

    // Original: AnthropicChatProvider._createClient().
    fn create_client(
        &self,
        auth: Option<&ProviderRequestAuth>,
    ) -> Result<Arc<dyn AnthropicClient>, ProviderError> {
        if let Some(factory) = self.client_factory.as_ref() {
            return factory(auth.cloned().unwrap_or_default());
        }
        if auth.is_none()
            && let Some(client) = self.cached_client.as_ref()
        {
            return Ok(Arc::clone(client));
        }
        let api_key = self.require_api_key(auth)?;
        Ok(Arc::new(ReqwestAnthropicClient::new(
            self.http_client.clone(),
            self.base_url.clone(),
            api_key,
            self.default_headers.clone(),
        )))
    }
}

#[async_trait]
impl ChatProvider for AnthropicChatProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn thinking_effort(&self) -> Option<&ThinkingEffort> {
        self.thinking_effort.as_ref()
    }

    fn max_completion_tokens(&self) -> Option<u64> {
        self.generation_kwargs
            .get("max_tokens")
            .and_then(Value::as_u64)
    }

    async fn generate(
        &self,
        system_prompt: &str,
        tools: &[Tool],
        history: &[Message],
        options: Option<&GenerateOptions>,
    ) -> Result<Box<dyn StreamedMessage>, ProviderError> {
        let prepared = build_anthropic_request(
            &self.model,
            &self.generation_kwargs,
            self.explicit_max_tokens,
            self.metadata.as_ref(),
            self.adaptive_thinking,
            self.support_efforts.as_deref(),
            self.beta_api,
            self.thinking_effort.as_ref(),
            self.hooks.as_ref(),
            system_prompt,
            tools,
            history,
            options,
        )?;
        let client = self.create_client(options.and_then(|options| options.auth.as_ref()))?;
        if let Some(callback) = options.and_then(|options| options.on_request_sent.as_ref()) {
            callback();
        }
        let response = client
            .create(
                prepared.params,
                Some(&prepared.extra_headers),
                self.stream,
                options.and_then(|options| options.signal.as_ref()),
            )
            .await?;
        Ok(match response {
            AnthropicHttpResponse::Message(message) => {
                Box::new(AnthropicStreamedMessage::from_response(message))
            }
            AnthropicHttpResponse::Stream(stream) => {
                Box::new(AnthropicStreamedMessage::from_stream(stream))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::kosong::contract::message::{MediaUrl, ToolCall, ToolCallType};
    use crate::kosong::contract::provider::JsonSchemaDefinition;
    use tokio_util::sync::CancellationToken;

    struct StubAnthropicClient;

    #[async_trait]
    impl AnthropicClient for StubAnthropicClient {
        async fn create(
            &self,
            _: Map<String, Value>,
            _: Option<&IndexMap<String, String>>,
            _: bool,
            _: Option<&CancellationToken>,
        ) -> Result<AnthropicHttpResponse, ProviderError> {
            Ok(AnthropicHttpResponse::Message(
                serde_json::json!({"content": []}),
            ))
        }
    }

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

        assert_eq!(resolve_default_max_tokens("claude-opus-4-8", None), 128_000);
        assert_eq!(
            resolve_default_max_tokens("claude-opus-4-9", Some(200_000)),
            128_000
        );
        assert_eq!(
            resolve_default_max_tokens("vendor-model", Some(12_345)),
            12_345
        );
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

    #[test]
    fn thinking_cache_and_capability_policies_match_transport_rules() {
        let mut kwargs = Map::from_iter([(
            "betaFeatures".to_owned(),
            serde_json::json!([INTERLEAVED_THINKING_BETA, "vendor-beta"]),
        )]);
        encode_thinking(
            "claude-opus-4-6",
            None,
            None,
            &ThinkingEffort::from("max"),
            &mut kwargs,
        );
        assert_eq!(kwargs["thinking"]["type"], "adaptive");
        assert_eq!(kwargs["output_config"]["effort"], "max");
        assert_eq!(kwargs["betaFeatures"], serde_json::json!(["vendor-beta"]));

        apply_thinking_keep(&mut kwargs, "all");
        apply_thinking_keep(&mut kwargs, "recent");
        assert_eq!(
            kwargs["contextManagement"]["edits"],
            serde_json::json!([{"type":CLEAR_THINKING_EDIT,"keep":"recent"}])
        );
        assert_eq!(
            kwargs["betaFeatures"],
            serde_json::json!(["vendor-beta", CONTEXT_MANAGEMENT_BETA])
        );

        let mut messages = vec![serde_json::json!({
            "role":"user","content":[{"type":"text","text":"last"}]
        })];
        inject_cache_control_on_last_block(&mut messages);
        assert_eq!(
            messages[0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(
            get_anthropic_model_capability("claude-sonnet-4-6")
                .unwrap()
                .thinking
        );
        assert!(
            !get_anthropic_model_capability("claude-3-5-sonnet")
                .unwrap()
                .thinking
        );
    }

    #[test]
    fn request_assembly_preserves_overlay_order_and_beta_endpoint_switch() {
        let generation_kwargs = Map::from_iter([
            ("max_tokens".to_owned(), Value::from(128_000)),
            (
                "betaFeatures".to_owned(),
                serde_json::json!([INTERLEAVED_THINKING_BETA]),
            ),
        ]);
        let history = vec![
            Message::new(
                Role::User,
                vec![ContentPart::Text {
                    text: "one".to_owned(),
                }],
                Vec::new(),
            ),
            Message::new(
                Role::User,
                vec![ContentPart::Text {
                    text: "two".to_owned(),
                }],
                Vec::new(),
            ),
        ];
        let tools = vec![Tool {
            name: "read".to_owned(),
            description: "Read".to_owned(),
            parameters: serde_json::json!({"type":"object"})
                .as_object()
                .unwrap()
                .clone(),
            deferred: None,
        }];
        let options = GenerateOptions {
            cache_key: Some("session-1".to_owned()),
            thinking: Some(crate::kosong::contract::provider::ThinkingRequestOptions {
                effort: ThinkingEffort::from("max"),
                keep: Some("recent".to_owned()),
            }),
            max_completion_tokens: Some(10_000),
            used_context_tokens: Some(95),
            max_context_tokens: Some(100),
            ..GenerateOptions::default()
        };
        let request = build_anthropic_request(
            "claude-opus-4-6",
            &generation_kwargs,
            false,
            None,
            None,
            None,
            false,
            None,
            None,
            "system",
            &tools,
            &history,
            Some(&options),
        )
        .unwrap();
        assert!(request.use_beta_api);
        assert!(request.extra_headers.is_empty());
        assert_eq!(request.params["max_tokens"], 5);
        assert_eq!(request.params["metadata"]["user_id"], "session-1");
        assert_eq!(request.params["messages"].as_array().unwrap().len(), 1);
        assert_eq!(
            request.params["messages"][0]["content"][1]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(
            request.params["tools"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(
            request.params["betas"],
            serde_json::json!([CONTEXT_MANAGEMENT_BETA])
        );
    }

    #[tokio::test]
    async fn streamed_message_converts_non_stream_and_incremental_events() {
        let mut message = AnthropicStreamedMessage::from_response(serde_json::json!({
            "id":"msg-1",
            "stop_reason":"tool_use",
            "usage":{"input_tokens":3,"output_tokens":4},
            "content":[
                {"type":"text","text":"answer"},
                {"type":"thinking","thinking":"why","signature":"signed"},
                {"type":"tool_use","id":"call-1","name":"read","input":{"path":"a"}}
            ]
        }));
        let parts = message.by_ref().collect::<Vec<_>>().await;
        assert_eq!(parts.len(), 3);
        assert_eq!(message.id(), Some("msg-1"));
        assert_eq!(message.finish_reason(), Some(FinishReason::ToolCalls));
        assert_eq!(message.usage().unwrap().output, 4);

        let events = futures_util::stream::iter([
            Ok(serde_json::json!({
                "type":"message_start",
                "message":{"id":"msg-2","usage":{"input_tokens":8,"output_tokens":0}}
            })),
            Ok(serde_json::json!({
                "type":"content_block_start","index":2,
                "content_block":{"type":"tool_use","id":"call-2","name":"write"}
            })),
            Ok(serde_json::json!({
                "type":"content_block_delta","index":2,
                "delta":{"type":"input_json_delta","partial_json":"{\"path\":"}
            })),
            Ok(serde_json::json!({
                "type":"message_delta",
                "usage":{"output_tokens":6},
                "delta":{"stop_reason":"end_turn"}
            })),
        ]);
        let mut message = AnthropicStreamedMessage::from_stream(events);
        let parts = message.by_ref().collect::<Vec<_>>().await;
        assert_eq!(parts.len(), 2);
        let StreamedMessagePart::ToolCall(call) = parts[0].as_ref().unwrap() else {
            panic!("expected tool call")
        };
        assert_eq!(call.stream_index, Some(StreamIndex::Number(2)));
        assert_eq!(message.id(), Some("msg-2"));
        assert_eq!(message.finish_reason(), Some(FinishReason::Completed));
        assert_eq!(message.usage().unwrap().input_other, 8);
        assert_eq!(message.usage().unwrap().output, 6);
    }

    #[tokio::test]
    async fn provider_requires_explicit_anthropic_auth_without_shell_fallback() {
        let mut options = AnthropicOptions::new("claude-sonnet-4-6");
        options.api_key = Some(String::new());
        options.default_max_tokens = Some(4_096);
        let provider = AnthropicChatProvider::new(options);
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.max_completion_tokens(), Some(4_096));
        let error = match provider.generate("", &[], &[], None).await {
            Ok(_) => panic!("missing auth must fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "AnthropicChatProvider: apiKey is required. Provide it via constructor options, options.auth.apiKey on each request, or an OAuth login. The Anthropic adapter does not read shell API-key environment variables."
        );
    }

    #[test]
    fn anthropic_client_factory_wins_and_cache_only_serves_absent_auth() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let factory_received = Arc::clone(&received);
        let mut factory_options = AnthropicOptions::new("claude-sonnet-4-6");
        factory_options.client_factory = Some(Arc::new(move |auth| {
            factory_received.lock().unwrap().push(auth);
            Ok(Arc::new(StubAnthropicClient))
        }));
        let factory_provider = AnthropicChatProvider::new(factory_options);
        factory_provider.create_client(None).unwrap();
        let auth = ProviderRequestAuth {
            api_key: Some("request-key".into()),
            headers: Some(IndexMap::from([("x-request".into(), "yes".into())])),
        };
        factory_provider.create_client(Some(&auth)).unwrap();
        assert_eq!(
            *received.lock().unwrap(),
            vec![ProviderRequestAuth::default(), auth.clone()]
        );

        let mut cached_options = AnthropicOptions::new("claude-sonnet-4-6");
        cached_options.api_key = Some("default-key".into());
        let cached_provider = AnthropicChatProvider::new(cached_options);
        let first = cached_provider.create_client(None).unwrap();
        let second = cached_provider.create_client(None).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let rebuilt = cached_provider.create_client(Some(&auth)).unwrap();
        assert!(!Arc::ptr_eq(&first, &rebuilt));
    }
}
