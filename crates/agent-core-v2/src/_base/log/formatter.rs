use std::{collections::HashSet, sync::LazyLock};

use regex::Regex;
use serde_json::Value;

use super::contract::{LogContext, LogEntry, LogEntryError, LogLevel};
use crate::_base::utils::iso_date_time::format_millis_rfc3339;

pub const MSG_MAX_CHARS: usize = 200;
pub const CTX_VALUE_MAX_CHARS: usize = 2048;
pub const STACK_MAX_BYTES: usize = 2048;
pub const ENTRY_MAX_BYTES: usize = 4096;
pub const REDACT_MAX_DEPTH: usize = 10;

const ELLIPSIS: &str = "…";
const TRUNCATED_TAIL: &str = " …truncated";
const REDACTED: &str = "[REDACTED]";
const REDACTED_KEYS: &[&str] = &[
    "authorization",
    "apikey",
    "token",
    "refreshtoken",
    "accesstoken",
    "idtoken",
    "password",
    "secret",
    "clientsecret",
    "apisecret",
    "cookie",
    "setcookie",
    "bearer",
];

static SAFE_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w.-]+$").expect("static regex"));
static NEEDS_QUOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[\s="\\]"#).expect("static regex"));
static RAW_SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r#"(?i)\b(authorization\s*[:=]\s*bearer\s+)[^\s"'`]+"#,
        r#"(?i)\b((?:api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|token|password|secret)\s*[:=]\s*)[^\s"'`]+"#,
        r#"(?i)\b(cookie\s*[:=]\s*)[^\r\n]+"#,
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("static regex"))
    .collect()
});

#[derive(Clone, Debug, Default)]
pub struct FormatOptions {
    pub ansi: bool,
    pub omit_context_keys: HashSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormattedEntry {
    pub text: String,
    pub dropped: bool,
}

pub fn redact_context(context: &LogContext) -> LogContext {
    context
        .iter()
        .map(|(key, value)| {
            let value = if is_redacted_key(key) {
                Value::String(REDACTED.into())
            } else {
                redact_value(value, 1)
            };
            (key.clone(), value)
        })
        .collect()
}

fn redact_value(value: &Value, depth: usize) -> Value {
    if depth > REDACT_MAX_DEPTH {
        return Value::String("[REDACTED:depth]".into());
    }
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| redact_value(value, depth + 1))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let value = if is_redacted_key(key) {
                        Value::String(REDACTED.into())
                    } else {
                        redact_value(value, depth + 1)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        primitive => primitive.clone(),
    }
}

fn is_redacted_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | '.'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    REDACTED_KEYS.contains(&normalized.as_str())
}

pub fn format_entry(entry: &LogEntry, options: &FormatOptions) -> FormattedEntry {
    let context = entry.context.as_ref().map(redact_context);
    let message = truncate_chars(&entry.message, MSG_MAX_CHARS);
    let pairs = context
        .iter()
        .flat_map(|context| context.iter())
        .filter(|(key, _)| !options.omit_context_keys.contains(*key))
        .map(|(key, value)| format_pair(key, value))
        .collect::<Vec<_>>();
    let timestamp = format_millis_rfc3339(entry.timestamp_ms);
    let label = match entry.level {
        LogLevel::Error => "ERROR",
        LogLevel::Warn => "WARN ",
        LogLevel::Info => "INFO ",
        LogLevel::Debug => "DEBUG",
        LogLevel::Off => "OFF  ",
    };
    let mut head = if pairs.is_empty() {
        format!("{timestamp} {label} {message}")
    } else {
        format!("{timestamp} {label} {message}  {}", pairs.join(" "))
    };
    head = clip_bytes(&head, ENTRY_MAX_BYTES);
    if options.ansi {
        let color = match entry.level {
            LogLevel::Error => "\u{1b}[31m",
            LogLevel::Warn => "\u{1b}[33m",
            LogLevel::Info => "\u{1b}[36m",
            LogLevel::Debug | LogLevel::Off => "\u{1b}[90m",
        };
        head = format!("{color}{head}\u{1b}[0m");
    }
    if let Some(error) = &entry.error {
        if let Some(stack) = &error.stack {
            head.push('\n');
            head.push_str(&indent_stack(&clip_bytes(
                &redact_string(stack),
                STACK_MAX_BYTES,
            )));
        } else {
            head.push_str("\n  Error: ");
            head.push_str(&redact_string(&error.message));
        }
    }
    FormattedEntry {
        text: head,
        dropped: false,
    }
}

fn format_pair(key: &str, value: &Value) -> String {
    let serialized = match value {
        Value::String(value) => redact_string(value),
        value => serde_json::to_string(value).unwrap_or_else(|_| "null".into()),
    };
    let limited = truncate_chars(&serialized, CTX_VALUE_MAX_CHARS);
    let key = if SAFE_KEY.is_match(key) {
        key.to_owned()
    } else {
        quote(key)
    };
    let value = if limited.is_empty() || NEEDS_QUOTE.is_match(&limited) {
        quote(&limited)
    } else {
        limited
    };
    format!("{key}={value}")
}

fn redact_string(value: &str) -> String {
    RAW_SECRET_PATTERNS
        .iter()
        .fold(value.to_owned(), |value, pattern| {
            pattern
                .replace_all(&value, format!("${{1}}{REDACTED}"))
                .into_owned()
        })
}

fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    let mut characters = value.chars();
    let prefix = characters.by_ref().take(maximum).collect::<String>();
    if characters.next().is_none() {
        prefix
    } else {
        prefix
            .chars()
            .take(maximum.saturating_sub(1))
            .collect::<String>()
            + ELLIPSIS
    }
}

fn clip_bytes(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let budget = maximum.saturating_sub(TRUNCATED_TAIL.len());
    let mut end = budget.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned() + TRUNCATED_TAIL
}

fn indent_stack(stack: &str) -> String {
    stack
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("  {line}")
            } else {
                format!("    {}", line.trim_start())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn extract_error(error: &(dyn std::error::Error + 'static)) -> LogEntryError {
    LogEntryError {
        message: error.to_string(),
        stack: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn formats_logfmt_and_redacts_keys_and_raw_secrets() {
        let entry = LogEntry {
            timestamp_ms: 0,
            level: LogLevel::Info,
            message: "request".into(),
            context: Some(serde_json::Map::from_iter([
                ("api_key".into(), json!("secret-value")),
                ("detail".into(), json!("Authorization: Bearer abc123")),
                ("with space".into(), json!("two words")),
            ])),
            error: None,
        };
        assert_eq!(
            format_entry(&entry, &FormatOptions::default()).text,
            "1970-01-01T00:00:00.000Z INFO  request  api_key=[REDACTED] detail=\"Authorization: Bearer [REDACTED]\" \"with space\"=\"two words\""
        );
    }

    #[test]
    fn clips_utf8_by_bytes_and_indents_redacted_stack() {
        let entry = LogEntry {
            timestamp_ms: 0,
            level: LogLevel::Error,
            message: "x".repeat(5_000),
            context: None,
            error: Some(LogEntryError {
                message: "bad".into(),
                stack: Some("Error: token=abc\n  at call".into()),
            }),
        };
        let formatted = format_entry(&entry, &FormatOptions::default());
        assert!(formatted.text.contains("…"));
        assert!(formatted.text.contains("token=[REDACTED]"));
        assert!(formatted.text.contains("\n    at call"));
    }
}
