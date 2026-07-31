use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use tokio_util::sync::CancellationToken;

use crate::kosong::contract::capability::ModelCapability;
use crate::kosong::contract::errors::{
    ApiStatusData, ChatProviderError, is_context_overflow_error_code,
};
use crate::kosong::contract::message::{
    ContentPart, Message, Role, StreamIndex, StreamedMessagePart, ToolCall, ToolCallPart,
    ToolCallPartType, ToolCallType, extract_text, is_tool_declaration_only_message,
};
use crate::kosong::contract::provider::{
    ChatProvider, FinishReason, GenerateOptions, ProviderError, ProviderRequestAuth,
    ResponseFormat, StreamedMessage, ThinkingEffort, ToolCallIdPolicy, TraceId,
};
use crate::kosong::contract::tool::Tool;
use crate::kosong::contract::usage::TokenUsage;
use crate::kosong::provider::bases::openai::openai_common::{
    NormalizedFinishReason, OPENAI_REASONING_CAPABILITY, OPENAI_VISION_TOOL_CAPABILITY,
    OPENAI_VISION_TOOL_PREFIXES, TOOL_RESULT_MEDIA_PLACEHOLDER, TOOL_RESULT_MEDIA_PROMPT,
    ToolMessageConversion, has_model_prefix, is_media_part, is_openai_reasoning_model,
};
use crate::kosong::provider::bases::openai::openai_responses_transport::{
    OpenAiResponsesClient, OpenAiResponsesHttpResponse, ReqwestOpenAiResponsesClient,
};
use crate::kosong::provider::bases::request_auth::{
    merge_request_headers, require_provider_api_key,
};
use crate::kosong::provider::bases::tool_call_id::{
    ToolCallIdError, normalize_tool_call_ids_for_provider, sanitize_openai_responses_call_id,
};

pub type OpenAiResponsesGenerationKwargs = Map<String, Value>;

pub static OPENAI_RESPONSES_TOOL_CALL_ID_POLICY: LazyLock<ToolCallIdPolicy> = LazyLock::new(|| {
    ToolCallIdPolicy::new(
        Arc::new(|id| sanitize_openai_responses_call_id(id, Some(64))),
        Some(64),
    )
});

// Original: openai-responses.ts, normalizeResponsesFinishReason()
pub fn normalize_responses_finish_reason(
    status: Option<&str>,
    incomplete_reason: Option<&str>,
) -> NormalizedFinishReason {
    match status {
        None => NormalizedFinishReason {
            finish_reason: None,
            raw_finish_reason: None,
        },
        Some("completed") => NormalizedFinishReason {
            finish_reason: Some(FinishReason::Completed),
            raw_finish_reason: Some("completed".to_owned()),
        },
        Some("incomplete") => {
            let (finish_reason, raw) = match incomplete_reason {
                Some("max_output_tokens") => (FinishReason::Truncated, "max_output_tokens"),
                Some("content_filter") => (FinishReason::Filtered, "content_filter"),
                Some(reason) => (FinishReason::Other, reason),
                None => (FinishReason::Other, "incomplete"),
            };
            NormalizedFinishReason {
                finish_reason: Some(finish_reason),
                raw_finish_reason: Some(raw.to_owned()),
            }
        }
        Some("failed") => NormalizedFinishReason {
            finish_reason: Some(FinishReason::Other),
            raw_finish_reason: Some("failed".to_owned()),
        },
        Some(_) => NormalizedFinishReason {
            finish_reason: None,
            raw_finish_reason: None,
        },
    }
}

pub fn response_format_to_responses_text(format: &ResponseFormat) -> Map<String, Value> {
    let format = match format {
        ResponseFormat::JsonObject => serde_json::json!({"type":"json_object"}),
        ResponseFormat::JsonSchema { json_schema } => {
            let mut value = Map::from_iter([
                ("type".to_owned(), Value::String("json_schema".to_owned())),
                ("name".to_owned(), Value::String(json_schema.name.clone())),
                (
                    "schema".to_owned(),
                    Value::Object(json_schema.schema.clone()),
                ),
            ]);
            if let Some(strict) = json_schema.strict {
                value.insert("strict".to_owned(), Value::Bool(strict));
            }
            if let Some(description) = json_schema.description.as_ref() {
                value.insert("description".to_owned(), Value::String(description.clone()));
            }
            Value::Object(value)
        }
    };
    Map::from_iter([("format".to_owned(), format)])
}

pub const OMITTED_AUDIO_PLACEHOLDER: &str = "(audio omitted: unsupported audio format)";
pub const OMITTED_VIDEO_PLACEHOLDER: &str = "(video omitted: not supported by this provider)";

pub fn map_audio_url_to_input_item(url: &str) -> Option<Value> {
    if let Some(value) = url.strip_prefix("data:audio/") {
        let mut comma_parts = value.split(',');
        let header = comma_parts.next()?;
        let data = comma_parts.next()?;
        let subtype = header
            .split(';')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let extension = match subtype.as_str() {
            "mp3" | "mpeg" => "mp3",
            "wav" => "wav",
            _ => return None,
        };
        return Some(serde_json::json!({
            "type":"input_file","file_data":data,"filename":format!("inline.{extension}")
        }));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(serde_json::json!({"type":"input_file","file_url":url}))
    } else {
        None
    }
}

pub fn content_parts_to_input_items(parts: &[ContentPart]) -> Vec<Value> {
    let mut items = Vec::new();
    for part in parts {
        match part {
            ContentPart::Text { text } if !text.is_empty() => {
                items.push(serde_json::json!({"type":"input_text","text":text}));
            }
            ContentPart::ImageUrl { image_url } => items.push(serde_json::json!({
                "type":"input_image","detail":"auto","image_url":image_url.url
            })),
            ContentPart::AudioUrl { audio_url } => {
                items.push(map_audio_url_to_input_item(&audio_url.url).unwrap_or_else(
                    || serde_json::json!({"type":"input_text","text":OMITTED_AUDIO_PLACEHOLDER}),
                ))
            }
            ContentPart::VideoUrl { .. } => items
                .push(serde_json::json!({"type":"input_text","text":OMITTED_VIDEO_PLACEHOLDER})),
            ContentPart::Text { .. } | ContentPart::Think { .. } => {}
        }
    }
    items
}

pub fn content_parts_to_output_items(parts: &[ContentPart]) -> Vec<Value> {
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } if !text.is_empty() => Some(serde_json::json!({
                "type":"output_text","text":text,"annotations":[]
            })),
            _ => None,
        })
        .collect()
}

pub fn message_content_to_function_output_items(parts: &[ContentPart]) -> Vec<Value> {
    let mut items = content_parts_to_input_items(parts);
    for item in &mut items {
        if item.get("type").and_then(Value::as_str) == Some("input_image")
            && let Some(item) = item.as_object_mut()
        {
            item.remove("detail");
        }
    }
    items
}

static DEVELOPER_ROLE_MODELS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "gpt-4.1",
        "gpt-4.1-mini",
        "gpt-4.1-nano",
        "gpt-5-codex",
        "o1",
        "o1-mini",
        "o1-pro",
        "o3",
        "o3-mini",
        "o3-pro",
        "o4-mini",
    ])
});

pub fn uses_openai_responses_developer_role(model_name: &str) -> bool {
    let normalized = model_name.to_ascii_lowercase();
    DEVELOPER_ROLE_MODELS.contains(normalized.as_str())
        || DEVELOPER_ROLE_MODELS
            .iter()
            .any(|model| normalized.starts_with(&format!("{model}-")))
}

// Original: openai-responses.ts, convertMessage()
pub fn convert_message(
    message: &Message,
    model_name: &str,
    tool_message_conversion: ToolMessageConversion,
) -> Vec<Value> {
    let role = if message.role == Role::System && uses_openai_responses_developer_role(model_name) {
        "developer"
    } else {
        message.role.as_str()
    };
    if message.role == Role::Tool {
        let output = match tool_message_conversion {
            ToolMessageConversion::ExtractText => {
                let text = extract_text(message, "");
                if text.is_empty() && message.content.iter().any(is_media_part) {
                    Value::String(TOOL_RESULT_MEDIA_PLACEHOLDER.to_owned())
                } else {
                    Value::String(text)
                }
            }
            ToolMessageConversion::Parts => {
                Value::Array(message_content_to_function_output_items(&message.content))
            }
        };
        return vec![serde_json::json!({
            "call_id":message.tool_call_id.as_deref().unwrap_or_default(),
            "output":output,"type":"function_call_output"
        })];
    }

    let mut result = Vec::new();
    let mut pending = Vec::new();
    let flush = |pending: &mut Vec<ContentPart>, result: &mut Vec<Value>| {
        if pending.is_empty() {
            return;
        }
        let content = if message.role == Role::Assistant {
            content_parts_to_output_items(pending)
        } else {
            content_parts_to_input_items(pending)
        };
        result.push(serde_json::json!({"content":content,"role":role,"type":"message"}));
        pending.clear();
    };
    let mut index = 0;
    while index < message.content.len() {
        match &message.content[index] {
            ContentPart::Think { think, encrypted } => {
                flush(&mut pending, &mut result);
                let encrypted_value = encrypted.clone();
                let mut summaries = vec![serde_json::json!({"type":"summary_text","text":think})];
                index += 1;
                while let Some(ContentPart::Think { think, encrypted }) = message.content.get(index)
                {
                    if *encrypted != encrypted_value {
                        break;
                    }
                    summaries.push(serde_json::json!({"type":"summary_text","text":think}));
                    index += 1;
                }
                let mut reasoning = Map::from_iter([
                    ("summary".to_owned(), Value::Array(summaries)),
                    ("type".to_owned(), Value::String("reasoning".to_owned())),
                ]);
                if let Some(encrypted) = encrypted_value {
                    reasoning.insert("encrypted_content".to_owned(), Value::String(encrypted));
                }
                result.push(Value::Object(reasoning));
            }
            part => {
                pending.push(part.clone());
                index += 1;
            }
        }
    }
    flush(&mut pending, &mut result);
    result.extend(message.tool_calls.iter().map(|call| {
        serde_json::json!({
            "arguments":call.arguments.as_deref().unwrap_or("{}"),
            "call_id":call.id,"name":call.name,"type":"function_call"
        })
    }));
    result
}

pub fn convert_tool(tool: &Tool) -> Value {
    serde_json::json!({
        "type":"function","name":tool.name,"description":tool.description,
        "parameters":tool.parameters,"strict":false
    })
}

pub fn convert_history_messages(
    history: &[Message],
    model_name: &str,
    conversion: ToolMessageConversion,
) -> Vec<Value> {
    let mut input = Vec::new();
    let mut pending_media = Vec::new();
    let flush_media = |input: &mut Vec<Value>, pending: &mut Vec<Value>| {
        if pending.is_empty() {
            return;
        }
        let mut content =
            vec![serde_json::json!({"type":"input_text","text":TOOL_RESULT_MEDIA_PROMPT})];
        content.append(pending);
        input.push(serde_json::json!({"type":"message","role":"user","content":content}));
    };
    for message in history {
        if is_tool_declaration_only_message(message) {
            continue;
        }
        if message.role != Role::Tool {
            flush_media(&mut input, &mut pending_media);
        }
        input.extend(convert_message(message, model_name, conversion));
        if message.role == Role::Tool && conversion == ToolMessageConversion::ExtractText {
            pending_media.extend(message_content_to_function_output_items(
                &message
                    .content
                    .iter()
                    .filter(|part| is_media_part(part))
                    .cloned()
                    .collect::<Vec<_>>(),
            ));
        }
    }
    flush_media(&mut input, &mut pending_media);
    input
}

// Original: openai-responses.ts, OpenAIResponsesChatProvider.generate()
// request construction before client.responses.create().
#[allow(clippy::too_many_arguments)]
pub fn build_openai_responses_request(
    model: &str,
    stream: bool,
    generation_kwargs: &OpenAiResponsesGenerationKwargs,
    default_thinking_effort: Option<&ThinkingEffort>,
    tool_message_conversion: ToolMessageConversion,
    system_prompt: &str,
    tools: &[Tool],
    history: &[Message],
    options: Option<&GenerateOptions>,
) -> Result<Map<String, Value>, ToolCallIdError> {
    let normalized_history = normalize_tool_call_ids_for_provider(
        history.to_vec(),
        &OPENAI_RESPONSES_TOOL_CALL_ID_POLICY,
    )?;
    let input = convert_history_messages(&normalized_history, model, tool_message_conversion);
    let mut kwargs = generation_kwargs.clone();

    if let Some(cache_key) = options.and_then(|options| options.cache_key.as_ref()) {
        kwargs.insert(
            "prompt_cache_key".to_owned(),
            Value::String(cache_key.clone()),
        );
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

    let thinking_effort = options
        .and_then(|options| options.thinking.as_ref())
        .map(|thinking| &thinking.effort)
        .or(default_thinking_effort);
    if let Some(effort) = thinking_effort {
        if matches!(effort.as_str(), "off" | "on") {
            kwargs.remove("reasoning_effort");
        } else {
            kwargs.insert(
                "reasoning_effort".to_owned(),
                Value::String(effort.to_string()),
            );
        }
    }

    if let Some(mut cap) = options.and_then(|options| options.max_completion_tokens) {
        if let Some((used, maximum)) = options.and_then(|options| {
            Some((options.used_context_tokens?, options.max_context_tokens?))
                .filter(|(_, maximum)| *maximum > 0)
        }) {
            cap = cap.min(maximum.saturating_sub(used));
        }
        kwargs.insert("max_output_tokens".to_owned(), Value::from(cap.max(1)));
    }

    if let Some(reasoning_effort) = kwargs.remove("reasoning_effort") {
        kwargs.insert(
            "reasoning".to_owned(),
            serde_json::json!({"effort":reasoning_effort,"summary":"auto"}),
        );
        kwargs.insert(
            "include".to_owned(),
            serde_json::json!(["reasoning.encrypted_content"]),
        );
    }

    let mut params = Map::from_iter([
        ("model".to_owned(), Value::String(model.to_owned())),
        ("input".to_owned(), Value::Array(input)),
        (
            "tools".to_owned(),
            Value::Array(tools.iter().map(convert_tool).collect()),
        ),
        ("store".to_owned(), Value::Bool(false)),
        ("stream".to_owned(), Value::Bool(stream)),
    ]);
    params.extend(kwargs);
    if !system_prompt.is_empty() {
        params.insert(
            "instructions".to_owned(),
            Value::String(system_prompt.to_owned()),
        );
    }
    if let Some(response_format) = options.and_then(|options| options.response_format.as_ref()) {
        let mut text = params
            .remove("text")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        text.extend(response_format_to_responses_text(response_format));
        params.insert("text".to_owned(), Value::Object(text));
    }
    Ok(params)
}

#[derive(Debug)]
enum ResponseOutputItem {
    Message {
        content: Vec<Map<String, Value>>,
    },
    FunctionCall {
        item_id: Option<String>,
        call_id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    Reasoning {
        encrypted_content: Option<String>,
        summary: Vec<Map<String, Value>>,
    },
    Other,
}

fn read_string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn read_number_field(object: &Map<String, Value>, key: &str) -> Option<i64> {
    object.get(key).and_then(Value::as_i64)
}

fn read_object_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Option<&'a Map<String, Value>> {
    object.get(key).and_then(Value::as_object)
}

fn read_object_array_field(
    object: &Map<String, Value>,
    key: &str,
) -> Option<Vec<Map<String, Value>>> {
    object.get(key).and_then(Value::as_array).map(|values| {
        values
            .iter()
            .filter_map(Value::as_object)
            .cloned()
            .collect()
    })
}

fn responses_decode_error(context: &str, detail: &str) -> ChatProviderError {
    ChatProviderError::ChatProvider {
        message: format!("OpenAI Responses decode error: {context} {detail}"),
    }
}

fn require_string_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a str, ChatProviderError> {
    read_string_field(object, key)
        .ok_or_else(|| responses_decode_error(&format!("{context}.{key}"), "must be a string."))
}

fn require_object_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a Map<String, Value>, ChatProviderError> {
    read_object_field(object, key)
        .ok_or_else(|| responses_decode_error(&format!("{context}.{key}"), "must be an object."))
}

fn read_response_output_item(
    value: &Value,
    context: &str,
) -> Result<ResponseOutputItem, ChatProviderError> {
    let item = value
        .as_object()
        .ok_or_else(|| responses_decode_error(context, "must be an object."))?;
    match require_string_field(item, "type", context)? {
        "message" => Ok(ResponseOutputItem::Message {
            content: read_object_array_field(item, "content").unwrap_or_default(),
        }),
        "function_call" => Ok(ResponseOutputItem::FunctionCall {
            item_id: read_string_field(item, "id").map(str::to_owned),
            call_id: read_string_field(item, "call_id").map(str::to_owned),
            name: read_string_field(item, "name").map(str::to_owned),
            arguments: read_string_field(item, "arguments").map(str::to_owned),
        }),
        "reasoning" => Ok(ResponseOutputItem::Reasoning {
            encrypted_content: read_string_field(item, "encrypted_content").map(str::to_owned),
            summary: read_object_array_field(item, "summary").unwrap_or_default(),
        }),
        _ => Ok(ResponseOutputItem::Other),
    }
}

fn response_stream_index(item_id: Option<&str>, output_index: Option<i64>) -> Option<StreamIndex> {
    item_id
        .map(|id| StreamIndex::String(id.to_owned()))
        .or_else(|| output_index.map(StreamIndex::Number))
}

fn format_response_stream_index(index: Option<&StreamIndex>) -> String {
    match index {
        Some(StreamIndex::String(index)) => index.clone(),
        Some(StreamIndex::Number(index)) => index.to_string(),
        None => "<unindexed>".to_owned(),
    }
}

fn require_function_call_name(name: Option<String>) -> Result<String, ChatProviderError> {
    name.ok_or_else(|| ChatProviderError::ChatProvider {
        message: "OpenAI Responses function_call item is missing a name.".to_owned(),
    })
}

fn function_call_id(call_id: Option<String>) -> String {
    call_id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

fn format_responses_error_event(code: Option<&str>, message: &str, param: Option<&str>) -> String {
    let code = code.unwrap_or("unknown");
    let param = param.map_or_else(String::new, |param| format!(" (param: {param})"));
    format!("{code}: {message}{param}")
}

static EMBEDDED_STATUS_CODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bstatus_code\s*[:=]\s*(\d{3})\b").expect("static regex must compile")
});

fn read_embedded_status_code(message: &str) -> Option<i32> {
    EMBEDDED_STATUS_CODE_RE
        .captures(message)
        .and_then(|capture| capture.get(1))
        .and_then(|value| value.as_str().parse().ok())
}

fn error_from_openai_responses_event(
    prefix: &str,
    code: Option<&str>,
    message: &str,
    param: Option<&str>,
) -> ChatProviderError {
    let message = format!(
        "{prefix}: {}",
        format_responses_error_event(code, message, param)
    );
    if is_context_overflow_error_code(code) {
        ChatProviderError::ApiContextOverflow {
            message,
            data: ApiStatusData::new(400, None, None, None),
        }
    } else if code == Some("rate_limit_exceeded")
        || read_embedded_status_code(&message) == Some(429)
    {
        ChatProviderError::ApiProviderRateLimit {
            message,
            data: ApiStatusData::new(429, None, None, None),
        }
    } else {
        ChatProviderError::ChatProvider { message }
    }
}

fn parse_nested_gateway_stream_error(
    message: &str,
) -> Option<(Option<String>, String, Option<String>)> {
    let marker = "received error while streaming:";
    let json = message.split_once(marker)?.1.trim();
    if json.is_empty() {
        return None;
    }
    let error: Value = serde_json::from_str(json).ok()?;
    let error = error.as_object()?;
    Some((
        read_string_field(error, "code").map(str::to_owned),
        read_string_field(error, "message")?.to_owned(),
        read_string_field(error, "param").map(str::to_owned),
    ))
}

fn malformed_stream_error_event(message: &str) -> ChatProviderError {
    if let Some((code, message, param)) = parse_nested_gateway_stream_error(message) {
        return error_from_openai_responses_event(
            "OpenAI Responses malformed stream error",
            code.as_deref(),
            &message,
            param.as_deref(),
        );
    }
    error_from_openai_responses_event(
        "OpenAI Responses malformed stream error",
        None,
        message,
        None,
    )
}

fn read_failed_response_error(response: &Map<String, Value>) -> Option<(String, String)> {
    let error = read_object_field(response, "error")?;
    Some((
        read_string_field(error, "code")
            .unwrap_or("unknown")
            .to_owned(),
        read_string_field(error, "message")
            .unwrap_or("no message")
            .to_owned(),
    ))
}

fn format_failed_response(response: &Map<String, Value>) -> String {
    if let Some((code, message)) = read_failed_response_error(response) {
        return format_responses_error_event(Some(&code), &message, None);
    }
    read_object_field(response, "incomplete_details")
        .and_then(|details| read_string_field(details, "reason"))
        .map_or_else(
            || "Unknown error (no error details in response)".to_owned(),
            |reason| format!("incomplete: {reason}"),
        )
}

enum OpenAiResponsesEvent {
    Response(Value),
    Chunk(Value),
}

// Original: openai-responses.ts, OpenAIResponsesStreamedMessage
pub struct OpenAiResponsesStreamedMessage {
    source: Pin<Box<dyn Stream<Item = Result<OpenAiResponsesEvent, ProviderError>> + Send>>,
    pending: VecDeque<StreamedMessagePart>,
    function_arguments: HashMap<StreamIndex, String>,
    unindexed_function_arguments: Option<String>,
    signal: Option<CancellationToken>,
    abort_emitted: bool,
    terminated: bool,
    id: Option<String>,
    usage: Option<TokenUsage>,
    finish_reason: Option<FinishReason>,
    raw_finish_reason: Option<String>,
}

impl OpenAiResponsesStreamedMessage {
    pub fn from_response(response: Value, signal: Option<CancellationToken>) -> Self {
        Self::new(
            futures_util::stream::iter([Ok(OpenAiResponsesEvent::Response(response))]),
            signal,
        )
    }

    pub fn from_stream<S>(source: S, signal: Option<CancellationToken>) -> Self
    where
        S: Stream<Item = Result<Value, ProviderError>> + Send + 'static,
    {
        Self::new(
            source.map(|item| item.map(OpenAiResponsesEvent::Chunk)),
            signal,
        )
    }

    fn new<S>(source: S, signal: Option<CancellationToken>) -> Self
    where
        S: Stream<Item = Result<OpenAiResponsesEvent, ProviderError>> + Send + 'static,
    {
        Self {
            source: Box::pin(source),
            pending: VecDeque::new(),
            function_arguments: HashMap::new(),
            unindexed_function_arguments: None,
            signal,
            abort_emitted: false,
            terminated: false,
            id: None,
            usage: None,
            finish_reason: None,
            raw_finish_reason: None,
        }
    }

    fn capture_finish_reason(&mut self, response: &Map<String, Value>) {
        let incomplete_reason = read_object_field(response, "incomplete_details")
            .and_then(|details| read_string_field(details, "reason"));
        let normalized = normalize_responses_finish_reason(
            read_string_field(response, "status"),
            incomplete_reason,
        );
        self.finish_reason = normalized.finish_reason;
        self.raw_finish_reason = normalized.raw_finish_reason;
    }

    fn extract_usage(&mut self, usage: &Map<String, Value>) {
        let input = usage
            .get("input_tokens")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let cached = read_object_field(usage, "input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        self.usage = Some(TokenUsage {
            input_other: input - cached,
            output,
            input_cache_read: cached,
            input_cache_creation: 0.0,
        });
    }

    fn push_reasoning(&mut self, text: String, encrypted: Option<String>) {
        self.pending
            .push_back(StreamedMessagePart::Content(ContentPart::Think {
                think: text,
                encrypted,
            }));
    }

    fn process_non_stream_response(&mut self, response: Value) -> Result<(), ChatProviderError> {
        let response = response
            .as_object()
            .ok_or_else(|| responses_decode_error("response", "must be an object."))?;
        self.id = read_string_field(response, "id").map(str::to_owned);
        if let Some(usage) = read_object_field(response, "usage") {
            self.extract_usage(usage);
        }
        self.capture_finish_reason(response);
        let Some(output) = response.get("output").and_then(Value::as_array) else {
            return Ok(());
        };
        for item in output {
            match read_response_output_item(item, "response.output item")? {
                ResponseOutputItem::Message { content } => {
                    for item in content {
                        if read_string_field(&item, "type") == Some("output_text")
                            && let Some(text) = read_string_field(&item, "text")
                        {
                            self.pending.push_back(StreamedMessagePart::Content(
                                ContentPart::Text {
                                    text: text.to_owned(),
                                },
                            ));
                        }
                    }
                }
                ResponseOutputItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => self
                    .pending
                    .push_back(StreamedMessagePart::ToolCall(ToolCall {
                        call_type: ToolCallType::Function,
                        id: function_call_id(call_id),
                        name: require_function_call_name(name)?,
                        arguments,
                        extras: None,
                        stream_index: None,
                    })),
                ResponseOutputItem::Reasoning {
                    encrypted_content,
                    summary,
                } => {
                    let mut found = false;
                    for item in summary {
                        if let Some(text) = read_string_field(&item, "text") {
                            found = true;
                            self.push_reasoning(text.to_owned(), encrypted_content.clone());
                        }
                    }
                    if !found {
                        self.push_reasoning(String::new(), encrypted_content);
                    }
                }
                ResponseOutputItem::Other => {}
            }
        }
        Ok(())
    }

    fn arguments_mut(&mut self, index: Option<&StreamIndex>) -> Option<&mut String> {
        match index {
            Some(index) => self.function_arguments.get_mut(index),
            None => self.unindexed_function_arguments.as_mut(),
        }
    }

    fn set_arguments(&mut self, index: Option<StreamIndex>, arguments: String) {
        match index {
            Some(index) => {
                self.function_arguments.insert(index, arguments);
            }
            None => self.unindexed_function_arguments = Some(arguments),
        }
    }

    fn append_arguments(
        &mut self,
        index: Option<&StreamIndex>,
        part: &str,
        event_type: &str,
    ) -> Result<(), ChatProviderError> {
        let formatted = format_response_stream_index(index);
        let arguments = self.arguments_mut(index).ok_or_else(|| {
            responses_decode_error(
                event_type,
                &format!("received function-call arguments for unknown stream index {formatted}."),
            )
        })?;
        arguments.push_str(part);
        Ok(())
    }

    fn push_final_arguments_suffix(
        &mut self,
        index: Option<StreamIndex>,
        final_arguments: String,
        event_type: &str,
    ) -> Result<(), ChatProviderError> {
        let formatted = format_response_stream_index(index.as_ref());
        let accumulated = self.arguments_mut(index.as_ref()).ok_or_else(|| {
            responses_decode_error(
                event_type,
                &format!(
                    "received final function-call arguments for unknown stream index {formatted}."
                ),
            )
        })?;
        if *accumulated == final_arguments {
            return Ok(());
        }
        let Some(suffix) = final_arguments.strip_prefix(accumulated.as_str()) else {
            return Err(ChatProviderError::ChatProvider {
                message: format!(
                    "OpenAI Responses final function-call arguments for stream index {formatted} do not match the streamed argument deltas."
                ),
            });
        };
        let suffix = suffix.to_owned();
        accumulated.clone_from(&final_arguments);
        if !suffix.is_empty() {
            self.pending
                .push_back(StreamedMessagePart::ToolCallPart(ToolCallPart {
                    part_type: ToolCallPartType::ToolCallPart,
                    arguments_part: Some(suffix),
                    index,
                }));
        }
        Ok(())
    }

    fn process_stream_event(&mut self, chunk: Value) -> Result<(), ChatProviderError> {
        let chunk = chunk
            .as_object()
            .ok_or_else(|| responses_decode_error("stream event", "must be an object."))?;
        let event_type = match read_string_field(chunk, "type") {
            Some(event_type) => event_type,
            None if !chunk.contains_key("type") => {
                if let Some(message) = read_string_field(chunk, "message") {
                    return Err(malformed_stream_error_event(message));
                }
                return Err(responses_decode_error(
                    "stream event.type",
                    "must be a string.",
                ));
            }
            None => {
                return Err(responses_decode_error(
                    "stream event.type",
                    "must be a string.",
                ));
            }
        };
        match event_type {
            "response.output_text.delta" => {
                let text = require_string_field(chunk, "delta", event_type)?;
                self.pending
                    .push_back(StreamedMessagePart::Content(ContentPart::Text {
                        text: text.to_owned(),
                    }));
            }
            "response.created" | "response.in_progress" => {
                let response = require_object_field(chunk, "response", event_type)?;
                if let Some(id) = read_string_field(response, "id") {
                    self.id = Some(id.to_owned());
                }
            }
            "response.output_item.added" => {
                let item = read_response_output_item(
                    chunk.get("item").unwrap_or(&Value::Null),
                    &format!("{event_type}.item"),
                )?;
                if let ResponseOutputItem::FunctionCall {
                    item_id,
                    call_id,
                    name,
                    arguments,
                } = item
                {
                    let index = response_stream_index(
                        item_id.as_deref(),
                        read_number_field(chunk, "output_index"),
                    );
                    self.set_arguments(index.clone(), arguments.clone().unwrap_or_default());
                    self.pending
                        .push_back(StreamedMessagePart::ToolCall(ToolCall {
                            call_type: ToolCallType::Function,
                            id: function_call_id(call_id),
                            name: require_function_call_name(name)?,
                            arguments,
                            extras: None,
                            stream_index: index,
                        }));
                }
            }
            "response.output_item.done" => {
                let item = read_response_output_item(
                    chunk.get("item").unwrap_or(&Value::Null),
                    &format!("{event_type}.item"),
                )?;
                match item {
                    ResponseOutputItem::Reasoning {
                        encrypted_content, ..
                    } => self.push_reasoning(String::new(), encrypted_content),
                    ResponseOutputItem::FunctionCall {
                        item_id,
                        arguments: Some(arguments),
                        ..
                    } => {
                        let index = response_stream_index(
                            item_id.as_deref(),
                            read_number_field(chunk, "output_index"),
                        );
                        self.push_final_arguments_suffix(index, arguments, event_type)?;
                    }
                    _ => {}
                }
            }
            "response.function_call_arguments.delta" => {
                let index = response_stream_index(
                    read_string_field(chunk, "item_id"),
                    read_number_field(chunk, "output_index"),
                );
                let part = require_string_field(chunk, "delta", event_type)?.to_owned();
                self.append_arguments(index.as_ref(), &part, event_type)?;
                self.pending
                    .push_back(StreamedMessagePart::ToolCallPart(ToolCallPart {
                        part_type: ToolCallPartType::ToolCallPart,
                        arguments_part: Some(part),
                        index,
                    }));
            }
            "response.function_call_arguments.done" => {
                let arguments = require_string_field(chunk, "arguments", event_type)?.to_owned();
                let index = response_stream_index(
                    read_string_field(chunk, "item_id"),
                    read_number_field(chunk, "output_index"),
                );
                self.push_final_arguments_suffix(index, arguments, event_type)?;
            }
            "response.reasoning_summary_part.added" => {
                self.push_reasoning(String::new(), None);
            }
            "response.reasoning_summary_text.delta" => {
                let text = require_string_field(chunk, "delta", event_type)?.to_owned();
                self.push_reasoning(text, None);
            }
            "response.completed" | "response.incomplete" => {
                let response = require_object_field(chunk, "response", event_type)?;
                if let Some(id) = read_string_field(response, "id") {
                    self.id = Some(id.to_owned());
                }
                if let Some(usage) = read_object_field(response, "usage") {
                    self.extract_usage(usage);
                }
                self.capture_finish_reason(response);
            }
            "error" => {
                let message = require_string_field(chunk, "message", event_type)?;
                return Err(error_from_openai_responses_event(
                    "OpenAI Responses stream error",
                    read_string_field(chunk, "code"),
                    message,
                    read_string_field(chunk, "param"),
                ));
            }
            "response.failed" => {
                let response = require_object_field(chunk, "response", event_type)?;
                if let Some((code, message)) = read_failed_response_error(response) {
                    return Err(error_from_openai_responses_event(
                        "OpenAI Responses response.failed",
                        Some(&code),
                        &message,
                        None,
                    ));
                }
                return Err(ChatProviderError::ChatProvider {
                    message: format!(
                        "OpenAI Responses response.failed: {}",
                        format_failed_response(response)
                    ),
                });
            }
            _ => {}
        }
        Ok(())
    }
}

impl Stream for OpenAiResponsesStreamedMessage {
    type Item = Result<StreamedMessagePart, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.terminated {
            return Poll::Ready(None);
        }
        if self
            .signal
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            if self.abort_emitted {
                return Poll::Ready(None);
            }
            self.abort_emitted = true;
            self.pending.clear();
            return Poll::Ready(Some(Err(Box::new(ChatProviderError::Abort))));
        }
        loop {
            if let Some(part) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(part)));
            }
            let event = match self.source.as_mut().poll_next(context) {
                Poll::Ready(Some(event)) => event,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            };
            let result = match event {
                Ok(OpenAiResponsesEvent::Response(response)) => {
                    self.process_non_stream_response(response)
                }
                Ok(OpenAiResponsesEvent::Chunk(chunk)) => self.process_stream_event(chunk),
                Err(error) => {
                    self.terminated = true;
                    return Poll::Ready(Some(Err(error)));
                }
            };
            if let Err(error) = result {
                self.terminated = true;
                return Poll::Ready(Some(Err(Box::new(error))));
            }
        }
    }
}

impl StreamedMessage for OpenAiResponsesStreamedMessage {
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
        TraceId::Absent
    }
}

pub struct OpenAiResponsesOptions {
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_output_tokens: Option<u64>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub default_headers: Option<IndexMap<String, String>>,
    pub tool_message_conversion: Option<ToolMessageConversion>,
    pub http_client: Option<reqwest::Client>,
    pub client_factory: Option<OpenAiResponsesClientFactory>,
}

impl OpenAiResponsesOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            api_key: None,
            base_url: None,
            max_output_tokens: None,
            thinking_effort: None,
            default_headers: None,
            tool_message_conversion: None,
            http_client: None,
            client_factory: None,
        }
    }
}

pub type OpenAiResponsesClientFactory = Arc<
    dyn Fn(ProviderRequestAuth) -> Result<Arc<dyn OpenAiResponsesClient>, ProviderError>
        + Send
        + Sync,
>;

// Original: openai-responses.ts, OpenAIResponsesChatProvider
//
// Rust adaptation: the SDK client boundary is represented by
// OpenAiResponsesClient; the default implementation remains reqwest-backed.
pub struct OpenAiResponsesChatProvider {
    model: String,
    stream: bool,
    api_key: Option<String>,
    base_url: String,
    default_headers: Option<IndexMap<String, String>>,
    thinking_effort: Option<ThinkingEffort>,
    generation_kwargs: OpenAiResponsesGenerationKwargs,
    tool_message_conversion: ToolMessageConversion,
    http_client: reqwest::Client,
    cached_client: Option<Arc<dyn OpenAiResponsesClient>>,
    client_factory: Option<OpenAiResponsesClientFactory>,
}

impl OpenAiResponsesChatProvider {
    pub fn new(options: OpenAiResponsesOptions) -> Self {
        let api_key = match options.api_key {
            Some(api_key) => Some(api_key),
            None => std::env::var("OPENAI_API_KEY").ok(),
        }
        .filter(|api_key| !api_key.is_empty());
        let generation_kwargs = options.max_output_tokens.map_or_else(Map::new, |tokens| {
            Map::from_iter([("max_output_tokens".to_owned(), Value::from(tokens))])
        });
        let base_url = options
            .base_url
            .unwrap_or_else(|| "https://api.openai.com/v1".to_owned());
        let http_client = options.http_client.unwrap_or_default();
        let cached_client = api_key.as_ref().map(|api_key| {
            Arc::new(ReqwestOpenAiResponsesClient::new(
                http_client.clone(),
                base_url.clone(),
                api_key.clone(),
                options.default_headers.clone(),
            )) as Arc<dyn OpenAiResponsesClient>
        });
        Self {
            model: options.model,
            stream: true,
            api_key,
            base_url,
            default_headers: options.default_headers,
            thinking_effort: options.thinking_effort,
            generation_kwargs,
            tool_message_conversion: options
                .tool_message_conversion
                .unwrap_or(ToolMessageConversion::Parts),
            http_client,
            cached_client,
            client_factory: options.client_factory,
        }
    }

    // Original: OpenAIResponsesChatProvider._createClient().
    fn create_client(
        &self,
        auth: Option<&ProviderRequestAuth>,
    ) -> Result<Arc<dyn OpenAiResponsesClient>, ProviderError> {
        if let Some(factory) = self.client_factory.as_ref() {
            return factory(auth.cloned().unwrap_or_default());
        }
        if auth.is_none()
            && let Some(client) = self.cached_client.as_ref()
        {
            return Ok(Arc::clone(client));
        }
        let api_key =
            require_provider_api_key("OpenAIResponsesChatProvider", auth, self.api_key.as_deref())?;
        let headers = merge_request_headers(
            self.default_headers.as_ref(),
            auth.and_then(|auth| auth.headers.as_ref()),
        );
        Ok(Arc::new(ReqwestOpenAiResponsesClient::new(
            self.http_client.clone(),
            self.base_url.clone(),
            api_key,
            headers,
        )))
    }
}

#[async_trait]
impl ChatProvider for OpenAiResponsesChatProvider {
    fn name(&self) -> &str {
        "openai-responses"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn thinking_effort(&self) -> Option<&ThinkingEffort> {
        self.thinking_effort.as_ref()
    }

    fn max_completion_tokens(&self) -> Option<u64> {
        self.generation_kwargs
            .get("max_output_tokens")
            .and_then(Value::as_u64)
    }

    async fn generate(
        &self,
        system_prompt: &str,
        tools: &[Tool],
        history: &[Message],
        options: Option<&GenerateOptions>,
    ) -> Result<Box<dyn StreamedMessage>, ProviderError> {
        let client = self.create_client(options.and_then(|options| options.auth.as_ref()))?;
        let params = build_openai_responses_request(
            &self.model,
            self.stream,
            &self.generation_kwargs,
            self.thinking_effort.as_ref(),
            self.tool_message_conversion,
            system_prompt,
            tools,
            history,
            options,
        )?;
        if let Some(callback) = options.and_then(|options| options.on_request_sent.as_ref()) {
            callback();
        }
        let response = client
            .create(
                params,
                self.stream,
                options.and_then(|options| options.signal.as_ref()),
            )
            .await?;
        let signal = options.and_then(|options| options.signal.clone());
        Ok(match response {
            OpenAiResponsesHttpResponse::Response(response) => Box::new(
                OpenAiResponsesStreamedMessage::from_response(response, signal),
            ),
            OpenAiResponsesHttpResponse::Stream(stream) => {
                Box::new(OpenAiResponsesStreamedMessage::from_stream(stream, signal))
            }
        })
    }
}

// Original: openai-responses.ts, getOpenAIResponsesModelCapability()
pub fn get_openai_responses_model_capability(model_name: &str) -> Option<&'static ModelCapability> {
    let normalized = model_name.to_ascii_lowercase();
    if is_openai_reasoning_model(&normalized) {
        Some(&OPENAI_REASONING_CAPABILITY)
    } else if has_model_prefix(&normalized, &OPENAI_VISION_TOOL_PREFIXES) {
        Some(&OPENAI_VISION_TOOL_CAPABILITY)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::kosong::contract::message::MediaUrl;

    struct StubResponsesClient;

    #[async_trait]
    impl OpenAiResponsesClient for StubResponsesClient {
        async fn create(
            &self,
            _: Map<String, Value>,
            _: bool,
            _: Option<&CancellationToken>,
        ) -> Result<OpenAiResponsesHttpResponse, ProviderError> {
            Ok(OpenAiResponsesHttpResponse::Response(
                serde_json::json!({"status": "completed", "output": []}),
            ))
        }
    }

    #[test]
    fn converts_reasoning_developer_role_and_tool_media() {
        let message = Message::new(
            Role::System,
            vec![
                ContentPart::Text {
                    text: "a".to_owned(),
                },
                ContentPart::Think {
                    think: "r1".to_owned(),
                    encrypted: Some("sig".to_owned()),
                },
                ContentPart::Think {
                    think: "r2".to_owned(),
                    encrypted: Some("sig".to_owned()),
                },
            ],
            Vec::new(),
        );
        let converted = convert_message(&message, "gpt-4.1-2025", ToolMessageConversion::Parts);
        assert_eq!(converted[0]["role"], "developer");
        assert_eq!(converted[1]["summary"].as_array().unwrap().len(), 2);
        assert_eq!(converted[1]["encrypted_content"], "sig");

        let mut tool = Message::new(
            Role::Tool,
            vec![ContentPart::AudioUrl {
                audio_url: MediaUrl {
                    url: "data:audio/wav;base64,d2F2".to_owned(),
                    id: None,
                },
            }],
            Vec::new(),
        );
        tool.tool_call_id = Some("call".to_owned());
        let history =
            convert_history_messages(&[tool], "gpt-4.1", ToolMessageConversion::ExtractText);
        assert_eq!(history[0]["output"], TOOL_RESULT_MEDIA_PLACEHOLDER);
        assert_eq!(history[1]["content"][1]["type"], "input_file");
    }

    #[test]
    fn builds_request_overlays_in_contract_order() {
        use crate::kosong::contract::provider::{SamplingOptions, ThinkingRequestOptions};

        let defaults = Map::from_iter([
            ("max_output_tokens".to_owned(), Value::from(900)),
            ("text".to_owned(), serde_json::json!({"verbosity":"low"})),
        ]);
        let options = GenerateOptions {
            cache_key: Some("cache-a".to_owned()),
            sampling: Some(SamplingOptions {
                temperature: Some(0.4),
                top_p: Some(0.8),
            }),
            thinking: Some(ThinkingRequestOptions {
                effort: ThinkingEffort::from("high"),
                keep: None,
            }),
            max_completion_tokens: Some(500),
            used_context_tokens: Some(900),
            max_context_tokens: Some(1_000),
            response_format: Some(ResponseFormat::JsonObject),
            ..GenerateOptions::default()
        };
        let request = build_openai_responses_request(
            "gpt-5",
            true,
            &defaults,
            Some(&ThinkingEffort::from("medium")),
            ToolMessageConversion::Parts,
            "be concise",
            &[],
            &[],
            Some(&options),
        )
        .unwrap();

        assert_eq!(request["max_output_tokens"], 100);
        assert_eq!(request["reasoning"]["effort"], "high");
        assert_eq!(request["include"][0], "reasoning.encrypted_content");
        assert_eq!(request["text"]["verbosity"], "low");
        assert_eq!(request["text"]["format"]["type"], "json_object");
        assert_eq!(request["instructions"], "be concise");
        assert_eq!(request["prompt_cache_key"], "cache-a");
        assert_eq!(request["tools"], serde_json::json!([]));
    }

    #[test]
    fn provider_preserves_constructor_defaults_and_capability_fallback() {
        let mut options = OpenAiResponsesOptions::new("gpt-5");
        options.api_key = Some("key".to_owned());
        options.max_output_tokens = Some(2048);
        let provider = OpenAiResponsesChatProvider::new(options);

        assert_eq!(provider.name(), "openai-responses");
        assert_eq!(provider.model_name(), "gpt-5");
        assert_eq!(provider.max_completion_tokens(), Some(2048));
        assert!(provider.stream);
        assert_eq!(provider.base_url, "https://api.openai.com/v1");
        assert!(std::ptr::eq(
            get_openai_responses_model_capability("o3").unwrap(),
            &OPENAI_REASONING_CAPABILITY
        ));
        assert!(get_openai_responses_model_capability("unknown").is_none());
    }

    #[test]
    fn client_factory_wins_without_api_key_and_receives_request_auth() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let factory_received = Arc::clone(&received);
        let mut options = OpenAiResponsesOptions::new("gpt-5");
        options.client_factory = Some(Arc::new(move |auth| {
            factory_received.lock().unwrap().push(auth);
            Ok(Arc::new(StubResponsesClient))
        }));
        let provider = OpenAiResponsesChatProvider::new(options);

        provider.create_client(None).unwrap();
        let auth = ProviderRequestAuth {
            api_key: Some("request-key".into()),
            headers: Some(IndexMap::from([("x-request".into(), "yes".into())])),
        };
        provider.create_client(Some(&auth)).unwrap();
        assert_eq!(
            *received.lock().unwrap(),
            vec![ProviderRequestAuth::default(), auth]
        );
    }

    #[test]
    fn default_client_cache_is_used_only_without_request_auth() {
        let mut options = OpenAiResponsesOptions::new("gpt-5");
        options.api_key = Some("default-key".into());
        let provider = OpenAiResponsesChatProvider::new(options);
        let first = provider.create_client(None).unwrap();
        let second = provider.create_client(None).unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        let request_auth = ProviderRequestAuth {
            api_key: Some("request-key".into()),
            headers: None,
        };
        let rebuilt = provider.create_client(Some(&request_auth)).unwrap();
        assert!(!Arc::ptr_eq(&first, &rebuilt));
    }

    #[tokio::test]
    async fn decodes_stream_state_and_strict_function_arguments() {
        use futures_util::StreamExt;

        let events = [
            serde_json::json!({
                "type":"response.output_item.added","output_index":0,
                "item":{"type":"function_call","id":"item-a","call_id":"call-a",
                    "name":"lookup","arguments":"{"}
            }),
            serde_json::json!({
                "type":"response.function_call_arguments.delta","output_index":0,
                "item_id":"item-a","delta":"\"x\":"
            }),
            serde_json::json!({
                "type":"response.function_call_arguments.done","output_index":0,
                "item_id":"item-a","arguments":"{\"x\":1}"
            }),
            serde_json::json!({
                "type":"response.output_item.done",
                "item":{"type":"reasoning","encrypted_content":"cipher","summary":[]}
            }),
            serde_json::json!({
                "type":"response.completed","response":{"id":"resp-a","status":"completed",
                    "usage":{"input_tokens":12,"output_tokens":3,
                        "input_tokens_details":{"cached_tokens":2}}}
            }),
        ];
        let source = futures_util::stream::iter(events.into_iter().map(Ok::<_, ProviderError>));
        let mut response = OpenAiResponsesStreamedMessage::from_stream(source, None);
        let mut parts = Vec::new();
        while let Some(part) = response.next().await {
            parts.push(part.unwrap());
        }

        assert!(matches!(
            &parts[0],
            StreamedMessagePart::ToolCall(ToolCall { id, name, .. })
                if id == "call-a" && name == "lookup"
        ));
        assert!(matches!(
            &parts[1],
            StreamedMessagePart::ToolCallPart(ToolCallPart { arguments_part: Some(part), .. })
                if part == "\"x\":"
        ));
        assert!(matches!(
            &parts[2],
            StreamedMessagePart::ToolCallPart(ToolCallPart { arguments_part: Some(part), .. })
                if part == "1}"
        ));
        assert!(matches!(
            &parts[3],
            StreamedMessagePart::Content(ContentPart::Think { think, encrypted: Some(value) })
                if think.is_empty() && value == "cipher"
        ));
        assert_eq!(response.id(), Some("resp-a"));
        assert_eq!(response.finish_reason(), Some(FinishReason::Completed));
        assert_eq!(response.usage().unwrap().input_other, 10.0);

        let bad_source = futures_util::stream::iter([Ok::<_, ProviderError>(
            serde_json::json!({"type":"response.output_text.delta","delta":7}),
        )]);
        let mut bad_response = OpenAiResponsesStreamedMessage::from_stream(bad_source, None);
        let error = bad_response.next().await.unwrap().unwrap_err();
        assert_eq!(
            error.to_string(),
            "OpenAI Responses decode error: response.output_text.delta.delta must be a string."
        );
        assert!(bad_response.next().await.is_none());
    }
}
