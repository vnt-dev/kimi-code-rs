use std::{
    any::Any,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value};

use crate::{
    sdk::types::TokenUsage,
    tui::{
        components::{
            Component, ComponentRole, Text,
            render::{truncate_to_width, visible_width},
        },
        types::{SubagentReplayBlockData, ToolCallBlockData, ToolResultBlockData},
        utils::{
            event_payload::{STREAMING_ARGS_PREVIEW_MAX_CHARS, append_streaming_args_preview},
            render_cache::is_render_cache_enabled,
        },
    },
    utils::usage::usage_format::format_token_count,
};

const MAX_ARG_LENGTH: usize = 60;
const MAX_SUB_TOOL_CALLS_SHOWN: usize = 4;
const MAX_PROGRESS_LINES: usize = 24;
const MAX_LIVE_OUTPUT_CHARS: usize = 50_000;
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
    let preview = truncate_utf16_prefix(value, STREAMING_ARGS_PREVIEW_MAX_CHARS);
    if preview.trim().is_empty() {
        return Map::new();
    }
    if value.encode_utf16().count() <= STREAMING_ARGS_PREVIEW_MAX_CHARS
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
    if value.encode_utf16().count() <= MAX_ARG_LENGTH {
        return value.to_owned();
    }
    if matches!(key, "path" | "file_path") {
        return format!("…{}", truncate_utf16_suffix(value, MAX_ARG_LENGTH - 1));
    }
    format!("{}...", truncate_utf16_prefix(value, MAX_ARG_LENGTH - 3))
}

fn truncate_utf16_prefix(value: &str, max_units: usize) -> String {
    let mut end = 0;
    let mut used = 0;
    for (index, character) in value.char_indices() {
        let units = character.len_utf16();
        if used + units > max_units {
            break;
        }
        used += units;
        end = index + character.len_utf8();
    }
    value[..end].to_owned()
}

fn truncate_utf16_suffix(value: &str, max_units: usize) -> String {
    let mut start = value.len();
    let mut used = 0;
    for (index, character) in value.char_indices().rev() {
        let units = character.len_utf16();
        if used + units > max_units {
            break;
        }
        used += units;
        start = index;
    }
    value[start..].to_owned()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentTextKind {
    Thinking,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentPhase {
    Queued,
    Spawning,
    Running,
    Done,
    Failed,
    Backgrounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPhase {
    Pending,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallSubagentSnapshot {
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_call_description: String,
    pub agent_name: Option<String>,
    pub phase: Option<SubagentPhase>,
    pub tool_count: usize,
    pub elapsed_seconds: Option<u64>,
    pub tokens: u64,
    pub is_error: bool,
    pub error_text: Option<String>,
    pub latest_activity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallReadSnapshot {
    pub tool_call_id: String,
    pub file_path: Option<String>,
    pub phase: ReadPhase,
    pub lines: usize,
}

#[derive(Debug, Clone)]
struct FinishedSubCall {
    name: String,
    args: Map<String, Value>,
    output: String,
    is_error: bool,
}

#[derive(Debug, Clone)]
struct OngoingSubCall {
    name: String,
    args: Map<String, Value>,
    streaming_arguments: Option<String>,
    live_output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSpawnMeta {
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub run_in_background: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubagentMetrics {
    pub context_tokens: Option<u64>,
    pub usage: Option<TokenUsage>,
}

/// Renderer-independent state machine owned by `ToolCallComponent`.
///
/// Rust adaptation: mutable event state is separated from terminal child
/// construction so grouping and replay can consume immutable snapshots.
pub struct ToolCallState {
    tool_call: ToolCallBlockData,
    result: Option<ToolResultBlockData>,
    workspace_dir: Option<String>,
    expanded: bool,
    plan_path: Option<String>,
    current_plan: Option<String>,
    progress_lines: Vec<String>,
    live_output: String,
    subagent_agent_id: Option<String>,
    subagent_agent_name: Option<String>,
    ongoing_sub_calls: IndexMap<String, OngoingSubCall>,
    finished_sub_calls: Vec<FinishedSubCall>,
    hidden_sub_call_count: usize,
    subagent_text: String,
    subagent_thinking_text: String,
    last_subagent_stream_kind: SubagentTextKind,
    subagent_phase: Option<SubagentPhase>,
    detached_from_foreground: bool,
    background_task_terminal_phase: Option<SubagentPhase>,
    subagent_context_tokens: Option<u64>,
    subagent_usage: Option<TokenUsage>,
    subagent_result_summary: Option<String>,
    subagent_error: Option<String>,
    subagent_started_at_ms: Option<u64>,
    subagent_ended_at_ms: Option<u64>,
    snapshot_listener: Option<Box<dyn FnMut() + Send>>,
}

impl ToolCallState {
    pub fn new(
        tool_call: ToolCallBlockData,
        result: Option<ToolResultBlockData>,
        workspace_dir: Option<String>,
    ) -> Self {
        let replay = tool_call.subagent.clone();
        let mut state = Self {
            tool_call,
            result,
            workspace_dir,
            expanded: false,
            plan_path: None,
            current_plan: None,
            progress_lines: Vec::new(),
            live_output: String::new(),
            subagent_agent_id: None,
            subagent_agent_name: None,
            ongoing_sub_calls: IndexMap::new(),
            finished_sub_calls: Vec::new(),
            hidden_sub_call_count: 0,
            subagent_text: String::new(),
            subagent_thinking_text: String::new(),
            last_subagent_stream_kind: SubagentTextKind::Text,
            subagent_phase: None,
            detached_from_foreground: false,
            background_task_terminal_phase: None,
            subagent_context_tokens: None,
            subagent_usage: None,
            subagent_result_summary: None,
            subagent_error: None,
            subagent_started_at_ms: None,
            subagent_ended_at_ms: None,
            snapshot_listener: None,
        };
        state.apply_subagent_replay(replay.as_ref());
        state
    }

    pub fn tool_call(&self) -> &ToolCallBlockData {
        &self.tool_call
    }

    pub fn result(&self) -> Option<&ToolResultBlockData> {
        self.result.as_ref()
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    pub fn progress_lines(&self) -> &[String] {
        &self.progress_lines
    }

    pub fn live_output(&self) -> &str {
        &self.live_output
    }

    pub fn last_subagent_stream_kind(&self) -> SubagentTextKind {
        self.last_subagent_stream_kind
    }

    pub fn latest_finished_sub_call_result(&self) -> Option<(&str, bool)> {
        self.finished_sub_calls
            .last()
            .map(|call| (call.output.as_str(), call.is_error))
    }

    pub fn sub_tool_live_output(&self, id: &str) -> Option<&str> {
        self.ongoing_sub_calls
            .get(id)
            .map(|call| call.live_output.as_str())
    }

    pub fn current_plan(&self) -> Option<&str> {
        self.current_plan.as_deref()
    }

    pub fn plan_path(&self) -> Option<&str> {
        self.plan_path.as_deref()
    }

    pub fn set_expanded(&mut self, expanded: bool) -> bool {
        if self.expanded == expanded {
            false
        } else {
            self.expanded = expanded;
            true
        }
    }

    pub fn set_result(&mut self, result: ToolResultBlockData) {
        self.result = Some(result);
        self.progress_lines.clear();
        self.live_output.clear();
        self.finalize_subagent_elapsed_if_needed();
        self.notify_snapshot_change();
    }

    pub fn update_tool_call(&mut self, tool_call: ToolCallBlockData) {
        self.tool_call = tool_call;
        self.notify_snapshot_change();
    }

    pub fn append_progress(&mut self, text: &str) {
        if self.result.is_some() {
            return;
        }
        self.progress_lines
            .extend(text.split('\n').map(str::to_owned));
        if self.progress_lines.len() > MAX_PROGRESS_LINES {
            self.progress_lines
                .drain(..self.progress_lines.len() - MAX_PROGRESS_LINES);
        }
        self.notify_snapshot_change();
    }

    pub fn append_live_output(&mut self, text: &str) {
        if self.result.is_some() || text.is_empty() {
            return;
        }
        self.live_output.push_str(text);
        truncate_live_output(&mut self.live_output);
        self.notify_snapshot_change();
    }

    pub fn set_plan_info(&mut self, plan: Option<&str>, path: Option<&str>) -> bool {
        if self.tool_call.name != "ExitPlanMode" {
            return false;
        }
        let mut changed = false;
        if let Some(plan) = plan.filter(|value| !value.is_empty())
            && self.current_plan.as_deref() != Some(plan)
        {
            self.current_plan = Some(plan.to_owned());
            changed = true;
        }
        if let Some(path) = path.filter(|value| !value.is_empty())
            && self.plan_path.as_deref() != Some(path)
        {
            self.plan_path = Some(path.to_owned());
            changed = true;
        }
        changed
    }

    pub fn set_snapshot_listener(&mut self, listener: Option<Box<dyn FnMut() + Send>>) {
        self.snapshot_listener = listener;
        self.notify_snapshot_change();
    }

    pub fn set_subagent_meta(&mut self, agent_id: &str, agent_name: Option<&str>) {
        if self.subagent_agent_id.as_deref() == Some(agent_id)
            && self.subagent_agent_name.as_deref() == agent_name
        {
            return;
        }
        self.subagent_agent_id = Some(agent_id.to_owned());
        self.subagent_agent_name = agent_name.map(str::to_owned);
        self.notify_snapshot_change();
    }

    pub fn on_subagent_spawned(&mut self, meta: &SubagentSpawnMeta) {
        self.subagent_agent_id = Some(meta.agent_id.clone());
        self.subagent_agent_name = meta.agent_name.clone();
        self.subagent_phase = Some(if meta.run_in_background {
            SubagentPhase::Backgrounded
        } else {
            SubagentPhase::Queued
        });
        self.subagent_started_at_ms = Some(now_ms());
        self.subagent_ended_at_ms = None;
        self.notify_snapshot_change();
    }

    pub fn on_subagent_started(&mut self, meta: &SubagentSpawnMeta) {
        self.subagent_agent_id = Some(meta.agent_id.clone());
        self.subagent_agent_name = meta.agent_name.clone();
        if !meta.run_in_background
            && matches!(
                self.subagent_phase,
                None | Some(SubagentPhase::Queued | SubagentPhase::Spawning)
            )
        {
            self.subagent_phase = Some(SubagentPhase::Running);
        }
        self.notify_snapshot_change();
    }

    pub fn on_subagent_completed(&mut self, metrics: SubagentMetrics, result_summary: &str) {
        self.subagent_phase = Some(SubagentPhase::Done);
        self.subagent_ended_at_ms.get_or_insert_with(now_ms);
        if metrics.context_tokens.is_some_and(|tokens| tokens > 0) {
            self.subagent_context_tokens = metrics.context_tokens;
        }
        self.subagent_usage = metrics.usage;
        self.subagent_result_summary =
            (!result_summary.is_empty()).then(|| result_summary.to_owned());
        if self.subagent_text.trim().is_empty()
            && let Some(summary) = &self.subagent_result_summary
        {
            self.subagent_text = summary.clone();
        }
        self.notify_snapshot_change();
    }

    pub fn update_subagent_metrics(&mut self, metrics: SubagentMetrics) {
        if metrics.context_tokens.is_some_and(|tokens| tokens > 0) {
            self.subagent_context_tokens = metrics.context_tokens;
        }
        if metrics.usage.is_some() {
            self.subagent_usage = metrics.usage;
        }
        self.notify_snapshot_change();
    }

    pub fn on_subagent_failed(&mut self, error: &str) {
        self.subagent_phase = Some(SubagentPhase::Failed);
        self.subagent_ended_at_ms.get_or_insert_with(now_ms);
        self.subagent_error = Some(error.to_owned());
        self.notify_snapshot_change();
    }

    pub fn set_background_task_terminal_status(
        &mut self,
        status: BackgroundTaskStatus,
        error_text: Option<&str>,
    ) {
        let phase = if status == BackgroundTaskStatus::Completed {
            SubagentPhase::Done
        } else {
            SubagentPhase::Failed
        };
        let phase_unchanged = self.background_task_terminal_phase == Some(phase);
        let mut error_changed = false;
        if phase == SubagentPhase::Failed {
            if let Some(error_text) = error_text
                && self.subagent_error.as_deref() != Some(error_text)
            {
                self.subagent_error = Some(error_text.to_owned());
                error_changed = true;
            } else if self.subagent_error.is_none()
                && let Some(generic) = background_failure_message(Some(status))
            {
                self.subagent_error = Some(generic.to_owned());
                error_changed = true;
            }
        }
        if phase_unchanged && !error_changed {
            return;
        }
        self.background_task_terminal_phase = Some(phase);
        self.subagent_ended_at_ms.get_or_insert_with(now_ms);
        self.notify_snapshot_change();
    }

    pub fn mark_backgrounded(&mut self) {
        if !self.detached_from_foreground {
            self.detached_from_foreground = true;
            self.subagent_phase = Some(SubagentPhase::Backgrounded);
            self.notify_snapshot_change();
        }
    }

    pub fn get_subagent_agent_id(&self) -> Option<String> {
        if self.subagent_agent_id.is_some() {
            return self.subagent_agent_id.clone();
        }
        if self.tool_call.name != "Agent" {
            return None;
        }
        let output = &self.result.as_ref()?.output;
        Regex::new(r"(?m)^agent_id:\s*(agent-[A-Za-z0-9_-]+)")
            .expect("static agent id regex")
            .captures(output)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_owned())
    }

    pub fn get_agent_tool_description(&self) -> Option<&str> {
        (self.tool_call.name == "Agent")
            .then(|| {
                self.tool_call
                    .args
                    .get("description")
                    .and_then(Value::as_str)
            })
            .flatten()
    }

    pub fn append_subagent_text(&mut self, text: &str, kind: SubagentTextKind) {
        self.last_subagent_stream_kind = kind;
        match kind {
            SubagentTextKind::Thinking => self.subagent_thinking_text.push_str(text),
            SubagentTextKind::Text => self.subagent_text.push_str(text),
        }
        self.mark_subagent_running();
        self.notify_snapshot_change();
    }

    pub fn append_sub_tool_call(&mut self, id: &str, name: &str, args: Map<String, Value>) {
        let streaming_arguments = self
            .ongoing_sub_calls
            .get(id)
            .and_then(|call| call.streaming_arguments.clone());
        self.ongoing_sub_calls.insert(
            id.to_owned(),
            OngoingSubCall {
                name: name.to_owned(),
                args,
                streaming_arguments,
                live_output: String::new(),
            },
        );
        self.mark_subagent_running();
        self.notify_snapshot_change();
    }

    pub fn append_sub_tool_call_delta(
        &mut self,
        id: &str,
        name: Option<&str>,
        arguments_part: Option<&str>,
    ) {
        let existing = self.ongoing_sub_calls.get(id);
        let arguments = append_streaming_args_preview(
            existing.and_then(|call| call.streaming_arguments.as_deref()),
            arguments_part,
        );
        let args = parse_args_preview(&arguments);
        let name = name
            .or_else(|| existing.map(|call| call.name.as_str()))
            .unwrap_or("Tool")
            .to_owned();
        let live_output = existing.map_or_else(String::new, |call| call.live_output.clone());
        self.ongoing_sub_calls.insert(
            id.to_owned(),
            OngoingSubCall {
                name,
                args,
                streaming_arguments: Some(arguments),
                live_output,
            },
        );
        self.mark_subagent_running();
        self.notify_snapshot_change();
    }

    pub fn append_sub_tool_live_output(&mut self, id: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(call) = self.ongoing_sub_calls.get_mut(id) else {
            return;
        };
        call.live_output.push_str(text);
        truncate_live_output(&mut call.live_output);
        self.notify_snapshot_change();
    }

    pub fn finish_sub_tool_call(&mut self, result: ToolResultBlockData) {
        let Some(ongoing) = self.ongoing_sub_calls.shift_remove(&result.tool_call_id) else {
            return;
        };
        self.finished_sub_calls.push(FinishedSubCall {
            name: ongoing.name,
            args: ongoing.args,
            output: result.output,
            is_error: result.is_error.unwrap_or(false),
        });
        if self.finished_sub_calls.len() > MAX_SUB_TOOL_CALLS_SHOWN {
            let hidden = self.finished_sub_calls.len() - MAX_SUB_TOOL_CALLS_SHOWN;
            self.finished_sub_calls.drain(..hidden);
            self.hidden_sub_call_count += hidden;
        }
        self.notify_snapshot_change();
    }

    pub fn get_subagent_snapshot(&self) -> ToolCallSubagentSnapshot {
        let phase = self.derived_subagent_phase();
        let tokens = self
            .subagent_context_tokens
            .filter(|tokens| *tokens > 0)
            .unwrap_or_else(|| usage_total(self.subagent_usage.as_ref()));
        let combined_text = [
            self.subagent_thinking_text.as_str(),
            self.subagent_text.as_str(),
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
        ToolCallSubagentSnapshot {
            tool_call_id: self.tool_call.id.clone(),
            tool_name: self.tool_call.name.clone(),
            tool_call_description: self
                .tool_call
                .args
                .get("description")
                .and_then(Value::as_str)
                .or(self.tool_call.description.as_deref())
                .unwrap_or_default()
                .to_owned(),
            agent_name: self.subagent_agent_name.clone(),
            phase,
            tool_count: self.finished_sub_calls.len() + self.hidden_sub_call_count,
            elapsed_seconds: self.subagent_elapsed_seconds(),
            tokens,
            is_error: phase == Some(SubagentPhase::Failed),
            error_text: self.subagent_error.clone().or_else(|| {
                (phase == Some(SubagentPhase::Failed))
                    .then(|| self.result.as_ref().map(|result| result.output.clone()))
                    .flatten()
            }),
            latest_activity: compute_latest_activity(
                &self.ongoing_sub_calls,
                &self.finished_sub_calls,
                &combined_text,
                self.workspace_dir.as_deref(),
            ),
        }
    }

    pub fn get_read_snapshot(&self) -> ToolCallReadSnapshot {
        let file_path = self
            .tool_call
            .args
            .get("file_path")
            .or_else(|| self.tool_call.args.get("path"))
            .and_then(Value::as_str)
            .map(|path| make_workspace_relative_path(path, self.workspace_dir.as_deref()));
        let (phase, lines) = match &self.result {
            None => (ReadPhase::Pending, 0),
            Some(result) if result.is_error == Some(true) => (ReadPhase::Failed, 0),
            Some(result) => (
                ReadPhase::Done,
                result
                    .output
                    .split('\n')
                    .filter(|line| !line.is_empty())
                    .count(),
            ),
        };
        ToolCallReadSnapshot {
            tool_call_id: self.tool_call.id.clone(),
            file_path,
            phase,
            lines,
        }
    }

    fn apply_subagent_replay(&mut self, subagent: Option<&SubagentReplayBlockData>) {
        let Some(subagent) = subagent else {
            return;
        };
        self.subagent_agent_id = Some(subagent.id.clone());
        self.subagent_agent_name = subagent.name.clone();
        self.subagent_text = subagent.text.clone().unwrap_or_default();
        for call in subagent.tool_calls.as_deref().unwrap_or_default() {
            if let Some(result) = &call.result {
                self.finished_sub_calls.push(FinishedSubCall {
                    name: call.name.clone(),
                    args: call.args.clone(),
                    output: result.output.clone(),
                    is_error: result.is_error.unwrap_or(false),
                });
            } else {
                self.ongoing_sub_calls.insert(
                    call.id.clone(),
                    OngoingSubCall {
                        name: call.name.clone(),
                        args: call.args.clone(),
                        streaming_arguments: None,
                        live_output: String::new(),
                    },
                );
            }
        }
        if self.finished_sub_calls.len() > MAX_SUB_TOOL_CALLS_SHOWN {
            let hidden = self.finished_sub_calls.len() - MAX_SUB_TOOL_CALLS_SHOWN;
            self.finished_sub_calls.drain(..hidden);
            self.hidden_sub_call_count = hidden;
        }
    }

    fn mark_subagent_running(&mut self) {
        if matches!(
            self.subagent_phase,
            None | Some(SubagentPhase::Queued | SubagentPhase::Spawning)
        ) {
            self.subagent_phase = Some(SubagentPhase::Running);
        }
    }

    fn derived_subagent_phase(&self) -> Option<SubagentPhase> {
        if self.background_task_terminal_phase.is_some() {
            return self.background_task_terminal_phase;
        }
        if self.detached_from_foreground && self.subagent_phase == Some(SubagentPhase::Backgrounded)
        {
            return Some(SubagentPhase::Backgrounded);
        }
        self.result.as_ref().map_or(self.subagent_phase, |result| {
            Some(if result.is_error == Some(true) {
                SubagentPhase::Failed
            } else {
                SubagentPhase::Done
            })
        })
    }

    fn subagent_elapsed_seconds(&self) -> Option<u64> {
        self.subagent_started_at_ms.map(|started| {
            self.subagent_ended_at_ms
                .unwrap_or_else(now_ms)
                .saturating_sub(started)
                / 1000
        })
    }

    fn finalize_subagent_elapsed_if_needed(&mut self) {
        if self.tool_call.name == "Agent"
            && self.subagent_started_at_ms.is_some()
            && self.subagent_ended_at_ms.is_none()
        {
            self.subagent_ended_at_ms = Some(now_ms());
        }
    }

    fn notify_snapshot_change(&mut self) {
        if let Some(listener) = &mut self.snapshot_listener {
            listener();
        }
    }
}

fn compute_latest_activity(
    ongoing: &IndexMap<String, OngoingSubCall>,
    finished: &[FinishedSubCall],
    text: &str,
    workspace_dir: Option<&str>,
) -> Option<String> {
    if let Some(call) = ongoing.values().last() {
        return Some(format_activity_line(
            "Using",
            &call.name,
            &call.args,
            workspace_dir,
        ));
    }
    if let Some(call) = finished.last() {
        return Some(format_activity_line(
            "Used",
            &call.name,
            &call.args,
            workspace_dir,
        ));
    }
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
}

fn format_activity_line(
    verb: &str,
    tool_name: &str,
    args: &Map<String, Value>,
    workspace_dir: Option<&str>,
) -> String {
    extract_key_argument(tool_name, args, workspace_dir).map_or_else(
        || format!("{verb} {tool_name}"),
        |argument| format!("{verb} {tool_name} ({argument})"),
    )
}

fn truncate_live_output(output: &mut String) {
    if output.len() <= MAX_LIVE_OUTPUT_CHARS {
        return;
    }
    let mut start = output.len() - MAX_LIVE_OUTPUT_CHARS;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    *output = format!("[...truncated]\n{}", &output[start..]);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
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
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    fn tool_call(name: &str, args: Value) -> ToolCallBlockData {
        ToolCallBlockData {
            id: "call-1".to_owned(),
            name: name.to_owned(),
            args: args.as_object().cloned().unwrap_or_default(),
            description: None,
            streaming_arguments: None,
            streaming_started_at_ms: None,
            subagent: None,
            step: None,
            turn_id: None,
            truncated: None,
        }
    }

    fn result(output: &str, is_error: bool) -> ToolResultBlockData {
        ToolResultBlockData {
            tool_call_id: "call-1".to_owned(),
            output: output.to_owned(),
            is_error: Some(is_error),
            synthetic: None,
        }
    }

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

    #[test]
    fn state_caps_progress_and_live_output_then_result_clears_both() {
        let mut state = ToolCallState::new(
            tool_call("Bash", serde_json::json!({"command": "run"})),
            None,
            None,
        );
        state.append_progress(
            &(1..=30)
                .map(|number| format!("line-{number}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        assert_eq!(state.progress_lines().len(), MAX_PROGRESS_LINES);
        assert_eq!(
            state.progress_lines().first().map(String::as_str),
            Some("line-7")
        );

        state.append_live_output(&"界".repeat(MAX_LIVE_OUTPUT_CHARS));
        assert!(state.live_output().starts_with("[...truncated]\n"));
        assert!(
            state
                .live_output()
                .is_char_boundary(state.live_output().len())
        );
        state.set_result(result("done", false));
        assert!(state.progress_lines().is_empty());
        assert!(state.live_output().is_empty());
    }

    #[test]
    fn read_snapshot_tracks_pending_done_and_failed_with_relative_paths() {
        let workspace = std::env::temp_dir().join("tool-call-workspace");
        let file = workspace.join("src").join("main.rs");
        let mut state = ToolCallState::new(
            tool_call("Read", serde_json::json!({"path": file.to_string_lossy()})),
            None,
            Some(workspace.to_string_lossy().into_owned()),
        );
        assert_eq!(
            state.get_read_snapshot(),
            ToolCallReadSnapshot {
                tool_call_id: "call-1".to_owned(),
                file_path: Some(
                    std::path::Path::new("src")
                        .join("main.rs")
                        .to_string_lossy()
                        .into_owned()
                ),
                phase: ReadPhase::Pending,
                lines: 0,
            }
        );
        state.set_result(result("one\n\ntwo\n", false));
        assert_eq!(state.get_read_snapshot().phase, ReadPhase::Done);
        assert_eq!(state.get_read_snapshot().lines, 2);
        state.set_result(result("missing", true));
        assert_eq!(state.get_read_snapshot().phase, ReadPhase::Failed);
        assert_eq!(state.get_read_snapshot().lines, 0);
    }

    #[test]
    fn subagent_lifecycle_and_background_terminal_status_drive_snapshots() {
        let mut state = ToolCallState::new(
            tool_call("Agent", serde_json::json!({"description": "inspect"})),
            None,
            None,
        );
        state.on_subagent_spawned(&SubagentSpawnMeta {
            agent_id: "agent-1".to_owned(),
            agent_name: Some("explorer".to_owned()),
            run_in_background: false,
        });
        assert_eq!(
            state.get_subagent_snapshot().phase,
            Some(SubagentPhase::Queued)
        );
        state.on_subagent_started(&SubagentSpawnMeta {
            agent_id: "agent-1".to_owned(),
            agent_name: Some("explorer".to_owned()),
            run_in_background: false,
        });
        state.append_subagent_text("working", SubagentTextKind::Thinking);
        assert_eq!(
            state.last_subagent_stream_kind(),
            SubagentTextKind::Thinking
        );
        assert_eq!(
            state.get_subagent_snapshot().phase,
            Some(SubagentPhase::Running)
        );
        assert_eq!(
            state.get_subagent_snapshot().latest_activity.as_deref(),
            Some("working")
        );

        state.mark_backgrounded();
        state.set_result(result("agent_id: agent-1", false));
        assert_eq!(
            state.get_subagent_snapshot().phase,
            Some(SubagentPhase::Backgrounded)
        );
        state.set_background_task_terminal_status(BackgroundTaskStatus::Lost, None);
        let failed = state.get_subagent_snapshot();
        assert_eq!(failed.phase, Some(SubagentPhase::Failed));
        assert_eq!(
            failed.error_text.as_deref(),
            Some("Background agent lost (session restarted before completion)")
        );
        assert_eq!(state.get_subagent_agent_id().as_deref(), Some("agent-1"));
    }

    #[test]
    fn child_tool_streaming_and_completion_update_latest_activity_and_counts() {
        let mut state = ToolCallState::new(
            tool_call("Agent", serde_json::json!({"description": "inspect"})),
            None,
            None,
        );
        state.append_sub_tool_call_delta(
            "child-1",
            Some("Read"),
            Some(r#"{"path":"src/main.rs"}"#),
        );
        state.append_sub_tool_live_output("child-1", "streaming");
        assert_eq!(state.sub_tool_live_output("child-1"), Some("streaming"));
        assert_eq!(
            state.get_subagent_snapshot().latest_activity.as_deref(),
            Some("Using Read (src/main.rs)")
        );
        state.finish_sub_tool_call(ToolResultBlockData {
            tool_call_id: "child-1".to_owned(),
            output: "content".to_owned(),
            is_error: Some(false),
            synthetic: None,
        });
        let snapshot = state.get_subagent_snapshot();
        assert_eq!(snapshot.tool_count, 1);
        assert_eq!(
            snapshot.latest_activity.as_deref(),
            Some("Used Read (src/main.rs)")
        );
        assert_eq!(
            state.latest_finished_sub_call_result(),
            Some(("content", false))
        );
    }

    #[test]
    fn replay_caps_finished_tools_and_snapshot_listener_observes_changes() {
        let calls = (1..=6)
            .map(|number| crate::tui::types::SubagentReplayToolCallData {
                id: format!("child-{number}"),
                name: "Read".to_owned(),
                args: serde_json::json!({"path": format!("file-{number}")})
                    .as_object()
                    .cloned()
                    .unwrap(),
                description: None,
                result: Some(ToolResultBlockData {
                    tool_call_id: format!("child-{number}"),
                    output: "ok".to_owned(),
                    is_error: Some(false),
                    synthetic: None,
                }),
            })
            .collect();
        let mut call = tool_call("Agent", serde_json::json!({"description": "inspect"}));
        call.subagent = Some(SubagentReplayBlockData {
            id: "agent-1".to_owned(),
            name: Some("explorer".to_owned()),
            text: Some("finished".to_owned()),
            tool_calls: Some(calls),
        });
        let mut state = ToolCallState::new(call, Some(result("done", false)), None);
        assert_eq!(state.get_subagent_snapshot().tool_count, 6);

        let changes = Arc::new(AtomicUsize::new(0));
        let listener_changes = Arc::clone(&changes);
        state.set_snapshot_listener(Some(Box::new(move || {
            listener_changes.fetch_add(1, Ordering::Relaxed);
        })));
        state.append_subagent_text("more", SubagentTextKind::Text);
        assert_eq!(changes.load(Ordering::Relaxed), 2);
    }
}
