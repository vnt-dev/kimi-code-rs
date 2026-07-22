use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use crate::agent_core_v2::kosong::contract::message::{
    ContentPart, Message, Role, extract_text, is_tool_declaration_only_message,
};
use crate::agent_core_v2::kosong::contract::provider::{
    FinishReason, ResponseFormat, ToolCallIdPolicy,
};
use crate::agent_core_v2::kosong::contract::tool::Tool;
use crate::agent_core_v2::kosong::provider::bases::openai::openai_common::{
    NormalizedFinishReason, TOOL_RESULT_MEDIA_PLACEHOLDER, TOOL_RESULT_MEDIA_PROMPT,
    ToolMessageConversion, is_media_part,
};
use crate::agent_core_v2::kosong::provider::bases::tool_call_id::sanitize_openai_responses_call_id;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::message::MediaUrl;

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
}
