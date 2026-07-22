use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

use crate::agent_core_v2::kosong::contract::capability::ModelCapability;
use crate::agent_core_v2::kosong::contract::errors::ChatProviderError;
use crate::agent_core_v2::kosong::contract::message::{
    ContentPart, Message, Role, is_tool_declaration_only_message,
};
use crate::agent_core_v2::kosong::contract::provider::{
    FinishReason, GenerateOptions, ResponseFormat, ThinkingEffort,
};
use crate::agent_core_v2::kosong::contract::tool::Tool;
use crate::agent_core_v2::kosong::provider::bases::merge_user_messages::{
    ConsecutiveUserMessageMergePolicy, merge_consecutive_user_messages,
};
use crate::agent_core_v2::kosong::provider::bases::openai::openai_common::NormalizedFinishReason;

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
            && max > 0.0
        {
            cap = javascript_min(cap, max - used);
        }
        kwargs.insert(
            "maxOutputTokens".to_owned(),
            Value::from(javascript_max(1.0, cap)),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::message::{ToolCall, ToolCallType};

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
            thinking: Some(
                crate::agent_core_v2::kosong::contract::provider::ThinkingRequestOptions {
                    effort: ThinkingEffort::from("high"),
                    keep: Some("ignored-on-this-wire".to_owned()),
                },
            ),
            max_completion_tokens: Some(9_000.0),
            used_context_tokens: Some(98.0),
            max_context_tokens: Some(100.0),
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
        assert_eq!(request["config"]["maxOutputTokens"], 2.0);
        assert_eq!(request["config"]["responseMimeType"], "application/json");
        assert!(request["config"].get("responseJsonSchema").is_none());
        assert!(
            get_google_gen_ai_model_capability("gemini-2.5-pro")
                .unwrap()
                .thinking
        );
        assert!(get_google_gen_ai_model_capability("gemini-3-preview").is_none());
    }
}
