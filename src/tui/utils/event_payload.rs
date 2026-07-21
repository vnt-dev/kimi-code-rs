use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::OnceLock,
};

use regex::{Captures, Regex};
use serde_json::{Map, Value};

pub const STREAMING_ARGS_PREVIEW_MAX_CHARS: usize = 64 * 1_024;

fn streaming_args_field_pattern() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(
                r#""(path|file_path|command|pattern|query|url|description|title|name)"\s*:\s*"((?:\\.|[^"\\])*)""#,
            )
            .ok()
        })
        .as_ref()
}

/// Original:
///   apps/kimi-code/src/tui/utils/event-payload.ts
///   appendStreamingArgsPreview()
pub fn append_streaming_args_preview(current: Option<&str>, next: Option<&str>) -> String {
    let existing = truncate_utf16(
        current.unwrap_or_default(),
        STREAMING_ARGS_PREVIEW_MAX_CHARS,
    );
    let Some(next) = next.filter(|next| !next.is_empty()) else {
        return existing;
    };
    let remaining =
        STREAMING_ARGS_PREVIEW_MAX_CHARS.saturating_sub(existing.encode_utf16().count());
    if remaining == 0 {
        return existing;
    }
    existing + &truncate_utf16(next, remaining)
}

pub fn parse_streaming_args(arguments_text: &str) -> Map<String, Value> {
    let preview_text = truncate_utf16(arguments_text, STREAMING_ARGS_PREVIEW_MAX_CHARS);
    if preview_text.trim().is_empty() {
        return Map::new();
    }
    if arguments_text.encode_utf16().count() <= STREAMING_ARGS_PREVIEW_MAX_CHARS
        && preview_text.trim_end().ends_with('}')
        && let Ok(Value::Object(arguments)) = serde_json::from_str::<Value>(&preview_text)
    {
        return arguments;
    }

    let mut result = Map::new();
    if let Some(pattern) = streaming_args_field_pattern() {
        for captures in pattern.captures_iter(&preview_text) {
            let (Some(key), Some(raw_value)) = (captures.get(1), captures.get(2)) else {
                continue;
            };
            if !result.contains_key(key.as_str()) {
                result.insert(
                    key.as_str().to_owned(),
                    Value::String(unescape_json_string(raw_value.as_str())),
                );
            }
        }
    }
    result
}

fn unescape_json_string(value: &str) -> String {
    fn escape_pattern() -> Option<&'static Regex> {
        static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
        PATTERN
            .get_or_init(|| Regex::new(r#"\\(["\\/bfnrt])"#).ok())
            .as_ref()
    }
    let Some(pattern) = escape_pattern() else {
        return value.to_owned();
    };
    pattern
        .replace_all(value, |captures: &Captures<'_>| {
            match captures.get(1).map(|value| value.as_str()) {
                Some("n") => "\n".to_owned(),
                Some("t") => "\t".to_owned(),
                Some("r") => "\r".to_owned(),
                Some("b") => "\u{8}".to_owned(),
                Some("f") => "\u{c}".to_owned(),
                Some("\"") => "\"".to_owned(),
                Some("\\") => "\\".to_owned(),
                Some("/") => "/".to_owned(),
                Some(value) => value.to_owned(),
                None => String::new(),
            }
        })
        .into_owned()
}

pub fn args_record(args: &Value) -> Map<String, Value> {
    args.as_object().cloned().unwrap_or_default()
}

pub fn serialize_tool_result_output(output: &Value) -> Result<String, serde_json::Error> {
    match output {
        Value::String(output) => Ok(output.clone()),
        _ => serde_json::to_string_pretty(output),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoItemStatus {
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub title: String,
    pub status: TodoItemStatus,
}

pub fn todo_item_from_value(value: &Value) -> Option<TodoItem> {
    let object = value.as_object()?;
    let title = object.get("title")?.as_str()?.to_owned();
    if title.is_empty() {
        return None;
    }
    let status = match object.get("status")?.as_str()? {
        "pending" => TodoItemStatus::Pending,
        "in_progress" => TodoItemStatus::InProgress,
        "done" => TodoItemStatus::Done,
        _ => return None,
    };
    Some(TodoItem { title, status })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    pub details: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KimiPayloadError(pub ErrorPayload);

impl Display for KimiPayloadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0.message)
    }
}

impl Error for KimiPayloadError {}

pub fn format_error_message(error: &(dyn Error + 'static)) -> String {
    error
        .downcast_ref::<KimiPayloadError>()
        .map(|error| format_error_payload(&error.0))
        .unwrap_or_else(|| error.to_string())
}

pub fn format_error_payload(error: &ErrorPayload) -> String {
    let message = format_provider_filtered_message(error.details.as_ref())
        .unwrap_or_else(|| error.message.clone());
    format!("[{}] {message}", error.code)
}

fn format_provider_filtered_message(details: Option<&Map<String, Value>>) -> Option<String> {
    let finish_reason = string_detail(details, "finishReason");
    let raw_finish_reason = string_detail(details, "rawFinishReason");
    if finish_reason != Some("filtered") && raw_finish_reason != Some("content_filter") {
        return None;
    }
    let normalized_finish_reason = finish_reason.unwrap_or("filtered");
    let raw = raw_finish_reason
        .map(|reason| format!(", rawFinishReason={reason}"))
        .unwrap_or_default();
    Some(format!(
        "Provider filtered the response before visible output (finishReason={normalized_finish_reason}{raw})."
    ))
}

fn string_detail<'a>(details: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a str> {
    details?.get(key)?.as_str()
}

pub fn string_value(value: &Value) -> Option<&str> {
    value.as_str()
}

fn truncate_utf16(value: &str, maximum: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > maximum {
                return false;
            }
            units = next;
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_and_partial_streaming_arguments() {
        assert_eq!(
            parse_streaming_args(r#"{"command":"echo hi","path":"/tmp/a"}"#),
            Map::from_iter([
                ("command".to_owned(), Value::String("echo hi".to_owned())),
                ("path".to_owned(), Value::String("/tmp/a".to_owned())),
            ])
        );
        assert_eq!(
            parse_streaming_args(r#"{"command":"echo\nhi","description":"unfinished"#)
                .get("command"),
            Some(&Value::String("echo\nhi".to_owned()))
        );
    }

    #[test]
    fn caps_accumulated_and_parsed_previews_using_utf16_length() {
        let current = "a".repeat(STREAMING_ARGS_PREVIEW_MAX_CHARS - 2);
        assert_eq!(
            append_streaming_args_preview(Some(&current), Some("bcdef")),
            format!("{current}bc")
        );
        let oversized = format!(
            r#"{{"command":"echo ok","description":"{}"}}"#,
            "x".repeat(STREAMING_ARGS_PREVIEW_MAX_CHARS + 100)
        );
        assert_eq!(
            parse_streaming_args(&oversized),
            Map::from_iter([("command".to_owned(), Value::String("echo ok".to_owned()))])
        );
    }

    #[test]
    fn normalizes_records_tool_outputs_and_todo_shapes() {
        assert!(args_record(&Value::Array(Vec::new())).is_empty());
        assert!(
            serialize_tool_result_output(&Value::String("raw".to_owned()))
                .is_ok_and(|value| value == "raw")
        );
        assert!(matches!(
            todo_item_from_value(&serde_json::json!({"title":"Ship", "status":"in_progress"})),
            Some(TodoItem {
                status: TodoItemStatus::InProgress,
                ..
            })
        ));
        assert_eq!(
            todo_item_from_value(&serde_json::json!({"title":"", "status":"done"})),
            None
        );
    }

    #[test]
    fn formats_concise_provider_filter_payloads() {
        let payload = ErrorPayload {
            code: "provider.api_error".to_owned(),
            message: "long provider explanation".to_owned(),
            details: Some(Map::from_iter([
                (
                    "finishReason".to_owned(),
                    Value::String("filtered".to_owned()),
                ),
                (
                    "rawFinishReason".to_owned(),
                    Value::String("content_filter".to_owned()),
                ),
            ])),
        };
        let expected = "[provider.api_error] Provider filtered the response before visible output (finishReason=filtered, rawFinishReason=content_filter).";

        assert_eq!(format_error_payload(&payload), expected);
        assert_eq!(format_error_message(&KimiPayloadError(payload)), expected);
    }

    #[test]
    fn keeps_normal_error_messages_and_string_values() {
        let payload = ErrorPayload {
            code: "config.invalid".to_owned(),
            message: "bad config".to_owned(),
            details: None,
        };
        assert_eq!(
            format_error_payload(&payload),
            "[config.invalid] bad config"
        );
        assert_eq!(string_value(&Value::String("x".to_owned())), Some("x"));
        assert_eq!(string_value(&Value::Bool(true)), None);
    }
}
