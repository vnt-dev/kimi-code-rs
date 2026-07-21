use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use crate::sdk::types::{
    ContentPart, ContextMessage, ContextMessageRole, PromptOriginKind, ToolCall,
};

const HINT_KEYS: [&str; 7] = [
    "path",
    "file_path",
    "command",
    "query",
    "url",
    "name",
    "pattern",
];
const MAX_HINT_WIDTH: usize = 60;

/// Original:
///   apps/kimi-code/src/tui/utils/export-markdown.ts
///   extractToolCallHint()
pub fn extract_tool_call_hint(arguments_json: &str) -> String {
    let Ok(Value::Object(arguments)) = serde_json::from_str::<Value>(arguments_json) else {
        return String::new();
    };
    for key in HINT_KEYS {
        if let Some(Value::String(value)) = arguments.get(key)
            && !value.trim().is_empty()
        {
            return shorten(value, MAX_HINT_WIDTH);
        }
    }
    for value in arguments.values() {
        if let Value::String(value) = value
            && !value.is_empty()
            && value.encode_utf16().count() <= 80
        {
            return shorten(value, MAX_HINT_WIDTH);
        }
    }
    String::new()
}

fn shorten(text: &str, width: usize) -> String {
    if text.encode_utf16().count() <= width {
        return text.to_owned();
    }
    let mut units = 0;
    let prefix = text
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > width {
                return false;
            }
            units = next;
            true
        })
        .collect::<String>();
    format!("{prefix}…")
}

pub fn format_content_part_markdown(part: &ContentPart) -> String {
    match part {
        ContentPart::Text { text } => text.clone(),
        ContentPart::Think { think } => {
            if think.trim().is_empty() {
                String::new()
            } else {
                format!("<details><summary>Thinking</summary>\n\n{think}\n\n</details>")
            }
        }
        ContentPart::ImageUrl { .. } => "[image]".to_owned(),
        ContentPart::AudioUrl { .. } => "[audio]".to_owned(),
        ContentPart::VideoUrl { .. } => "[video]".to_owned(),
    }
}

pub fn format_tool_call_markdown(tool_call: &ToolCall) -> String {
    let arguments_raw = tool_call.arguments.as_deref().unwrap_or("{}");
    let hint = extract_tool_call_hint(arguments_raw);
    let mut title = format!("#### Tool Call: {}", tool_call.name);
    if !hint.is_empty() {
        title.push_str(&format!(" (`{hint}`)"));
    }
    let arguments_formatted = serde_json::from_str::<Value>(arguments_raw)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| arguments_raw.to_owned());
    format!(
        "{title}\n<!-- call_id: {} -->\n```json\n{arguments_formatted}\n```",
        tool_call.id
    )
}

fn format_tool_result_markdown(message: &ContextMessage, tool_name: &str, hint: &str) -> String {
    let call_id = message.tool_call_id.as_deref().unwrap_or("unknown");
    let result_text = message
        .content
        .iter()
        .map(format_content_part_markdown)
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut summary = format!("Tool Result: {tool_name}");
    if !hint.is_empty() {
        summary.push_str(&format!(" (`{hint}`)"));
    }
    format!(
        "<details><summary>{summary}</summary>\n\n<!-- call_id: {call_id} -->\n{result_text}\n\n</details>"
    )
}

pub fn is_internal_message(message: &ContextMessage) -> bool {
    message.origin.as_ref().is_some_and(|origin| {
        matches!(
            origin.kind,
            PromptOriginKind::Injection
                | PromptOriginKind::SystemTrigger
                | PromptOriginKind::CompactionSummary
                | PromptOriginKind::HookResult
                | PromptOriginKind::CronJob
                | PromptOriginKind::CronMissed
        )
    })
}

pub fn group_into_turns(history: &[ContextMessage]) -> Vec<Vec<&ContextMessage>> {
    let mut turns = Vec::new();
    let mut current = Vec::new();
    for message in history {
        if is_internal_message(message) {
            continue;
        }
        if message.role == ContextMessageRole::User && !current.is_empty() {
            turns.push(std::mem::take(&mut current));
        }
        current.push(message);
    }
    if !current.is_empty() {
        turns.push(current);
    }
    turns
}

fn format_turn_markdown(messages: &[&ContextMessage], turn_number: usize) -> String {
    let mut lines = vec![format!("## Turn {turn_number}"), String::new()];
    let mut tool_call_info: HashMap<&str, (&str, String)> = HashMap::new();
    let mut assistant_header_written = false;

    for message in messages {
        if is_internal_message(message) {
            continue;
        }
        match message.role {
            ContextMessageRole::User => {
                lines.extend(["### User".to_owned(), String::new()]);
                append_content_markdown(&mut lines, &message.content);
            }
            ContextMessageRole::Assistant => {
                if !assistant_header_written {
                    lines.extend(["### Assistant".to_owned(), String::new()]);
                    assistant_header_written = true;
                }
                append_content_markdown(&mut lines, &message.content);
                for tool_call in &message.tool_calls {
                    let hint =
                        extract_tool_call_hint(tool_call.arguments.as_deref().unwrap_or("{}"));
                    tool_call_info.insert(&tool_call.id, (&tool_call.name, hint));
                    lines.extend([format_tool_call_markdown(tool_call), String::new()]);
                }
            }
            ContextMessageRole::Tool => {
                let call_id = message.tool_call_id.as_deref().unwrap_or_default();
                let (name, hint) = tool_call_info
                    .get(call_id)
                    .map(|(name, hint)| (*name, hint.as_str()))
                    .unwrap_or(("unknown", ""));
                lines.extend([
                    format_tool_result_markdown(message, name, hint),
                    String::new(),
                ]);
            }
            ContextMessageRole::System => {
                lines.extend(["### System".to_owned(), String::new()]);
                append_content_markdown(&mut lines, &message.content);
            }
        }
    }
    lines.join("\n")
}

fn append_content_markdown(lines: &mut Vec<String>, content: &[ContentPart]) {
    for part in content {
        let text = format_content_part_markdown(part);
        if !text.trim().is_empty() {
            lines.extend([text, String::new()]);
        }
    }
}

fn build_overview(history: &[ContextMessage], turns: &[Vec<&ContextMessage>]) -> String {
    let topic = history
        .iter()
        .find(|message| message.role == ContextMessageRole::User && !is_internal_message(message))
        .map(|message| {
            let text = message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            shorten(&text, 80)
        })
        .unwrap_or_default();
    let tool_call_count = history
        .iter()
        .map(|message| message.tool_calls.len())
        .sum::<usize>();
    [
        "## Overview".to_owned(),
        String::new(),
        if topic.is_empty() {
            "- **Topic**: (empty)".to_owned()
        } else {
            format!("- **Topic**: {topic}")
        },
        format!(
            "- **Conversation**: {} turns | {tool_call_count} tool calls",
            turns.len()
        ),
        String::new(),
        "---".to_owned(),
    ]
    .join("\n")
}

pub struct BuildExportMarkdownInput<'a> {
    pub session_id: &'a str,
    pub work_dir: &'a str,
    pub history: &'a [ContextMessage],
    pub token_count: u64,
    pub now: DateTime<Utc>,
}

/// Original:
///   apps/kimi-code/src/tui/utils/export-markdown.ts
///   buildExportMarkdown()
pub fn build_export_markdown(input: &BuildExportMarkdownInput<'_>) -> String {
    let mut lines = vec![
        "---".to_owned(),
        format!("session_id: {}", input.session_id),
        format!(
            "exported_at: {}",
            input.now.to_rfc3339_opts(SecondsFormat::Millis, true)
        ),
        format!("work_dir: {}", input.work_dir),
        format!("message_count: {}", input.history.len()),
        format!("token_count: {}", input.token_count),
        "---".to_owned(),
        String::new(),
        "# Kimi Session Export".to_owned(),
        String::new(),
    ];
    let turns = group_into_turns(input.history);
    lines.push(build_overview(input.history, &turns));
    lines.push(String::new());
    for (index, turn) in turns.iter().enumerate() {
        lines.push(format_turn_markdown(turn, index + 1));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::sdk::types::{MediaUrl, PromptOrigin};

    fn origin(kind: PromptOriginKind) -> Option<PromptOrigin> {
        Some(PromptOrigin {
            kind,
            fields: Map::new(),
        })
    }

    fn user(text: &str, message_origin: Option<PromptOrigin>) -> ContextMessage {
        ContextMessage {
            role: ContextMessageRole::User,
            content: vec![ContentPart::Text {
                text: text.to_owned(),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
            origin: message_origin,
        }
    }

    fn assistant(text: &str, tool_calls: Vec<ToolCall>) -> ContextMessage {
        ContextMessage {
            role: ContextMessageRole::Assistant,
            content: vec![ContentPart::Text {
                text: text.to_owned(),
            }],
            tool_calls,
            tool_call_id: None,
            origin: None,
        }
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            tool_type: "function".to_owned(),
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: Some(arguments.to_owned()),
        }
    }

    #[test]
    fn extracts_preferred_and_fallback_tool_hints() {
        assert_eq!(
            extract_tool_call_hint(r#"{"command":"ls","path":"/a.ts"}"#),
            "/a.ts"
        );
        assert_eq!(extract_tool_call_hint(r#"{"foo":"hello"}"#), "hello");
        assert_eq!(extract_tool_call_hint("not json"), "");
        assert!(
            extract_tool_call_hint(&format!(r#"{{"path":"{}"}}"#, "a".repeat(100))).ends_with('…')
        );
    }

    #[test]
    fn formats_content_kinds_and_blank_thinking() {
        assert_eq!(
            format_content_part_markdown(&ContentPart::Text {
                text: "hello".to_owned()
            }),
            "hello"
        );
        assert!(
            format_content_part_markdown(&ContentPart::Think {
                think: "reasoning".to_owned()
            })
            .contains("<details><summary>Thinking</summary>")
        );
        assert_eq!(
            format_content_part_markdown(&ContentPart::Think {
                think: "  ".to_owned()
            }),
            ""
        );
        assert_eq!(
            format_content_part_markdown(&ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "http://x".to_owned(),
                    id: None
                }
            }),
            "[image]"
        );
    }

    #[test]
    fn formats_tool_call_with_pretty_arguments_and_hint() {
        let markdown = format_tool_call_markdown(&tool_call("c1", "Bash", r#"{"command":"ls"}"#));
        assert!(markdown.contains("#### Tool Call: Bash (`ls`)"));
        assert!(markdown.contains("<!-- call_id: c1 -->"));
        assert!(markdown.contains("\"command\": \"ls\""));
    }

    #[test]
    fn filters_internal_origins_and_groups_external_turns() {
        let history = [
            user("q1", origin(PromptOriginKind::User)),
            user("hidden", origin(PromptOriginKind::Injection)),
            assistant("a1", Vec::new()),
            user("q2", origin(PromptOriginKind::User)),
            assistant("a2", Vec::new()),
        ];
        let turns = group_into_turns(&history);

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].len(), 2);
        assert!(is_internal_message(&history[1]));
    }

    #[test]
    fn builds_complete_export_with_tool_results_and_counts() {
        let call = tool_call("c1", "Read", r#"{"file_path":"/foo.ts"}"#);
        let history = [
            user("read file", origin(PromptOriginKind::User)),
            assistant("let me read", vec![call]),
            ContextMessage {
                role: ContextMessageRole::Tool,
                content: vec![ContentPart::Text {
                    text: "file contents".to_owned(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: Some("c1".to_owned()),
                origin: None,
            },
        ];
        let now = DateTime::parse_from_rfc3339("2026-05-27T10:00:00+08:00")
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        let markdown = build_export_markdown(&BuildExportMarkdownInput {
            session_id: "ses_test",
            work_dir: "/tmp",
            history: &history,
            token_count: 1_234,
            now,
        });

        assert!(markdown.contains("exported_at: 2026-05-27T02:00:00.000Z"));
        assert!(markdown.contains("1 turns | 1 tool calls"));
        assert!(markdown.contains("Tool Result: Read (`/foo.ts`)"));
        assert!(markdown.contains("file contents"));
    }
}
