use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::kosong::contract::capability::ModelCapability;
use crate::kosong::contract::errors::{
    ChatProviderError, classify_base_api_error, normalize_api_status_error,
};
use crate::kosong::contract::message::{
    ContentPart, Message, Role, StreamedMessagePart, ToolCall, ToolCallType,
    is_tool_declaration_only_message,
};
use crate::kosong::contract::provider::{
    ChatProvider, FinishReason, GenerateOptions, ProviderError, ProviderRequestAuth,
    ResponseFormat, StreamedMessage, ThinkingEffort, TraceId,
};
use crate::kosong::contract::tool::Tool;
use crate::kosong::contract::usage::TokenUsage;
use crate::kosong::provider::bases::google_genai::google_genai_transport::{
    GoogleAdcProvider, GoogleGenAiClient, GoogleGenAiHttpResponse, ReqwestGoogleGenAiClient,
};
use crate::kosong::provider::bases::http_client::default_provider_http_client;
use crate::kosong::provider::bases::merge_user_messages::{
    ConsecutiveUserMessageMergePolicy, merge_consecutive_user_messages,
};
use crate::kosong::provider::bases::openai::openai_common::NormalizedFinishReason;
use crate::kosong::provider::bases::request_auth::require_provider_api_key;

pub type GoogleGenAiGenerationKwargs = Map<String, Value>;

// Original: google-genai.ts, normalizeGoogleGenAIFinishReason()
pub fn normalize_google_gen_ai_finish_reason(raw: Option<&Value>) -> NormalizedFinishReason {
    let raw = match raw {
        Some(Value::String(value)) => value.to_ascii_uppercase(),
        Some(Value::Number(value)) => value.to_string().to_ascii_uppercase(),
        Some(Value::Bool(value)) => value.to_string().to_ascii_uppercase(),
        _ => String::new(),
    };
    if raw.is_empty() || raw == "FINISH_REASON_UNSPECIFIED" {
        return NormalizedFinishReason {
            finish_reason: None,
            raw_finish_reason: None,
        };
    }
    let finish_reason = match raw.as_str() {
        "STOP" => FinishReason::Completed,
        "MAX_TOKENS" => FinishReason::Truncated,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => {
            FinishReason::Filtered
        }
        _ => FinishReason::Other,
    };
    NormalizedFinishReason {
        finish_reason: Some(finish_reason),
        raw_finish_reason: Some(raw),
    }
}

// Original: google-genai.ts, convertGoogleGenAIError()
pub fn convert_google_gen_ai_error(error: reqwest::Error) -> ChatProviderError {
    if error.is_timeout() {
        ChatProviderError::timeout(error.to_string())
    } else if error.is_connect() || error.is_request() || error.is_body() {
        ChatProviderError::connection(error.to_string())
    } else if error.is_decode() {
        ChatProviderError::ChatProvider {
            message: format!("GoogleGenAI error: {error}"),
        }
    } else {
        classify_base_api_error(&error.to_string())
    }
}

pub fn convert_google_gen_ai_status_error(status_code: u16, message: &str) -> ChatProviderError {
    normalize_api_status_error(i32::from(status_code), message, None, None, None)
}

// Original: google-genai.ts, toolToGoogleGenAI()
pub fn tool_to_google_gen_ai(tool: &Tool) -> Value {
    serde_json::json!({
        "functionDeclarations":[{
            "name":tool.name,
            "description":tool.description,
            "parametersJsonSchema":tool.parameters,
        }]
    })
}

// Original: google-genai.ts, applyResponseFormat()
pub fn apply_response_format(config: &mut Map<String, Value>, format: Option<&ResponseFormat>) {
    let Some(format) = format else {
        return;
    };
    config.insert(
        "responseMimeType".to_owned(),
        Value::String("application/json".to_owned()),
    );
    config.remove("responseSchema");
    config.remove("responseJsonSchema");
    if let ResponseFormat::JsonSchema { json_schema } = format {
        config.insert(
            "responseJsonSchema".to_owned(),
            Value::Object(json_schema.schema.clone()),
        );
    }
}

static ENTROPY_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_[0-9a-f]{8}$").unwrap());
static TOOL_NAME_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+)_[^_]+$").unwrap());

// Original: google-genai.ts, toolCallIdToName()
pub fn tool_call_id_to_name(
    tool_call_id: &str,
    tool_name_by_id: &IndexMap<String, String>,
) -> String {
    if let Some(name) = tool_name_by_id.get(tool_call_id) {
        return name.clone();
    }
    let without_entropy = ENTROPY_SUFFIX.replace(tool_call_id, "");
    if let Some(name) = TOOL_NAME_SUFFIX
        .captures(&without_entropy)
        .and_then(|captures| captures.get(1))
    {
        name.as_str().to_owned()
    } else {
        without_entropy.into_owned()
    }
}

fn inferred_mime_type(url: &str, fallback: &str) -> String {
    let Ok(url) = url::Url::parse(url) else {
        return fallback.to_owned();
    };
    let pathname = url.path().to_ascii_lowercase();
    if pathname.ends_with(".png") {
        "image/png"
    } else if pathname.ends_with(".jpg") || pathname.ends_with(".jpeg") {
        "image/jpeg"
    } else if pathname.ends_with(".gif") {
        "image/gif"
    } else if pathname.ends_with(".webp") {
        "image/webp"
    } else if pathname.ends_with(".mp3") || pathname.ends_with(".mpeg") {
        "audio/mpeg"
    } else if pathname.ends_with(".wav") {
        "audio/wav"
    } else if pathname.ends_with(".ogg") {
        "audio/ogg"
    } else {
        fallback
    }
    .to_owned()
}

// Original: google-genai.ts, convertMediaUrl()
pub fn convert_media_url(url: &str, fallback_mime_type: &str) -> Value {
    if url.starts_with("data:")
        && let Some(comma) = url.find(',')
    {
        let meta = &url[..comma];
        let data = &url[comma + 1..];
        let mime_type = meta
            .find(':')
            .zip(meta.find(';'))
            .map(|(colon, semi)| &meta[colon + 1..semi])
            .unwrap_or(fallback_mime_type);
        return serde_json::json!({
            "inlineData":{"mimeType":mime_type,"data":data}
        });
    }
    serde_json::json!({
        "fileData":{
            "fileUri":url,
            "mimeType":inferred_mime_type(url, fallback_mime_type),
        }
    })
}

// Original: google-genai.ts, messageToGoogleGenAI()
pub fn message_to_google_gen_ai(message: &Message) -> Result<Value, ChatProviderError> {
    if message.role == Role::Tool {
        return Err(ChatProviderError::ChatProvider {
            message: "Tool messages must be converted via messagesToGoogleGenAIContents."
                .to_owned(),
        });
    }
    let role = if message.role == Role::Assistant {
        "model"
    } else {
        message.role.as_str()
    };
    let mut parts = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Text { text } => parts.push(serde_json::json!({"text":text})),
            ContentPart::Think { think, encrypted } => {
                let mut part = Map::from_iter([
                    ("text".to_owned(), Value::String(think.clone())),
                    ("thought".to_owned(), Value::Bool(true)),
                ]);
                if let Some(signature) = encrypted.as_ref().filter(|value| !value.is_empty()) {
                    part.insert(
                        "thoughtSignature".to_owned(),
                        Value::String(signature.clone()),
                    );
                }
                parts.push(Value::Object(part));
            }
            ContentPart::ImageUrl { image_url } => {
                parts.push(convert_media_url(&image_url.url, "image/jpeg"));
            }
            ContentPart::AudioUrl { audio_url } => {
                parts.push(convert_media_url(&audio_url.url, "audio/mpeg"));
            }
            ContentPart::VideoUrl { video_url } => {
                parts.push(convert_media_url(&video_url.url, "video/mp4"));
            }
        }
    }
    for tool_call in &message.tool_calls {
        let args = match tool_call.arguments.as_deref() {
            None | Some("") => Map::new(),
            Some(arguments) => match serde_json::from_str(arguments) {
                Ok(Value::Object(args)) => args,
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
        let mut part = Map::from_iter([(
            "functionCall".to_owned(),
            serde_json::json!({"name":tool_call.name,"args":args}),
        )]);
        if let Some(signature) = tool_call
            .extras
            .as_ref()
            .and_then(|extras| extras.get("thought_signature_b64"))
        {
            part.insert("thoughtSignature".to_owned(), signature.clone());
        }
        parts.push(Value::Object(part));
    }
    Ok(serde_json::json!({"role":role,"parts":parts}))
}

pub fn tool_message_to_function_response_parts(
    message: &Message,
    tool_name_by_id: &IndexMap<String, String>,
) -> Result<Vec<Value>, ChatProviderError> {
    if message.role != Role::Tool {
        return Err(ChatProviderError::ChatProvider {
            message: "Expected a tool message.".to_owned(),
        });
    }
    let tool_call_id =
        message
            .tool_call_id
            .as_deref()
            .ok_or_else(|| ChatProviderError::ChatProvider {
                message: "Tool response is missing `toolCallId`.".to_owned(),
            })?;
    let mut text_output = String::new();
    let mut media_parts = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Text { text } if !text.is_empty() => text_output.push_str(text),
            ContentPart::ImageUrl { image_url } => {
                media_parts.push(convert_media_url(&image_url.url, "image/jpeg"));
            }
            ContentPart::AudioUrl { audio_url } => {
                media_parts.push(convert_media_url(&audio_url.url, "audio/mpeg"));
            }
            ContentPart::VideoUrl { video_url } => {
                media_parts.push(convert_media_url(&video_url.url, "video/mp4"));
            }
            ContentPart::Text { .. } | ContentPart::Think { .. } => {}
        }
    }
    let mut parts = vec![serde_json::json!({
        "functionResponse":{
            "name":tool_call_id_to_name(tool_call_id, tool_name_by_id),
            "response":{"output":text_output},
            "parts":[],
        }
    })];
    parts.extend(media_parts);
    Ok(parts)
}

struct GoogleContentMergePolicy;

impl ConsecutiveUserMessageMergePolicy<Value> for GoogleContentMergePolicy {
    fn is_user(&self, content: &Value) -> bool {
        content.get("role").and_then(Value::as_str) == Some("user")
    }

    fn is_tool_result_only(&self, content: &Value) -> bool {
        content
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                !parts.is_empty()
                    && parts
                        .iter()
                        .all(|part| part.get("functionResponse").is_some())
            })
    }

    fn merge(&self, mut last: Value, next: Value) -> Value {
        let next = next
            .get("parts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(parts) = last.get_mut("parts").and_then(Value::as_array_mut) {
            parts.extend(next);
        }
        last
    }
}

// Original: google-genai.ts, messagesToGoogleGenAIContents()
pub fn messages_to_google_gen_ai_contents(
    messages: &[Message],
) -> Result<Vec<Value>, ChatProviderError> {
    let mut contents = Vec::new();
    let mut tool_name_by_id = IndexMap::new();
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        if is_tool_declaration_only_message(message) {
            index += 1;
            continue;
        }
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
            if !text.is_empty() {
                contents.push(serde_json::json!({
                    "role":"user","parts":[{"text":format!("<system>{text}</system>")}]
                }));
            }
            index += 1;
            continue;
        }
        if message.role == Role::Assistant && !message.tool_calls.is_empty() {
            contents.push(message_to_google_gen_ai(message)?);
            let expected_ids = message
                .tool_calls
                .iter()
                .map(|call| {
                    tool_name_by_id.insert(call.id.clone(), call.name.clone());
                    call.id.clone()
                })
                .collect::<Vec<_>>();
            let mut end = index + 1;
            while end < messages.len() && messages[end].role == Role::Tool {
                end += 1;
            }
            if end > index + 1 {
                let mut by_id = IndexMap::<String, &Message>::new();
                for tool_message in &messages[index + 1..end] {
                    let id = tool_message.tool_call_id.as_ref().ok_or_else(|| {
                        ChatProviderError::ChatProvider {
                            message: "Tool response is missing `toolCallId`.".to_owned(),
                        }
                    })?;
                    if by_id.contains_key(id) {
                        return Err(ChatProviderError::ChatProvider {
                            message: format!("Duplicate tool response for id: {id}"),
                        });
                    }
                    by_id.insert(id.clone(), tool_message);
                }
                let mut sorted = Vec::new();
                for id in expected_ids {
                    let message =
                        by_id
                            .shift_remove(&id)
                            .ok_or_else(|| ChatProviderError::ChatProvider {
                                message: format!("Missing tool responses for ids: {id}"),
                            })?;
                    sorted.push(message);
                }
                if !by_id.is_empty() {
                    let ids = by_id.keys().cloned().collect::<Vec<_>>();
                    return Err(ChatProviderError::ChatProvider {
                        message: format!(
                            "Unexpected tool responses for ids: {}",
                            serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_owned())
                        ),
                    });
                }
                let mut parts = Vec::new();
                for message in sorted {
                    parts.extend(tool_message_to_function_response_parts(
                        message,
                        &tool_name_by_id,
                    )?);
                }
                contents.push(serde_json::json!({"role":"user","parts":parts}));
                index = end;
                continue;
            }
            index += 1;
            continue;
        }
        if message.role == Role::Tool {
            let parts = tool_message_to_function_response_parts(message, &tool_name_by_id)?;
            contents.push(serde_json::json!({"role":"user","parts":parts}));
        } else {
            contents.push(message_to_google_gen_ai(message)?);
        }
        index += 1;
    }
    Ok(merge_consecutive_user_messages(
        &contents,
        &GoogleContentMergePolicy,
    ))
}

// Original: google-genai.ts, GoogleGenAIChatProvider._encodeThinking()
pub fn encode_thinking(model: &str, effort: &ThinkingEffort) -> Map<String, Value> {
    let mut config = Map::from_iter([("includeThoughts".to_owned(), Value::Bool(true))]);
    if model.contains("gemini-3") {
        match effort.as_str() {
            "off" => {
                config.insert(
                    "thinkingLevel".to_owned(),
                    Value::String("MINIMAL".to_owned()),
                );
                config.insert("includeThoughts".to_owned(), Value::Bool(false));
            }
            "low" => {
                config.insert("thinkingLevel".to_owned(), Value::String("LOW".to_owned()));
            }
            "medium" => {
                config.insert(
                    "thinkingLevel".to_owned(),
                    Value::String("MEDIUM".to_owned()),
                );
            }
            "high" | "xhigh" | "max" => {
                config.insert("thinkingLevel".to_owned(), Value::String("HIGH".to_owned()));
            }
            _ => {}
        }
    } else {
        let budget = match effort.as_str() {
            "off" => Some(0),
            "low" => Some(1_024),
            "medium" => Some(4_096),
            "high" | "xhigh" | "max" => Some(32_000),
            _ => None,
        };
        if let Some(budget) = budget {
            config.insert("thinkingBudget".to_owned(), Value::from(budget));
        }
        if effort.is_off() {
            config.insert("includeThoughts".to_owned(), Value::Bool(false));
        }
    }
    config
}

// Original: google-genai.ts, GoogleGenAIChatProvider.generate() pre-I/O path.
#[allow(clippy::too_many_arguments)]
pub fn build_google_gen_ai_request(
    model: &str,
    generation_kwargs: &GoogleGenAiGenerationKwargs,
    default_thinking_effort: Option<&ThinkingEffort>,
    system_prompt: &str,
    tools: &[Tool],
    history: &[Message],
    options: Option<&GenerateOptions>,
) -> Result<Map<String, Value>, ChatProviderError> {
    if options
        .and_then(|options| options.signal.as_ref())
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
    {
        return Err(ChatProviderError::Abort);
    }
    let contents = messages_to_google_gen_ai_contents(history)?;
    let mut kwargs = generation_kwargs.clone();
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
        kwargs.insert("topP".to_owned(), Value::from(top_p));
    }
    let thinking = options
        .and_then(|options| options.thinking.as_ref())
        .map(|thinking| &thinking.effort)
        .or(default_thinking_effort);
    if let Some(effort) = thinking {
        kwargs.insert(
            "thinkingConfig".to_owned(),
            Value::Object(encode_thinking(model, effort)),
        );
    }
    if let Some(mut cap) = options.and_then(|options| options.max_completion_tokens) {
        if let Some((used, max)) =
            options.and_then(|options| options.used_context_tokens.zip(options.max_context_tokens))
            && max > 0
        {
            cap = cap.min(max.saturating_sub(used));
        }
        kwargs.insert("maxOutputTokens".to_owned(), Value::from(cap.max(1)));
    }
    let mut config = kwargs;
    config.insert(
        "systemInstruction".to_owned(),
        Value::String(system_prompt.to_owned()),
    );
    if !tools.is_empty() {
        config.insert(
            "tools".to_owned(),
            Value::Array(tools.iter().map(tool_to_google_gen_ai).collect()),
        );
    }
    apply_response_format(
        &mut config,
        options.and_then(|options| options.response_format.as_ref()),
    );
    Ok(Map::from_iter([
        ("model".to_owned(), Value::String(model.to_owned())),
        ("contents".to_owned(), Value::Array(contents)),
        ("config".to_owned(), Value::Object(config)),
    ]))
}

pub const GEMINI_MULTIMODAL_TOOL_CAPABILITY: ModelCapability = ModelCapability {
    image_in: true,
    video_in: true,
    audio_in: true,
    thinking: false,
    tool_use: true,
    max_context_tokens: 0,
    dynamically_loaded_tools: None,
};

pub const GEMINI_THINKING_MULTIMODAL_TOOL_CAPABILITY: ModelCapability = ModelCapability {
    thinking: true,
    ..GEMINI_MULTIMODAL_TOOL_CAPABILITY
};

// Original: google-genai.ts, getGoogleGenAIModelCapability()
pub fn get_google_gen_ai_model_capability(model_name: &str) -> Option<&'static ModelCapability> {
    let normalized = model_name.to_ascii_lowercase();
    if !normalized.starts_with("gemini-")
        || ![
            "gemini-1.5-pro",
            "gemini-1.5-flash",
            "gemini-2.0-flash",
            "gemini-2.0-pro",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
        ]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return None;
    }
    if normalized.starts_with("gemini-2.5-") || normalized.contains("thinking") {
        Some(&GEMINI_THINKING_MULTIMODAL_TOOL_CAPABILITY)
    } else {
        Some(&GEMINI_MULTIMODAL_TOOL_CAPABILITY)
    }
}

enum GoogleGenAiResponseEvent {
    Response(Value),
    Chunk(Value),
}

// Original: google-genai.ts, GoogleGenAIStreamedMessage
pub struct GoogleGenAiStreamedMessage {
    source: Pin<Box<dyn Stream<Item = Result<GoogleGenAiResponseEvent, ProviderError>> + Send>>,
    pending: VecDeque<StreamedMessagePart>,
    signal: Option<CancellationToken>,
    abort_emitted: bool,
    id: Option<String>,
    usage: Option<TokenUsage>,
    finish_reason: Option<FinishReason>,
    raw_finish_reason: Option<String>,
}

impl GoogleGenAiStreamedMessage {
    pub fn from_response(response: Value, signal: Option<CancellationToken>) -> Self {
        Self::new(
            futures_util::stream::iter([Ok(GoogleGenAiResponseEvent::Response(response))]),
            signal,
        )
    }

    pub fn from_stream<S>(response: S, signal: Option<CancellationToken>) -> Self
    where
        S: Stream<Item = Result<Value, ProviderError>> + Send + 'static,
    {
        Self::new(
            response.map(|chunk| chunk.map(GoogleGenAiResponseEvent::Chunk)),
            signal,
        )
    }

    fn new<S>(source: S, signal: Option<CancellationToken>) -> Self
    where
        S: Stream<Item = Result<GoogleGenAiResponseEvent, ProviderError>> + Send + 'static,
    {
        Self {
            source: Box::pin(source),
            pending: VecDeque::new(),
            signal,
            abort_emitted: false,
            id: None,
            usage: None,
            finish_reason: None,
            raw_finish_reason: None,
        }
    }

    fn capture_finish_reason(&mut self, response: &Value) {
        let Some(first) = response
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
        else {
            return;
        };
        let raw = first
            .get("finishReason")
            .or_else(|| first.get("finish_reason"));
        if raw.is_none() {
            return;
        }
        let normalized = normalize_google_gen_ai_finish_reason(raw);
        if normalized.finish_reason.is_some() || normalized.raw_finish_reason.is_some() {
            self.finish_reason = normalized.finish_reason;
            self.raw_finish_reason = normalized.raw_finish_reason;
        }
    }

    fn extract_usage(&mut self, response: &Value) {
        let Some(usage) = response.get("usageMetadata") else {
            return;
        };
        let prompt = usage
            .get("promptTokenCount")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let cached = usage
            .get("cachedContentTokenCount")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        self.usage = Some(TokenUsage {
            input_other: (prompt - cached).max(0.0),
            output: usage
                .get("candidatesTokenCount")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            input_cache_read: cached,
            input_cache_creation: 0.0,
        });
    }

    fn extract_id(&mut self, response: &Value) {
        if let Some(id) = response.get("responseId").and_then(Value::as_str) {
            self.id = Some(id.to_owned());
        }
    }

    fn js_truthy(value: &Value) -> bool {
        match value {
            Value::Null => false,
            Value::Bool(value) => *value,
            Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
            Value::String(value) => !value.is_empty(),
            Value::Array(_) | Value::Object(_) => true,
        }
    }

    fn extract_chunk_parts(&mut self, response: &Value) {
        let candidates = response
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for candidate in candidates {
            let Some(parts) = candidate
                .get("content")
                .and_then(|content| content.get("parts"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for part in parts {
                if part.get("thought").and_then(Value::as_bool) == Some(true)
                    && let Some(text) = part.get("text").and_then(Value::as_str)
                {
                    let signature = part
                        .get("thoughtSignature")
                        .or_else(|| part.get("thought_signature"))
                        .and_then(Value::as_str)
                        .filter(|signature| !signature.is_empty())
                        .map(str::to_owned);
                    self.pending
                        .push_back(StreamedMessagePart::Content(ContentPart::Think {
                            think: text.to_owned(),
                            encrypted: signature,
                        }));
                    continue;
                }
                if let Some(text) = part
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    self.pending
                        .push_back(StreamedMessagePart::Content(ContentPart::Text {
                            text: text.to_owned(),
                        }));
                    continue;
                }
                let Some(function_call) = part
                    .get("functionCall")
                    .or_else(|| part.get("function_call"))
                else {
                    continue;
                };
                let Some(name) = function_call
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                else {
                    continue;
                };
                let id = function_call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let entropy = uuid::Uuid::new_v4()
                    .simple()
                    .to_string()
                    .chars()
                    .take(8)
                    .collect::<String>();
                let arguments = function_call.get("args").map_or_else(
                    || "{}".to_owned(),
                    |args| {
                        if Self::js_truthy(args) {
                            serde_json::to_string(args).unwrap_or_else(|_| "{}".to_owned())
                        } else {
                            "{}".to_owned()
                        }
                    },
                );
                let signature = part
                    .get("thoughtSignature")
                    .or_else(|| part.get("thought_signature"))
                    .and_then(Value::as_str)
                    .filter(|signature| !signature.is_empty());
                let extras = signature.map(|signature| {
                    Map::from_iter([(
                        "thought_signature_b64".to_owned(),
                        Value::String(signature.to_owned()),
                    )])
                });
                self.pending
                    .push_back(StreamedMessagePart::ToolCall(ToolCall {
                        call_type: ToolCallType::Function,
                        id: format!("{name}_{id}_{entropy}"),
                        name: name.to_owned(),
                        arguments: Some(arguments),
                        extras,
                        stream_index: None,
                    }));
            }
        }
    }

    fn process_response(&mut self, response: Value) {
        self.extract_usage(&response);
        self.extract_id(&response);
        self.capture_finish_reason(&response);
        self.extract_chunk_parts(&response);
    }
}

impl Stream for GoogleGenAiStreamedMessage {
    type Item = Result<StreamedMessagePart, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
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
            match self.source.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(
                    GoogleGenAiResponseEvent::Response(response)
                    | GoogleGenAiResponseEvent::Chunk(response),
                ))) => self.process_response(response),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl StreamedMessage for GoogleGenAiStreamedMessage {
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

pub struct GoogleGenAiOptions {
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub vertex_ai: Option<bool>,
    pub project: Option<String>,
    pub location: Option<String>,
    pub stream: Option<bool>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub default_headers: Option<IndexMap<String, String>>,
    pub http_client: Option<reqwest::Client>,
    pub client_factory: Option<GoogleGenAiClientFactory>,
    pub adc_provider: Option<Arc<dyn GoogleAdcProvider>>,
}

impl GoogleGenAiOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            api_key: None,
            base_url: None,
            vertex_ai: None,
            project: None,
            location: None,
            stream: None,
            thinking_effort: None,
            default_headers: None,
            http_client: None,
            client_factory: None,
            adc_provider: None,
        }
    }
}

pub type GoogleGenAiClientFactory = Arc<
    dyn Fn(ProviderRequestAuth) -> Result<Arc<dyn GoogleGenAiClient>, ProviderError> + Send + Sync,
>;

// Original: google-genai.ts, GoogleGenAIChatProvider
pub struct GoogleGenAiChatProvider {
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    vertex_ai: bool,
    project: Option<String>,
    location: Option<String>,
    stream: bool,
    thinking_effort: Option<ThinkingEffort>,
    default_headers: Option<IndexMap<String, String>>,
    generation_kwargs: GoogleGenAiGenerationKwargs,
    http_client: reqwest::Client,
    cached_client: Option<Arc<dyn GoogleGenAiClient>>,
    client_factory: Option<GoogleGenAiClientFactory>,
    adc_provider: OnceCell<Arc<dyn GoogleAdcProvider>>,
}

struct DefaultGoogleAdcProvider {
    inner: Arc<dyn gcp_auth::TokenProvider>,
}

#[async_trait]
impl GoogleAdcProvider for DefaultGoogleAdcProvider {
    async fn access_token(&self) -> Result<String, ProviderError> {
        self.inner
            .token(&["https://www.googleapis.com/auth/cloud-platform"])
            .await
            .map(|token| token.as_str().to_owned())
            .map_err(|error| {
                Box::new(ChatProviderError::ChatProvider {
                    message: format!(
                        "GoogleGenAIChatProvider: failed to resolve Vertex ADC token: {error}"
                    ),
                }) as ProviderError
            })
    }

    async fn project_id(&self) -> Result<String, ProviderError> {
        self.inner
            .project_id()
            .await
            .map(|id| id.to_string())
            .map_err(|error| {
                Box::new(ChatProviderError::ChatProvider {
                    message: format!(
                        "GoogleGenAIChatProvider: failed to resolve Vertex ADC project: {error}"
                    ),
                }) as ProviderError
            })
    }
}

impl GoogleGenAiChatProvider {
    pub fn new(options: GoogleGenAiOptions) -> Self {
        let api_key = match options.api_key {
            Some(api_key) => Some(api_key),
            None => std::env::var("GOOGLE_API_KEY").ok(),
        }
        .filter(|api_key| !api_key.is_empty());
        let vertex_ai = options.vertex_ai.unwrap_or(false);
        let base_url = options.base_url.filter(|base_url| !base_url.is_empty());
        let http_client = options
            .http_client
            .unwrap_or_else(default_provider_http_client);
        let cached_client = if vertex_ai {
            None
        } else {
            api_key.as_ref().map(|api_key| {
                Arc::new(ReqwestGoogleGenAiClient::new(
                    http_client.clone(),
                    base_url.clone(),
                    Some(api_key.clone()),
                    options.default_headers.clone(),
                    None,
                    None,
                )) as Arc<dyn GoogleGenAiClient>
            })
        };
        let adc_provider = OnceCell::new();
        if let Some(provider) = options.adc_provider {
            let _ = adc_provider.set(provider);
        }
        Self {
            model: options.model,
            api_key,
            base_url,
            vertex_ai,
            project: options.project,
            location: options.location,
            stream: options.stream.unwrap_or(true),
            thinking_effort: options.thinking_effort,
            default_headers: options.default_headers,
            generation_kwargs: Map::new(),
            http_client,
            cached_client,
            client_factory: options.client_factory,
            adc_provider,
        }
    }

    async fn vertex_identity(
        &self,
        adc: Option<&Arc<dyn GoogleAdcProvider>>,
    ) -> Result<(String, String), ProviderError> {
        let project = self
            .project
            .clone()
            .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok());
        let project = match (project, adc) {
            (Some(project), _) => Some(project),
            (None, Some(adc)) => Some(adc.project_id().await?),
            (None, None) => None,
        };
        let location = self
            .location
            .clone()
            .or_else(|| std::env::var("GOOGLE_CLOUD_LOCATION").ok());
        match (project, location) {
            (Some(project), Some(location)) if !project.is_empty() && !location.is_empty() => {
                Ok((project, location))
            }
            _ => Err(Box::new(ChatProviderError::ChatProvider {
                message: "GoogleGenAIChatProvider: Vertex AI requires project and location. Provide providerOptions.project/location or GOOGLE_CLOUD_PROJECT/GOOGLE_CLOUD_LOCATION.".to_owned(),
            })),
        }
    }

    // Original: GoogleGenAIChatProvider._createClient().
    async fn create_client(
        &self,
        auth: Option<&ProviderRequestAuth>,
    ) -> Result<Arc<dyn GoogleGenAiClient>, ProviderError> {
        if let Some(factory) = self.client_factory.as_ref() {
            return factory(auth.cloned().unwrap_or_default());
        }
        if auth.is_none()
            && let Some(client) = self.cached_client.as_ref()
        {
            return Ok(Arc::clone(client));
        }
        let (api_key, vertex, adc_provider) = if self.vertex_ai {
            let adc = if self.api_key.is_none() {
                Some(Arc::clone(
                    self.adc_provider
                        .get_or_try_init(|| async {
                            let provider = gcp_auth::provider().await.map_err(|error| {
                                Box::new(ChatProviderError::ChatProvider {
                                    message: format!("GoogleGenAIChatProvider: failed to initialize Vertex ADC: {error}"),
                                }) as ProviderError
                            })?;
                            Ok::<Arc<dyn GoogleAdcProvider>, ProviderError>(Arc::new(
                                DefaultGoogleAdcProvider { inner: provider },
                            ))
                        })
                        .await?,
                ))
            } else {
                None
            };
            (
                self.api_key.clone(),
                Some(self.vertex_identity(adc.as_ref()).await?),
                adc,
            )
        } else {
            (
                Some(require_provider_api_key(
                    "GoogleGenAIChatProvider",
                    auth,
                    self.api_key.as_deref(),
                )?),
                None,
                None,
            )
        };
        Ok(Arc::new(ReqwestGoogleGenAiClient::new(
            self.http_client.clone(),
            self.base_url.clone(),
            api_key,
            self.default_headers.clone(),
            vertex,
            adc_provider,
        )))
    }
}

#[async_trait]
impl ChatProvider for GoogleGenAiChatProvider {
    fn name(&self) -> &str {
        "google_genai"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn thinking_effort(&self) -> Option<&ThinkingEffort> {
        self.thinking_effort.as_ref()
    }

    fn max_completion_tokens(&self) -> Option<u64> {
        self.generation_kwargs
            .get("maxOutputTokens")
            .and_then(Value::as_u64)
    }

    async fn generate(
        &self,
        system_prompt: &str,
        tools: &[Tool],
        history: &[Message],
        options: Option<&GenerateOptions>,
    ) -> Result<Box<dyn StreamedMessage>, ProviderError> {
        let params = build_google_gen_ai_request(
            &self.model,
            &self.generation_kwargs,
            self.thinking_effort.as_ref(),
            system_prompt,
            tools,
            history,
            options,
        )?;
        let client = self
            .create_client(options.and_then(|options| options.auth.as_ref()))
            .await?;
        if let Some(callback) = options.and_then(|options| options.on_request_sent.as_ref()) {
            callback();
        }
        let response = client
            .generate(
                params,
                self.stream,
                options.and_then(|options| options.signal.as_ref()),
            )
            .await?;
        let signal = options.and_then(|options| options.signal.clone());
        Ok(match response {
            GoogleGenAiHttpResponse::Response(response) => {
                Box::new(GoogleGenAiStreamedMessage::from_response(response, signal))
            }
            GoogleGenAiHttpResponse::Stream(stream) => {
                Box::new(GoogleGenAiStreamedMessage::from_stream(stream, signal))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::kosong::contract::message::{ToolCall, ToolCallType};

    struct StubGoogleGenAiClient;

    struct StubGoogleAdcProvider {
        project_calls: Arc<AtomicUsize>,
        token_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl GoogleAdcProvider for StubGoogleAdcProvider {
        async fn access_token(&self) -> Result<String, ProviderError> {
            self.token_calls.fetch_add(1, Ordering::SeqCst);
            Ok("adc-token".into())
        }

        async fn project_id(&self) -> Result<String, ProviderError> {
            self.project_calls.fetch_add(1, Ordering::SeqCst);
            Ok("adc-project".into())
        }
    }

    #[async_trait]
    impl GoogleGenAiClient for StubGoogleGenAiClient {
        async fn generate(
            &self,
            _: Map<String, Value>,
            _: bool,
            _: Option<&CancellationToken>,
        ) -> Result<GoogleGenAiHttpResponse, ProviderError> {
            Ok(GoogleGenAiHttpResponse::Response(serde_json::json!({})))
        }
    }

    #[test]
    fn wire_conversion_preserves_media_and_tool_response_order() {
        assert_eq!(
            convert_media_url("https://example/image.PNG?x=1", "image/jpeg")["fileData"]["mimeType"],
            "image/png"
        );
        assert_eq!(
            convert_media_url("data:audio/wav;base64,d2F2", "audio/mpeg")["inlineData"]["mimeType"],
            "audio/wav"
        );

        let assistant = Message::new(
            Role::Assistant,
            Vec::new(),
            vec![ToolCall {
                call_type: ToolCallType::Function,
                id: "call-a".to_owned(),
                name: "read".to_owned(),
                arguments: Some("{\"path\":\"a\"}".to_owned()),
                extras: None,
                stream_index: None,
            }],
        );
        let mut tool = Message::new(
            Role::Tool,
            vec![ContentPart::Text {
                text: "result".to_owned(),
            }],
            Vec::new(),
        );
        tool.tool_call_id = Some("call-a".to_owned());
        let contents = messages_to_google_gen_ai_contents(&[assistant, tool]).unwrap();
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["functionResponse"]["name"], "read");
        assert_eq!(
            contents[1]["parts"][0]["functionResponse"]["response"]["output"],
            "result"
        );
    }

    #[test]
    fn request_policy_encodes_thinking_limits_and_structured_output() {
        let options = GenerateOptions {
            thinking: Some(crate::kosong::contract::provider::ThinkingRequestOptions {
                effort: ThinkingEffort::from("high"),
                keep: Some("ignored-on-this-wire".to_owned()),
            }),
            max_completion_tokens: Some(9_000),
            used_context_tokens: Some(98),
            max_context_tokens: Some(100),
            response_format: Some(ResponseFormat::JsonObject),
            ..GenerateOptions::default()
        };
        let request = build_google_gen_ai_request(
            "gemini-3-preview",
            &Map::new(),
            None,
            "system",
            &[],
            &[],
            Some(&options),
        )
        .unwrap();
        assert_eq!(request["config"]["thinkingConfig"]["thinkingLevel"], "HIGH");
        assert_eq!(request["config"]["maxOutputTokens"], 2);
        assert_eq!(request["config"]["responseMimeType"], "application/json");
        assert!(request["config"].get("responseJsonSchema").is_none());
        assert!(
            get_google_gen_ai_model_capability("gemini-2.5-pro")
                .unwrap()
                .thinking
        );
        assert!(get_google_gen_ai_model_capability("gemini-3-preview").is_none());
    }

    #[tokio::test]
    async fn streamed_message_extracts_parts_metadata_and_cancellation() {
        let response = serde_json::json!({
            "responseId":"response-1",
            "usageMetadata":{
                "promptTokenCount":10,
                "cachedContentTokenCount":4,
                "candidatesTokenCount":3
            },
            "candidates":[{
                "finishReason":"STOP",
                "content":{"parts":[
                    {"thought":true,"text":"reason","thoughtSignature":"signed"},
                    {"text":"answer"},
                    {"functionCall":{"name":"read","id":"call","args":{"path":"a"}},
                     "thoughtSignature":"tool-signed"}
                ]}
            }]
        });
        let mut message = GoogleGenAiStreamedMessage::from_response(response, None);
        let parts = message.by_ref().collect::<Vec<_>>().await;
        assert_eq!(parts.len(), 3);
        assert_eq!(message.id(), Some("response-1"));
        assert_eq!(message.finish_reason(), Some(FinishReason::Completed));
        assert_eq!(message.usage().unwrap().input_other, 6.0);
        let StreamedMessagePart::ToolCall(call) = parts[2].as_ref().unwrap() else {
            panic!("expected tool call")
        };
        assert!(call.id.starts_with("read_call_"));
        assert_eq!(call.arguments.as_deref(), Some("{\"path\":\"a\"}"));
        assert_eq!(
            call.extras.as_ref().unwrap()["thought_signature_b64"],
            "tool-signed"
        );

        let signal = CancellationToken::new();
        signal.cancel();
        let mut cancelled =
            GoogleGenAiStreamedMessage::from_response(serde_json::json!({}), Some(signal));
        let error = cancelled.next().await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), "The operation was aborted.");
        assert!(cancelled.next().await.is_none());
    }

    #[tokio::test]
    async fn provider_uses_google_name_and_requires_api_key_outside_vertex() {
        let mut options = GoogleGenAiOptions::new("gemini-2.5-pro");
        options.api_key = Some(String::new());
        let provider = GoogleGenAiChatProvider::new(options);
        assert_eq!(provider.name(), "google_genai");
        assert_eq!(provider.model_name(), "gemini-2.5-pro");
        let error = match provider.generate("", &[], &[], None).await {
            Ok(_) => panic!("missing key must fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "GoogleGenAIChatProvider: apiKey is required. Provide it via the constructor options, the provider's API-key environment variable, options.auth.apiKey on each request, or an OAuth login."
        );
    }

    #[tokio::test]
    async fn google_client_factory_wins_and_cache_only_serves_absent_auth() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let factory_received = Arc::clone(&received);
        let mut factory_options = GoogleGenAiOptions::new("gemini-2.5-pro");
        factory_options.client_factory = Some(Arc::new(move |auth| {
            factory_received.lock().unwrap().push(auth);
            Ok(Arc::new(StubGoogleGenAiClient))
        }));
        let factory_provider = GoogleGenAiChatProvider::new(factory_options);
        factory_provider.create_client(None).await.unwrap();
        let auth = ProviderRequestAuth {
            api_key: Some("request-key".into()),
            headers: Some(IndexMap::from([("x-request".into(), "yes".into())])),
        };
        factory_provider.create_client(Some(&auth)).await.unwrap();
        assert_eq!(
            *received.lock().unwrap(),
            vec![ProviderRequestAuth::default(), auth.clone()]
        );

        let mut cached_options = GoogleGenAiOptions::new("gemini-2.5-pro");
        cached_options.api_key = Some("default-key".into());
        let cached_provider = GoogleGenAiChatProvider::new(cached_options);
        let first = cached_provider.create_client(None).await.unwrap();
        let second = cached_provider.create_client(None).await.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let rebuilt = cached_provider.create_client(Some(&auth)).await.unwrap();
        assert!(!Arc::ptr_eq(&first, &rebuilt));
    }

    #[tokio::test]
    async fn vertex_adc_supplies_missing_project_and_defers_token_fetch_until_request() {
        let project_calls = Arc::new(AtomicUsize::new(0));
        let token_calls = Arc::new(AtomicUsize::new(0));
        let mut options = GoogleGenAiOptions::new("gemini-2.5-pro");
        options.vertex_ai = Some(true);
        options.api_key = Some(String::new());
        options.location = Some("us-central1".into());
        options.adc_provider = Some(Arc::new(StubGoogleAdcProvider {
            project_calls: Arc::clone(&project_calls),
            token_calls: Arc::clone(&token_calls),
        }));
        let provider = GoogleGenAiChatProvider::new(options);

        provider.create_client(None).await.unwrap();
        assert_eq!(project_calls.load(Ordering::SeqCst), 1);
        assert_eq!(token_calls.load(Ordering::SeqCst), 0);
    }
}
