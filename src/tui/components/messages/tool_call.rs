use std::{any::Any, path::Path};

use regex::Regex;
use serde_json::{Map, Value};

use crate::{
    sdk::types::TokenUsage,
    tui::{
        components::{
            Component, ComponentRole, Text,
            render::{truncate_to_width, visible_width},
        },
        utils::render_cache::is_render_cache_enabled,
    },
    utils::usage::usage_format::format_token_count,
};

const MAX_ARG_LENGTH: usize = 60;
const STREAMING_ARGS_PREVIEW_MAX_CHARS: usize = 8_000;
const APPROVED_PLAN_MARKER: &str = "## Approved Plan:";
const AUTO_APPROVED_PLAN_MARKER: &str = "## Plan (auto-approved, not user-reviewed):";
const REJECT_PREFIX: &str = "User rejected the plan.";
const REJECT_FEEDBACK_PREFIX: &str = "User rejected the plan. Feedback:";
const PLAN_REJECT_PREFIX: &str = "Plan rejected by user.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskStatus {
    Completed,
    Failed,
    TimedOut,
    Killed,
    Lost,
}

pub fn background_failure_message(status: Option<BackgroundTaskStatus>) -> Option<&'static str> {
    match status {
        Some(BackgroundTaskStatus::Lost) => {
            Some("Background agent lost (session restarted before completion)")
        }
        Some(BackgroundTaskStatus::Killed) => Some("Background agent killed"),
        Some(BackgroundTaskStatus::TimedOut) => Some("Background agent timed out"),
        Some(BackgroundTaskStatus::Failed) => Some("Background agent failed"),
        Some(BackgroundTaskStatus::Completed) | None => None,
    }
}

pub fn format_subagent_context_tokens(context_tokens: Option<u64>) -> Option<String> {
    context_tokens
        .filter(|tokens| *tokens > 0)
        .map(|tokens| format!("{} tok", format_token_count(tokens as f64)))
}

pub fn usage_input_total(usage: &TokenUsage) -> u64 {
    usage
        .input_other
        .saturating_add(usage.input_cache_read)
        .saturating_add(usage.input_cache_creation)
}

pub fn usage_total(usage: Option<&TokenUsage>) -> u64 {
    usage.map_or(0, |usage| {
        usage_input_total(usage).saturating_add(usage.output)
    })
}

pub fn format_subagent_tokens(usage: Option<&TokenUsage>) -> Option<String> {
    let total = usage_total(usage);
    (total > 0).then(|| format!("{} tok", format_token_count(total as f64)))
}

pub fn format_byte_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

pub fn extract_approved_plan(output: &str) -> String {
    let marker = if output.contains(AUTO_APPROVED_PLAN_MARKER) {
        AUTO_APPROVED_PLAN_MARKER
    } else {
        APPROVED_PLAN_MARKER
    };
    output
        .find(marker)
        .map(|index| output[index + marker.len()..].trim().to_owned())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitPlanModeOutcomeKind {
    Approved,
    AutoApproved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitPlanModeOutcome {
    pub kind: ExitPlanModeOutcomeKind,
    pub chosen: Option<String>,
    pub feedback: Option<String>,
    pub path: Option<String>,
}

impl ExitPlanModeOutcome {
    fn new(kind: ExitPlanModeOutcomeKind) -> Self {
        Self {
            kind,
            chosen: None,
            feedback: None,
            path: None,
        }
    }
}

/// Parses the string protocol emitted by ExitPlanMode results.
pub fn interpret_exit_plan_mode_outcome(output: &str) -> ExitPlanModeOutcome {
    if output.starts_with(REJECT_PREFIX) {
        let mut outcome = ExitPlanModeOutcome::new(ExitPlanModeOutcomeKind::Rejected);
        if let Some(feedback) = output.strip_prefix(REJECT_FEEDBACK_PREFIX) {
            outcome.feedback = Some(feedback.trim_start().to_owned());
        }
        return outcome;
    }
    if output.starts_with(PLAN_REJECT_PREFIX) {
        return ExitPlanModeOutcome::new(ExitPlanModeOutcomeKind::Rejected);
    }

    let path = capture(output, r"\nPlan saved to: ([^\n]+)\n", 1)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned);
    if output.contains(AUTO_APPROVED_PLAN_MARKER) {
        let mut outcome = ExitPlanModeOutcome::new(ExitPlanModeOutcomeKind::AutoApproved);
        outcome.path = path;
        return outcome;
    }

    let chosen = capture(
        output,
        r"^Exited plan mode\. Selected approach: ([^\n]+)\n",
        1,
    )
    .or_else(|| capture(output, r#"^User approved option "([^"]+)"\."#, 1))
    .map(str::to_owned);
    let mut outcome = ExitPlanModeOutcome::new(ExitPlanModeOutcomeKind::Approved);
    outcome.chosen = chosen;
    outcome.path = path;
    outcome
}

pub fn is_exit_plan_mode_outcome_output(output: &str) -> bool {
    output.starts_with(REJECT_PREFIX)
        || output.starts_with(PLAN_REJECT_PREFIX)
        || output.starts_with("Exited plan mode.")
        || Regex::new(r#"^User approved option "[^"]+"\."#)
            .expect("static regex")
            .is_match(output)
        || output.contains(APPROVED_PLAN_MARKER)
        || output.contains(AUTO_APPROVED_PLAN_MARKER)
}

fn capture<'a>(text: &'a str, pattern: &str, group: usize) -> Option<&'a str> {
    Regex::new(pattern)
        .expect("static regex")
        .captures(text)
        .and_then(|captures| captures.get(group))
        .map(|value| value.as_str())
}

pub fn unescape_json_string(value: &str) -> String {
    let mut output = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        let Some(escaped) = characters.next() else {
            output.push('\\');
            break;
        };
        output.push(match escaped {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'b' => '\u{0008}',
            'f' => '\u{000c}',
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            other => other,
        });
    }
    output
}

/// Extracts a live JSON string field even before its closing quote arrives.
pub fn extract_partial_string_field(text: &str, key: &str) -> Option<String> {
    let opener = Regex::new(&format!(r#""{}"\s*:\s*""#, regex::escape(key))).ok()?;
    let found = opener.find(text)?;
    let mut output = String::new();
    let mut characters = text[found.end()..].chars().peekable();
    while let Some(character) = characters.next() {
        if character == '"' {
            return Some(output);
        }
        if character != '\\' {
            output.push(character);
            continue;
        }
        let Some(escaped) = characters.next() else {
            return Some(output);
        };
        match escaped {
            'n' => output.push('\n'),
            't' => output.push('\t'),
            'r' => output.push('\r'),
            'b' => output.push('\u{0008}'),
            'f' => output.push('\u{000c}'),
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            '/' => output.push('/'),
            'u' => {
                let hex = characters.by_ref().take(4).collect::<String>();
                if hex.len() < 4 {
                    return Some(output);
                }
                let Ok(code) = u32::from_str_radix(&hex, 16) else {
                    return Some(output);
                };
                let Some(decoded) = char::from_u32(code) else {
                    return Some(output);
                };
                output.push(decoded);
            }
            other => output.push(other),
        }
    }
    Some(output)
}

/// Builds a best-effort argument object from complete or partial JSON.
pub fn parse_args_preview(value: &str) -> Map<String, Value> {
    let preview = value
        .chars()
        .take(STREAMING_ARGS_PREVIEW_MAX_CHARS)
        .collect::<String>();
    if preview.trim().is_empty() {
        return Map::new();
    }
    if value.chars().count() <= STREAMING_ARGS_PREVIEW_MAX_CHARS
        && preview.trim_end().ends_with('}')
        && let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&preview)
    {
        return parsed;
    }

    let field =
        Regex::new(r#""([^"\\]+)"\s*:\s*"((?:\\.|[^"\\])*)"?"#).expect("static field regex");
    let mut result = Map::new();
    for captures in field.captures_iter(&preview) {
        let (Some(key), Some(raw)) = (captures.get(1), captures.get(2)) else {
            continue;
        };
        result
            .entry(key.as_str().to_owned())
            .or_insert_with(|| Value::String(unescape_json_string(raw.as_str())));
    }
    result
}

pub fn truncate_arg_value(key: &str, value: &str) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() <= MAX_ARG_LENGTH {
        return value.to_owned();
    }
    if matches!(key, "path" | "file_path") {
        return format!(
            "…{}",
            characters[characters.len() - (MAX_ARG_LENGTH - 1)..]
                .iter()
                .collect::<String>()
        );
    }
    format!(
        "{}...",
        characters[..MAX_ARG_LENGTH - 3].iter().collect::<String>()
    )
}

pub fn make_workspace_relative_path(file_path: &str, workspace_dir: Option<&str>) -> String {
    let Some(workspace_dir) = workspace_dir.filter(|path| !path.is_empty()) else {
        return file_path.to_owned();
    };
    let file = Path::new(file_path);
    if !file.is_absolute() {
        return file_path.to_owned();
    }
    let Ok(relative) = file.strip_prefix(Path::new(workspace_dir)) else {
        return file_path.to_owned();
    };
    if relative.as_os_str().is_empty() {
        file_path.to_owned()
    } else {
        relative.to_string_lossy().into_owned()
    }
}

pub fn extract_key_argument(
    tool_name: &str,
    args: &Map<String, Value>,
    workspace_dir: Option<&str>,
) -> Option<String> {
    if tool_name == "Glob" {
        let pattern = args
            .get("pattern")?
            .as_str()
            .filter(|value| !value.is_empty())?;
        let mut summary = pattern.to_owned();
        if let Some(path) = args
            .get("path")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            summary.push_str(" · ");
            summary.push_str(&make_workspace_relative_path(path, workspace_dir));
        }
        if args.get("include_ignored").and_then(Value::as_bool) == Some(true) {
            summary.push_str(" · include ignored");
        }
        return Some(truncate_arg_value("pattern", &summary));
    }

    let candidates: &[&str] = match tool_name {
        "Bash" => &["command"],
        "Read" | "Write" | "Edit" => &["path", "file_path"],
        "Grep" | "Glob" => &["pattern"],
        "FetchURL" => &["url"],
        "WebSearch" => &["query"],
        "Agent" => &["description", "prompt"],
        _ => {
            return args.iter().find_map(|(key, value)| {
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(|value| format_key_argument(tool_name, key, value, workspace_dir))
            });
        }
    };
    candidates.iter().find_map(|key| {
        args.get(*key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| format_key_argument(tool_name, key, value, workspace_dir))
    })
}

fn format_key_argument(
    tool_name: &str,
    key: &str,
    value: &str,
    workspace_dir: Option<&str>,
) -> String {
    let first_line = value.lines().next().unwrap_or(value);
    let display = if tool_name == "Bash" && value.contains('\n') {
        format!("{first_line}…")
    } else {
        first_line.to_owned()
    };
    let display = if tool_name == "Read" && matches!(key, "path" | "file_path") {
        make_workspace_relative_path(&display, workspace_dir)
    } else {
        display
    };
    truncate_arg_value(key, &display)
}

pub fn format_subagent_label(agent_name: Option<&str>) -> String {
    let Some(raw) = agent_name.map(str::trim).filter(|name| !name.is_empty()) else {
        return "SubAgent".to_owned();
    };
    let label = raw
        .split(|character: char| character == '-' || character == '_' || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    if label
        .split_whitespace()
        .last()
        .is_some_and(|part| part.eq_ignore_ascii_case("agent"))
    {
        label
    } else {
        format!("{label} Agent")
    }
}

pub fn tail_non_empty_lines(text: &str, max_lines: usize) -> Vec<String> {
    let mut lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines.drain(..lines.len() - max_lines);
    }
    lines
}

/// Prefixes wrapped text and optionally keeps a fixed-height tail window.
///
/// Original: `PrefixedWrappedLine` in `tool-call.ts`.
pub struct PrefixedWrappedLine {
    first_prefix: String,
    continuation_prefix: String,
    text: String,
    tail_lines: Option<usize>,
    min_lines: Option<usize>,
    render_cache: Option<(usize, Vec<String>)>,
}

impl PrefixedWrappedLine {
    pub fn new(
        first_prefix: impl Into<String>,
        continuation_prefix: impl Into<String>,
        text: impl Into<String>,
        tail_lines: Option<usize>,
        min_lines: Option<usize>,
    ) -> Self {
        Self {
            first_prefix: first_prefix.into(),
            continuation_prefix: continuation_prefix.into(),
            text: text.into(),
            tail_lines,
            min_lines,
            render_cache: None,
        }
    }
}

impl Component for PrefixedWrappedLine {
    fn render(&mut self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }
        if is_render_cache_enabled()
            && let Some((cached_width, lines)) = &self.render_cache
            && *cached_width == width
        {
            return lines.clone();
        }

        let prefix_width =
            visible_width(&self.first_prefix).max(visible_width(&self.continuation_prefix));
        let content_width = width.saturating_sub(prefix_width).max(1);
        let mut wrapped = Text::new(&self.text, 0, 0).render(content_width);
        if let Some(tail_lines) = self.tail_lines
            && wrapped.len() > tail_lines
        {
            wrapped.drain(..wrapped.len() - tail_lines);
        }
        if let Some(min_lines) = self.min_lines {
            wrapped.resize(min_lines.max(wrapped.len()), String::new());
        }
        let rendered = wrapped
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                let prefix = if index == 0 {
                    &self.first_prefix
                } else {
                    &self.continuation_prefix
                };
                truncate_to_width(&format!("{prefix}{line}"), width, "…", false)
            })
            .collect::<Vec<_>>();
        if is_render_cache_enabled() {
            self.render_cache = Some((width, rendered.clone()));
        }
        rendered
    }

    fn invalidate(&mut self) {
        self.render_cache = None;
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exit_plan_mode_protocol_variants() {
        let approved = interpret_exit_plan_mode_outcome(
            "Exited plan mode. Selected approach: Fast\n\nPlan saved to: /tmp/plan.md\n\n## Approved Plan:\nDo it",
        );
        assert_eq!(approved.kind, ExitPlanModeOutcomeKind::Approved);
        assert_eq!(approved.chosen.as_deref(), Some("Fast"));
        assert_eq!(approved.path.as_deref(), Some("/tmp/plan.md"));
        assert_eq!(
            extract_approved_plan("x\n## Approved Plan:\nDo it"),
            "Do it"
        );

        let automatic = interpret_exit_plan_mode_outcome(
            "Exited plan mode.\n## Plan (auto-approved, not user-reviewed):\nAuto",
        );
        assert_eq!(automatic.kind, ExitPlanModeOutcomeKind::AutoApproved);
        let rejected =
            interpret_exit_plan_mode_outcome("User rejected the plan. Feedback:\n\nPlease revise");
        assert_eq!(rejected.kind, ExitPlanModeOutcomeKind::Rejected);
        assert_eq!(rejected.feedback.as_deref(), Some("Please revise"));
        assert!(is_exit_plan_mode_outcome_output("Plan rejected by user."));
    }

    #[test]
    fn extracts_complete_and_partial_streaming_arguments() {
        assert_eq!(
            extract_partial_string_field(r#"{"content":"first\nsecond"#, "content").as_deref(),
            Some("first\nsecond")
        );
        assert_eq!(
            extract_partial_string_field(r#"{"value":"\u4f60\u597d"}"#, "value").as_deref(),
            Some("你好")
        );
        let complete = parse_args_preview(r#"{"path":"src/main.rs","count":2}"#);
        assert_eq!(complete.get("count").and_then(Value::as_u64), Some(2));
        let partial = parse_args_preview(r#"{"path":"src/main.rs","content":"hello\nworld"#);
        assert_eq!(
            partial.get("path").and_then(Value::as_str),
            Some("src/main.rs")
        );
        assert_eq!(
            partial.get("content").and_then(Value::as_str),
            Some("hello\nworld")
        );
    }

    #[test]
    fn formats_key_arguments_paths_and_labels() {
        let args = serde_json::json!({
            "pattern": "*.rs",
            "path": "D:/work/src",
            "include_ignored": true
        });
        assert_eq!(
            extract_key_argument("Glob", args.as_object().unwrap(), Some("D:/work")),
            Some("*.rs · src · include ignored".to_owned())
        );
        let bash = serde_json::json!({"command": "echo first\necho second"});
        assert_eq!(
            extract_key_argument("Bash", bash.as_object().unwrap(), None),
            Some("echo first…".to_owned())
        );
        assert_eq!(format_subagent_label(None), "SubAgent");
        assert_eq!(
            format_subagent_label(Some("code-review")),
            "Code Review Agent"
        );
        assert_eq!(format_subagent_label(Some("Review Agent")), "Review Agent");
    }

    #[test]
    fn formats_sizes_elapsed_tokens_and_failures() {
        assert_eq!(format_byte_size(100), "100 B");
        assert_eq!(format_byte_size(1536), "1.5 KB");
        assert_eq!(format_byte_size(1_572_864), "1.5 MB");
        assert_eq!(format_elapsed(59), "59s");
        assert_eq!(format_elapsed(125), "2m 5s");
        assert_eq!(
            format_subagent_context_tokens(Some(1536)).as_deref(),
            Some("1.5k tok")
        );
        assert_eq!(
            background_failure_message(Some(BackgroundTaskStatus::Killed)),
            Some("Background agent killed")
        );
        assert_eq!(
            background_failure_message(Some(BackgroundTaskStatus::Completed)),
            None
        );
    }

    #[test]
    fn tails_non_empty_lines_and_prefixes_wrapped_windows() {
        assert_eq!(
            tail_non_empty_lines("one\n\n two  \nthree\n", 2),
            [" two", "three"]
        );
        let mut line =
            PrefixedWrappedLine::new("└─ ", "   ", "one two three four", Some(2), Some(3));
        let rendered = line.render(10);
        assert_eq!(rendered.len(), 3);
        assert!(rendered.iter().all(|line| visible_width(line) <= 10));
        assert!(rendered[0].starts_with("└─ "));
        assert!(rendered[1].starts_with("   "));
        assert!(rendered[2].starts_with("   "));
    }
}
