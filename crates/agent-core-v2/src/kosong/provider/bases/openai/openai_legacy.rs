use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};

use crate::kosong::contract::capability::ModelCapability;
use crate::kosong::contract::message::{
    ContentPart, Message, Role, StreamIndex, StreamedMessagePart, ToolCall, ToolCallType,
    is_tool_declaration_only_message,
};
use crate::kosong::contract::provider::{
    ChatProvider, FinishReason, GenerateOptions, ProviderError, ResponseFormat, StreamedMessage,
    ThinkingEffort, ToolCallIdPolicy, TraceId, VideoUploadSource,
};
use crate::kosong::contract::tool::Tool;
use crate::kosong::contract::usage::TokenUsage;
use crate::kosong::provider::bases::http_client::default_provider_http_client;
use crate::kosong::provider::bases::openai::chat_completions_stream::{
    BufferedChatCompletionToolCall, ChatCompletionStreamToolCallDelta,
    convert_chat_completion_stream_tool_call,
};
use crate::kosong::provider::bases::openai::openai_common::{
    ConvertedToolMessageContent, OPENAI_REASONING_CAPABILITY, OPENAI_TEXT_TOOL_CAPABILITY,
    OPENAI_VISION_TOOL_CAPABILITY, OPENAI_VISION_TOOL_PREFIXES, OpenAiContentPart,
    TOOL_RESULT_MEDIA_PLACEHOLDER, TOOL_RESULT_MEDIA_PROMPT, ToolMessageConversion,
    convert_content_part, convert_tool_message_content, extract_usage, has_model_prefix,
    is_openai_reasoning_model, normalize_openai_finish_reason, tool_to_openai,
};
use crate::kosong::provider::bases::openai::openai_hooks::{
    BoundExtractUsageHook, OpenAiChatHooks,
};
use crate::kosong::provider::bases::openai::openai_legacy_transport::{
    OpenAiLegacyHttpResponse, send_openai_legacy_request,
};
use crate::kosong::provider::bases::request_auth::{
    merge_request_headers, require_provider_api_key,
};
use crate::kosong::provider::bases::tool_call_id::{
    ToolCallIdError, normalize_tool_call_ids_for_provider, sanitize_tool_call_id,
};

pub const KNOWN_REASONING_KEYS: [&str; 3] = ["reasoning_content", "reasoning_details", "reasoning"];
pub const DEFAULT_OUTBOUND_REASONING_KEY: &str = KNOWN_REASONING_KEYS[0];
pub const CHAT_COMPLETIONS_MAX_OUTPUT_TOKENS_CEILING: u64 = 128 * 1024;

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
//
// Rust adaptation: token counts are u64, so they always serialize as JSON
// integers. Strict gateways declare max_tokens as an unsigned integer and
// reject `131072.0`.
pub fn completion_token_kwargs(
    model: &str,
    max_completion_tokens: u64,
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
    for part in message.content.iter() {
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
    if message.role == Role::Assistant
        && has_reasoning_part
        && !result.contains_key("content")
        && !result.contains_key("tool_calls")
    {
        result.insert("content".to_owned(), Value::String(String::new()));
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
                    &crate::kosong::protocol::protocol_trait::ThinkingHookOptions { keep },
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
            && max > 0
        {
            cap = cap.min(max.saturating_sub(used));
        }
        cap = cap.max(1);
        let hooked = hooks
            .and_then(|hooks| hooks.with_max_completion_tokens.as_ref())
            .and_then(|hook| hook(cap));
        if let Some(hooked) = hooked {
            kwargs.extend(hooked);
        } else {
            kwargs.extend(completion_token_kwargs(
                model,
                cap.clamp(1, CHAT_COMPLETIONS_MAX_OUTPUT_TOKENS_CEILING),
            ));
        }
    }

    ResolvedRequestKwargs {
        kwargs,
        reasoning_effort,
    }
}

// Original: openai-legacy.ts, getOpenAILegacyModelCapability()
pub fn get_openai_legacy_model_capability(model_name: &str) -> Option<&'static ModelCapability> {
    let normalized = model_name.to_ascii_lowercase();
    if is_openai_reasoning_model(&normalized) {
        Some(&OPENAI_REASONING_CAPABILITY)
    } else if has_model_prefix(&normalized, &OPENAI_VISION_TOOL_PREFIXES) {
        Some(&OPENAI_VISION_TOOL_CAPABILITY)
    } else if normalized.starts_with("gpt-3.5-turbo") {
        Some(&OPENAI_TEXT_TOOL_CAPABILITY)
    } else {
        None
    }
}

// Original: openai-legacy.ts, OpenAILegacyChatProvider.generate() request
// construction before the SDK call.
#[allow(clippy::too_many_arguments)]
pub fn build_openai_legacy_request(
    model: &str,
    stream: bool,
    reasoning_key: Option<&str>,
    generation_kwargs: &OpenAiLegacyGenerationKwargs,
    default_thinking_effort: Option<&ThinkingEffort>,
    tool_message_conversion: ToolMessageConversion,
    hooks: Option<&OpenAiChatHooks>,
    system_prompt: &str,
    tools: &[Tool],
    history: &[Message],
    options: Option<&GenerateOptions>,
) -> Result<Map<String, Value>, ToolCallIdError> {
    let resolved = resolve_request_kwargs(
        model,
        generation_kwargs,
        default_thinking_effort,
        hooks,
        history,
        options,
    );
    let preserve_thinking = hooks
        .and_then(|hooks| hooks.preserve_thinking.as_ref())
        .and_then(|hook| hook(&resolved.kwargs))
        .unwrap_or(false);
    let policy = hooks
        .and_then(|hooks| hooks.tool_call_id_policy.as_ref())
        .and_then(|hook| hook())
        .unwrap_or_else(|| OPENAI_CHAT_TOOL_CALL_ID_POLICY.clone());
    let normalized_history = normalize_tool_call_ids_for_provider(history.to_vec(), &policy)?;

    let mut messages = Vec::new();
    if !system_prompt.is_empty() {
        messages.push(Map::from_iter([
            ("role".to_owned(), Value::String("system".to_owned())),
            (
                "content".to_owned(),
                Value::String(system_prompt.to_owned()),
            ),
        ]));
    }
    if let Some(convert_message_hook) = hooks.and_then(|hooks| hooks.convert_message.as_ref()) {
        for message in &normalized_history {
            let converted = convert_message(
                message,
                reasoning_key,
                ToolMessageConversion::Parts,
                preserve_thinking,
                false,
            );
            if let Some(shaped) = convert_message_hook(message, converted) {
                messages.push(shaped);
            }
        }
    } else {
        messages.extend(convert_history_messages(
            &normalized_history,
            reasoning_key,
            tool_message_conversion,
            preserve_thinking,
        ));
    }
    if let Some(merge_history) = hooks.and_then(|hooks| hooks.merge_history.as_ref()) {
        messages = merge_history(&messages);
    }

    let mut params = Map::from_iter([
        ("model".to_owned(), Value::String(model.to_owned())),
        (
            "messages".to_owned(),
            Value::Array(messages.into_iter().map(Value::Object).collect()),
        ),
        ("stream".to_owned(), Value::Bool(stream)),
    ]);
    params.extend(resolved.kwargs);
    if !tools.is_empty() {
        let converted = tools
            .iter()
            .map(|tool| {
                hooks
                    .and_then(|hooks| hooks.convert_tool.as_ref())
                    .and_then(|hook| hook(tool))
                    .map(Value::Object)
                    .unwrap_or_else(|| {
                        if hooks.is_some_and(|hooks| hooks.convert_tool.is_some()) {
                            Value::Null
                        } else {
                            serde_json::json!(tool_to_openai(tool))
                        }
                    })
            })
            .collect();
        params.insert("tools".to_owned(), Value::Array(converted));
    }
    if let Some(response_format) = options.and_then(|options| options.response_format.as_ref()) {
        params.insert(
            "response_format".to_owned(),
            Value::Object(response_format_to_openai(response_format)),
        );
    }
    if stream {
        params.insert(
            "stream_options".to_owned(),
            serde_json::json!({"include_usage":true}),
        );
    }
    if let Some(reasoning_effort) = resolved.reasoning_effort {
        params.insert(
            "reasoning_effort".to_owned(),
            Value::String(reasoning_effort),
        );
    }
    if let Some(build_params) = hooks.and_then(|hooks| hooks.build_params.as_ref()) {
        params = build_params(params);
    }
    Ok(params)
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiLegacyFunctionCall {
    pub name: String,
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiLegacyToolCall {
    pub call_type: String,
    pub id: String,
    pub function: OpenAiLegacyFunctionCall,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenAiLegacyMessagePayload {
    pub fields: Map<String, Value>,
    pub content: Option<String>,
    pub tool_calls: Vec<OpenAiLegacyToolCall>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenAiLegacyDelta {
    pub fields: Map<String, Value>,
    pub content: Option<String>,
    pub tool_calls: Vec<ChatCompletionStreamToolCallDelta>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenAiLegacyChoice {
    pub finish_reason: Option<String>,
    pub message: Option<OpenAiLegacyMessagePayload>,
    pub delta: Option<OpenAiLegacyDelta>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenAiLegacyCompletion {
    pub id: String,
    pub usage: Option<Value>,
    pub raw: Map<String, Value>,
    pub choices: Vec<OpenAiLegacyChoice>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenAiLegacyChunk {
    pub id: Option<String>,
    pub usage: Option<Value>,
    pub raw: Map<String, Value>,
    pub choices: Vec<OpenAiLegacyChoice>,
}

enum OpenAiLegacyEvent {
    Completion(OpenAiLegacyCompletion),
    Chunk(OpenAiLegacyChunk),
}

// Original: openai-legacy.ts, OpenAILegacyStreamedMessage
pub struct OpenAiLegacyStreamedMessage {
    source: Pin<Box<dyn Stream<Item = Result<OpenAiLegacyEvent, ProviderError>> + Send>>,
    pending: VecDeque<StreamedMessagePart>,
    buffered_tool_calls: HashMap<StreamIndex, BufferedChatCompletionToolCall>,
    reasoning_key: Option<String>,
    extract_usage_hook: Option<BoundExtractUsageHook>,
    id: Option<String>,
    usage: Option<TokenUsage>,
    finish_reason: Option<FinishReason>,
    raw_finish_reason: Option<String>,
    trace_id: Option<String>,
}

impl OpenAiLegacyStreamedMessage {
    pub fn from_completion(
        response: OpenAiLegacyCompletion,
        reasoning_key: Option<String>,
        trace_id: Option<String>,
        extract_usage_hook: Option<BoundExtractUsageHook>,
    ) -> Self {
        Self::new(
            futures_util::stream::iter([Ok(OpenAiLegacyEvent::Completion(response))]),
            reasoning_key,
            trace_id,
            extract_usage_hook,
        )
    }

    pub fn from_stream<S>(
        response: S,
        reasoning_key: Option<String>,
        trace_id: Option<String>,
        extract_usage_hook: Option<BoundExtractUsageHook>,
    ) -> Self
    where
        S: Stream<Item = Result<OpenAiLegacyChunk, ProviderError>> + Send + 'static,
    {
        Self::new(
            response.map(|result| result.map(OpenAiLegacyEvent::Chunk)),
            reasoning_key,
            trace_id,
            extract_usage_hook,
        )
    }

    fn new<S>(
        source: S,
        reasoning_key: Option<String>,
        trace_id: Option<String>,
        extract_usage_hook: Option<BoundExtractUsageHook>,
    ) -> Self
    where
        S: Stream<Item = Result<OpenAiLegacyEvent, ProviderError>> + Send + 'static,
    {
        Self {
            source: Box::pin(source),
            pending: VecDeque::new(),
            buffered_tool_calls: HashMap::new(),
            reasoning_key,
            extract_usage_hook,
            id: None,
            usage: None,
            finish_reason: None,
            raw_finish_reason: None,
            trace_id,
        }
    }

    fn capture_finish_reason(&mut self, raw: Option<&str>) {
        let normalized = normalize_openai_finish_reason(raw);
        self.finish_reason = normalized.finish_reason;
        self.raw_finish_reason = normalized.raw_finish_reason;
    }

    fn capture_usage(&mut self, raw: &Map<String, Value>, fallback: Option<&Value>) {
        let raw_usage = match self.extract_usage_hook.as_ref().map(|hook| hook(raw)) {
            Some(crate::kosong::protocol::protocol_trait::UsageExtraction::Usage(usage)) => {
                Some(Value::Object(usage))
            }
            Some(crate::kosong::protocol::protocol_trait::UsageExtraction::NoUsage) => None,
            Some(crate::kosong::protocol::protocol_trait::UsageExtraction::Defer) | None => {
                fallback.cloned()
            }
        };
        if let Some(raw_usage) = raw_usage.as_ref() {
            self.usage = extract_usage(raw_usage);
        }
    }

    fn process_completion(&mut self, response: OpenAiLegacyCompletion) {
        self.id = Some(response.id);
        self.capture_usage(&response.raw, response.usage.as_ref());
        let Some(choice) = response.choices.first() else {
            self.capture_finish_reason(None);
            return;
        };
        self.capture_finish_reason(choice.finish_reason.as_deref());
        let Some(message) = choice.message.as_ref() else {
            return;
        };
        if let Some(reasoning) = extract_reasoning_content(
            &Value::Object(message.fields.clone()),
            self.reasoning_key.as_deref(),
        ) {
            self.pending
                .push_back(StreamedMessagePart::Content(ContentPart::Think {
                    think: reasoning,
                    encrypted: None,
                }));
        }
        if let Some(content) = message
            .content
            .as_ref()
            .filter(|content| !content.is_empty())
        {
            self.pending
                .push_back(StreamedMessagePart::Content(ContentPart::Text {
                    text: content.clone(),
                }));
        }
        for tool_call in &message.tool_calls {
            if tool_call.call_type != "function" {
                continue;
            }
            self.pending
                .push_back(StreamedMessagePart::ToolCall(ToolCall {
                    call_type: ToolCallType::Function,
                    id: if tool_call.id.is_empty() {
                        uuid::Uuid::new_v4().to_string()
                    } else {
                        tool_call.id.clone()
                    },
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                    extras: None,
                    stream_index: None,
                }));
        }
    }

    fn process_chunk(&mut self, chunk: OpenAiLegacyChunk) {
        if let Some(id) = chunk.id.filter(|id| !id.is_empty()) {
            self.id = Some(id);
        }
        self.capture_usage(&chunk.raw, chunk.usage.as_ref());
        let Some(choice) = chunk.choices.first() else {
            return;
        };
        if choice.finish_reason.is_some() {
            self.capture_finish_reason(choice.finish_reason.as_deref());
        }
        let Some(delta) = choice.delta.as_ref() else {
            return;
        };
        if let Some(reasoning) = extract_reasoning_content(
            &Value::Object(delta.fields.clone()),
            self.reasoning_key.as_deref(),
        ) {
            self.pending
                .push_back(StreamedMessagePart::Content(ContentPart::Think {
                    think: reasoning,
                    encrypted: None,
                }));
        }
        if let Some(content) = delta.content.as_ref().filter(|content| !content.is_empty()) {
            self.pending
                .push_back(StreamedMessagePart::Content(ContentPart::Text {
                    text: content.clone(),
                }));
        }
        for tool_call in &delta.tool_calls {
            self.pending
                .extend(convert_chat_completion_stream_tool_call(
                    tool_call,
                    &mut self.buffered_tool_calls,
                ));
        }
    }
}

impl Stream for OpenAiLegacyStreamedMessage {
    type Item = Result<StreamedMessagePart, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(part) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(part)));
            }
            match self.source.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(OpenAiLegacyEvent::Completion(response)))) => {
                    self.process_completion(response);
                }
                Poll::Ready(Some(Ok(OpenAiLegacyEvent::Chunk(chunk)))) => {
                    self.process_chunk(chunk);
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl StreamedMessage for OpenAiLegacyStreamedMessage {
    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn usage(&self) -> Option<&TokenUsage> {
        self.usage.as_ref()
    }

    fn finish_reason(&self) -> Option<FinishReason> {
        self.finish_reason
    }

    fn raw_finish_reason(&self) -> Option<&str> {
        self.raw_finish_reason.as_deref()
    }

    fn trace_id(&self) -> TraceId<'_> {
        TraceId::Present(self.trace_id.as_deref())
    }
}

pub struct OpenAiLegacyOptions {
    pub model: String,
    pub stream: Option<bool>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub default_headers: Option<IndexMap<String, String>>,
    pub reasoning_key: Option<String>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub max_tokens: Option<u64>,
    pub tool_message_conversion: Option<ToolMessageConversion>,
    pub hooks: Option<OpenAiChatHooks>,
    pub http_client: Option<reqwest::Client>,
}

impl OpenAiLegacyOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            stream: None,
            api_key: None,
            base_url: None,
            default_headers: None,
            reasoning_key: None,
            thinking_effort: None,
            max_tokens: None,
            tool_message_conversion: None,
            hooks: None,
            http_client: None,
        }
    }
}

// Original:
//   packages/agent-core-v2/src/kosong/provider/bases/openai/openai-legacy.ts
//   OpenAILegacyChatProvider
//
// Rust adaptation:
//   The OpenAI SDK client becomes a reusable reqwest client. Request shaping
//   stays in build_openai_legacy_request(), and the transport boundary keeps
//   authentication, cancellation, response headers, JSON and SSE handling in
//   their original call order.
pub struct OpenAiLegacyChatProvider {
    model: String,
    stream: bool,
    api_key: Option<String>,
    base_url: String,
    default_headers: Option<IndexMap<String, String>>,
    reasoning_key: Option<String>,
    thinking_effort: Option<ThinkingEffort>,
    generation_kwargs: OpenAiLegacyGenerationKwargs,
    tool_message_conversion: ToolMessageConversion,
    hooks: Option<OpenAiChatHooks>,
    http_client: reqwest::Client,
}

impl OpenAiLegacyChatProvider {
    pub fn new(options: OpenAiLegacyOptions) -> Self {
        // Presence, rather than non-emptiness, controls the environment
        // fallback. This preserves the contrib factory's explicit empty-key
        // sentinel for vendor endpoints.
        let api_key = match options.api_key {
            Some(api_key) => Some(api_key),
            None => std::env::var("OPENAI_API_KEY").ok(),
        }
        .filter(|api_key| !api_key.is_empty());
        let normalized_reasoning_key = options
            .reasoning_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                options
                    .hooks
                    .as_ref()
                    .and_then(|hooks| hooks.reasoning_key.as_ref())
                    .and_then(|hook| hook())
            });
        let generation_kwargs = normalize_generation_kwargs(
            &options.model,
            &options.max_tokens.map_or_else(Map::new, |tokens| {
                completion_token_kwargs(&options.model, tokens)
            }),
        );
        Self {
            model: options.model,
            stream: options.stream.unwrap_or(true),
            api_key,
            base_url: options
                .base_url
                .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
            default_headers: options.default_headers,
            reasoning_key: normalized_reasoning_key,
            thinking_effort: options.thinking_effort,
            generation_kwargs,
            tool_message_conversion: options
                .tool_message_conversion
                .unwrap_or(ToolMessageConversion::Parts),
            hooks: options.hooks,
            http_client: options
                .http_client
                .unwrap_or_else(default_provider_http_client),
        }
    }
}

#[async_trait]
impl ChatProvider for OpenAiLegacyChatProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn thinking_effort(&self) -> Option<&ThinkingEffort> {
        self.thinking_effort.as_ref()
    }

    fn max_completion_tokens(&self) -> Option<u64> {
        self.generation_kwargs
            .get("max_completion_tokens")
            .or_else(|| self.generation_kwargs.get("max_tokens"))
            .and_then(Value::as_u64)
    }

    async fn generate(
        &self,
        system_prompt: &str,
        tools: &[Tool],
        history: &[Message],
        options: Option<&GenerateOptions>,
    ) -> Result<Box<dyn StreamedMessage>, ProviderError> {
        let api_key = require_provider_api_key(
            "OpenAILegacyChatProvider",
            options.and_then(|options| options.auth.as_ref()),
            self.api_key.as_deref(),
        )?;
        let headers = merge_request_headers(
            self.default_headers.as_ref(),
            options
                .and_then(|options| options.auth.as_ref())
                .and_then(|auth| auth.headers.as_ref()),
        );
        let params = build_openai_legacy_request(
            &self.model,
            self.stream,
            self.reasoning_key.as_deref(),
            &self.generation_kwargs,
            self.thinking_effort.as_ref(),
            self.tool_message_conversion,
            self.hooks.as_ref(),
            system_prompt,
            tools,
            history,
            options,
        )?;
        if let Some(callback) = options.and_then(|options| options.on_request_sent.as_ref()) {
            callback();
        }
        let response = send_openai_legacy_request(
            &self.http_client,
            &self.base_url,
            &api_key,
            headers.as_ref(),
            params,
            self.stream,
            options.and_then(|options| options.signal.as_ref()),
        )
        .await?;
        let extract_usage = self
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.extract_usage.clone());
        let message: Box<dyn StreamedMessage> = match response {
            OpenAiLegacyHttpResponse::Completion { value, trace_id } => {
                Box::new(OpenAiLegacyStreamedMessage::from_completion(
                    value,
                    self.reasoning_key.clone(),
                    trace_id,
                    extract_usage,
                ))
            }
            OpenAiLegacyHttpResponse::Stream { value, trace_id } => {
                Box::new(OpenAiLegacyStreamedMessage::from_stream(
                    value,
                    self.reasoning_key.clone(),
                    trace_id,
                    extract_usage,
                ))
            }
        };
        Ok(message)
    }

    async fn upload_video(
        &self,
        input: VideoUploadSource,
        options: Option<&GenerateOptions>,
    ) -> Result<Option<ContentPart>, ProviderError> {
        let Some(upload) = self
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.upload_video.as_ref())
        else {
            return Ok(None);
        };
        upload(input, options.cloned()).await.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::{MediaUrl, ToolCall, ToolCallType};
    use crate::kosong::contract::provider::JsonSchemaDefinition;
    use crate::kosong::contract::provider::{SamplingOptions, ThinkingRequestOptions};
    use crate::kosong::contract::tool::Tool;
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
            completion_token_kwargs("o1", 8192),
            json!({"max_completion_tokens":8192})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(
            completion_token_kwargs("gpt-4o", 4096),
            json!({"max_tokens":4096}).as_object().unwrap().clone()
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
        assert_eq!(CHAT_COMPLETIONS_MAX_OUTPUT_TOKENS_CEILING, 131_072);
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
        declaration.tools = Some(Arc::new(vec![Tool {
            name: "ignored".to_owned(),
            description: "ignored".to_owned(),
            parameters: Map::new(),
            deferred: None,
        }]));
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
    fn reasoning_only_assistant_has_explicit_empty_content_for_strict_gateways() {
        let assistant = Message::new(
            Role::Assistant,
            vec![ContentPart::Think {
                think: "earlier reasoning".to_owned(),
                encrypted: None,
            }],
            Vec::new(),
        );

        let converted = convert_message(
            &assistant,
            Some("reasoning_content"),
            ToolMessageConversion::Parts,
            false,
            true,
        );

        assert_eq!(
            converted,
            json!({
                "role": "assistant",
                "content": "",
                "reasoning_content": "earlier reasoning",
            })
            .as_object()
            .unwrap()
            .clone()
        );
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
            max_completion_tokens: Some(100),
            used_context_tokens: Some(95),
            max_context_tokens: Some(100),
            ..GenerateOptions::default()
        };
        let resolved =
            resolve_request_kwargs("gpt-4o", &Map::new(), None, None, &history, Some(&options));
        assert_eq!(resolved.kwargs["prompt_cache_key"], "session");
        assert_eq!(resolved.kwargs["temperature"], 0.4);
        assert_eq!(resolved.kwargs["top_p"], 0.8);
        assert_eq!(resolved.kwargs["max_tokens"], 5);
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
                assert_eq!(cap, 1);
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
            max_completion_tokens: Some(100),
            used_context_tokens: Some(120),
            max_context_tokens: Some(100),
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
        assert_eq!(resolved.kwargs["custom_max"], 1);
        assert_eq!(resolved.reasoning_effort, None);

        let off_effort = ThinkingEffort::from("off");
        let off =
            resolve_request_kwargs("o1", &Map::new(), Some(&off_effort), None, &history, None);
        assert_eq!(off.reasoning_effort, None);
    }

    #[test]
    fn legacy_capability_catalog_routes_known_model_families() {
        for (model, expected) in [
            ("O1-mini", Some(&OPENAI_REASONING_CAPABILITY)),
            ("gpt-4.1-mini", Some(&OPENAI_VISION_TOOL_CAPABILITY)),
            ("GPT-3.5-TURBO-0125", Some(&OPENAI_TEXT_TOOL_CAPABILITY)),
            ("unknown-model", None),
        ] {
            assert_eq!(get_openai_legacy_model_capability(model), expected);
        }
    }

    #[test]
    fn request_builder_runs_trait_mode_and_build_params_last() {
        let mut declaration = Message::new(Role::User, Vec::new(), Vec::new());
        declaration.tools = Some(Arc::new(vec![Tool {
            name: "declared".to_owned(),
            description: "declaration message".to_owned(),
            parameters: Map::new(),
            deferred: None,
        }]));
        let user = Message::new(
            Role::User,
            vec![ContentPart::Text {
                text: "hello".to_owned(),
            }],
            Vec::new(),
        );
        let hooks = OpenAiChatHooks {
            convert_message: Some(Arc::new(|message, mut converted| {
                if message.tools.is_some() {
                    None
                } else {
                    converted.insert("shaped".to_owned(), Value::Bool(true));
                    Some(converted)
                }
            })),
            merge_history: Some(Arc::new(|messages| {
                let mut messages = messages.to_vec();
                messages.push(Map::from_iter([(
                    "role".to_owned(),
                    Value::String("merge-marker".to_owned()),
                )]));
                messages
            })),
            build_params: Some(Arc::new(|mut params| {
                assert_eq!(params["stream_options"]["include_usage"], true);
                assert_eq!(params["response_format"]["type"], "json_object");
                params.insert("built_last".to_owned(), Value::Bool(true));
                params
            })),
            ..OpenAiChatHooks::default()
        };
        let options = GenerateOptions {
            response_format: Some(ResponseFormat::JsonObject),
            ..GenerateOptions::default()
        };
        let params = build_openai_legacy_request(
            "gpt-4o",
            true,
            None,
            &Map::new(),
            None,
            ToolMessageConversion::Parts,
            Some(&hooks),
            "system",
            &[Tool {
                name: "read".to_owned(),
                description: "Read".to_owned(),
                parameters: Map::new(),
                deferred: None,
            }],
            &[declaration, user],
            Some(&options),
        )
        .unwrap();
        assert_eq!(params["messages"].as_array().unwrap().len(), 3);
        assert_eq!(params["messages"][0]["role"], "system");
        assert_eq!(params["messages"][1]["shaped"], true);
        assert_eq!(params["messages"][2]["role"], "merge-marker");
        assert_eq!(params["tools"][0]["type"], "function");
        assert_eq!(params["built_last"], true);
    }

    #[tokio::test]
    async fn streamed_message_converts_both_response_modes_and_captures_metadata() {
        let index = StreamIndex::Number(0);
        let chunks = futures_util::stream::iter(vec![
            Ok(OpenAiLegacyChunk {
                id: Some("chat-1".to_owned()),
                choices: vec![OpenAiLegacyChoice {
                    delta: Some(OpenAiLegacyDelta {
                        fields: Map::from_iter([(
                            "reasoning_content".to_owned(),
                            Value::String("think".to_owned()),
                        )]),
                        tool_calls: vec![ChatCompletionStreamToolCallDelta {
                            index: Some(index.clone()),
                            id: Some("call-1".to_owned()),
                            function: Some(
                                crate::kosong::provider::bases::openai::chat_completions_stream::ChatCompletionStreamToolFunctionDelta {
                                    name: None,
                                    arguments: Some("{\"x\":".to_owned()),
                                },
                            ),
                        }],
                        ..OpenAiLegacyDelta::default()
                    }),
                    ..OpenAiLegacyChoice::default()
                }],
                ..OpenAiLegacyChunk::default()
            }),
            Ok(OpenAiLegacyChunk {
                choices: vec![OpenAiLegacyChoice {
                    finish_reason: Some("stop".to_owned()),
                    delta: Some(OpenAiLegacyDelta {
                        content: Some("hello".to_owned()),
                        tool_calls: vec![ChatCompletionStreamToolCallDelta {
                            index: Some(index),
                            id: None,
                            function: Some(
                                crate::kosong::provider::bases::openai::chat_completions_stream::ChatCompletionStreamToolFunctionDelta {
                                    name: Some("run".to_owned()),
                                    arguments: Some("1}".to_owned()),
                                },
                            ),
                        }],
                        ..OpenAiLegacyDelta::default()
                    }),
                    ..OpenAiLegacyChoice::default()
                }],
                ..OpenAiLegacyChunk::default()
            }),
            Ok(OpenAiLegacyChunk {
                usage: Some(json!({
                    "prompt_tokens": 10,
                    "completion_tokens": 3,
                    "prompt_tokens_details": {"cached_tokens": 2}
                })),
                ..OpenAiLegacyChunk::default()
            }),
        ]);
        let mut streamed = OpenAiLegacyStreamedMessage::from_stream(
            chunks,
            None,
            Some("trace-1".to_owned()),
            None,
        );
        let mut parts = Vec::new();
        while let Some(part) = streamed.next().await {
            parts.push(part.unwrap());
        }
        assert_eq!(parts.len(), 3);
        assert!(matches!(
            &parts[0],
            StreamedMessagePart::Content(ContentPart::Think { think, .. }) if think == "think"
        ));
        assert!(matches!(
            &parts[1],
            StreamedMessagePart::Content(ContentPart::Text { text }) if text == "hello"
        ));
        assert!(matches!(
            &parts[2],
            StreamedMessagePart::ToolCall(call)
                if call.id == "call-1" && call.arguments.as_deref() == Some("{\"x\":1}")
        ));
        assert_eq!(streamed.id(), Some("chat-1"));
        assert_eq!(streamed.usage().unwrap().input_other, 8);
        assert_eq!(streamed.finish_reason(), Some(FinishReason::Completed));
        assert_eq!(streamed.raw_finish_reason(), Some("stop"));
        assert_eq!(streamed.trace_id(), TraceId::Present(Some("trace-1")));

        let response = OpenAiLegacyCompletion {
            id: "complete-1".to_owned(),
            choices: vec![OpenAiLegacyChoice {
                finish_reason: Some("length".to_owned()),
                message: Some(OpenAiLegacyMessagePayload {
                    fields: Map::from_iter([(
                        "reasoning".to_owned(),
                        Value::String("r".to_owned()),
                    )]),
                    content: Some("done".to_owned()),
                    tool_calls: Vec::new(),
                }),
                delta: None,
            }],
            ..OpenAiLegacyCompletion::default()
        };
        let mut non_stream =
            OpenAiLegacyStreamedMessage::from_completion(response, None, None, None);
        let mut count = 0;
        while non_stream.next().await.transpose().unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 2);
        assert_eq!(non_stream.finish_reason(), Some(FinishReason::Truncated));
    }
}
