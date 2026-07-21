use std::{
    sync::{Arc, LazyLock},
    time::{SystemTime, UNIX_EPOCH},
};

use regex::Regex;
use serde_json::{Map, Value};

use super::agent_swarm_progress_estimator::{
    AgentSwarmProgressEstimator, AgentSwarmProgressEstimatorPhase,
};

const TEXT_CELL_PREFERRED_WIDTH: usize = 30;
const CELL_GAP_WIDTH: usize = 2;
const TEXT_BRAILLE_BAR_MIN_WIDTH: usize = 6;
const BRAILLE_BAR_MAX_WIDTH: usize = 8;
const MIN_LABEL_WIDTH: usize = 9;
const COMPACT_TERMINAL_MARK_WIDTH: usize = 1;
const AGENT_SWARM_NON_GRID_LINES: usize = 6;

static PARTIAL_ITEMS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""items"\s*:\s*\["#).expect("partial AgentSwarm items regex must compile")
});
static PARTIAL_RESUME_ITEMS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""resume_agent_ids"\s*:\s*\{"#)
        .expect("partial AgentSwarm resume items regex must compile")
});
static PARTIAL_PROMPT_TEMPLATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""prompt_template"\s*:\s*""#)
        .expect("partial AgentSwarm prompt template regex must compile")
});
static WORK_ITEMS_STARTED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""(?:items|resume_agent_ids)"\s*:"#)
        .expect("AgentSwarm work items regex must compile")
});
static SUBAGENT_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<subagent\b([^>]*)>").expect("subagent tag regex must compile"));
static XML_INDEX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bindex="([^"]*)""#).expect("subagent index attribute regex must compile")
});
static XML_OUTCOME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\boutcome="([^"]*)""#).expect("subagent outcome attribute regex must compile")
});
static LEGACY_AGENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\[agent (\d+)\]$").expect("legacy agent block regex must compile")
});
static LEGACY_STATUS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^status: (completed|failed|aborted|cancelled)$")
        .expect("legacy agent status regex must compile")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentSwarmResultStatusKind {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentSwarmResultStatus {
    index: usize,
    status: AgentSwarmResultStatusKind,
    completed_text: Option<String>,
    failure_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentSwarmResultSummary {
    pub completed: usize,
    pub failed: usize,
    pub aborted: usize,
    pub parsed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentSwarmGridLayoutInput {
    pub width: f64,
    pub height: f64,
    pub count: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentSwarmGridLayout {
    pub render_text: bool,
    pub bar_cells: usize,
    pub columns: usize,
    pub rows: usize,
    pub cell_width: usize,
    pub column_gap: usize,
    pub left_padding: usize,
}

fn javascript_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.as_f64().map_or_else(
            || value.to_string(),
            |number| {
                if number == 0.0 {
                    "0".to_owned()
                } else if number.fract() == 0.0 {
                    format!("{number:.0}")
                } else {
                    value.to_string()
                }
            },
        ),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(javascript_string)
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

// Original: agent-swarm-progress.ts agentSwarmItemsFromArgs()
pub fn agent_swarm_items_from_args(args: &Map<String, Value>) -> Vec<String> {
    args.get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(javascript_string).collect())
        .unwrap_or_default()
}

// Original: agent-swarm-progress.ts agentSwarmResumeItemsFromArgs()
fn agent_swarm_resume_items_from_args(args: &Map<String, Value>) -> Vec<String> {
    args.get("resume_agent_ids")
        .and_then(Value::as_object)
        .map(|items| vec!["(resumed)".to_owned(); items.len()])
        .unwrap_or_default()
}

pub fn agent_swarm_partial_items_count_from_arguments(arguments_text: &str) -> usize {
    agent_swarm_partial_items_from_arguments(arguments_text).len()
}

// Original: agent-swarm-progress.ts agentSwarmWorkItemsStartedFromArguments()
fn agent_swarm_work_items_started_from_arguments(arguments_text: &str) -> bool {
    WORK_ITEMS_STARTED_RE.is_match(arguments_text)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PartialJsonString {
    value: String,
    closed: bool,
    next_index: usize,
}

fn parse_partial_json_string(text: &str, start_index: usize) -> PartialJsonString {
    let mut value = String::new();
    let mut index = start_index;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character == '"' {
            return PartialJsonString {
                value,
                closed: true,
                next_index: index,
            };
        }
        if character != '\\' {
            value.push(character);
            index += character.len_utf8();
            continue;
        }
        let escape_index = index + 1;
        let Some(escaped) = text
            .get(escape_index..)
            .and_then(|tail| tail.chars().next())
        else {
            return PartialJsonString {
                value,
                closed: false,
                next_index: index,
            };
        };
        match escaped {
            'n' => value.push('\n'),
            't' => value.push('\t'),
            'r' => value.push('\r'),
            'b' => value.push('\u{8}'),
            'f' => value.push('\u{c}'),
            '"' | '\\' | '/' => value.push(escaped),
            'u' => {
                let hex_start = escape_index + 1;
                let hex_end = hex_start.saturating_add(4);
                let Some(hex) = text.get(hex_start..hex_end) else {
                    return PartialJsonString {
                        value,
                        closed: false,
                        next_index: index,
                    };
                };
                let Ok(code) = u32::from_str_radix(hex, 16) else {
                    return PartialJsonString {
                        value,
                        closed: false,
                        next_index: index,
                    };
                };
                value.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
                index = hex_end;
                continue;
            }
            other => value.push(other),
        }
        index = escape_index + escaped.len_utf8();
    }
    PartialJsonString {
        value,
        closed: false,
        next_index: text.len(),
    }
}

// Original: agent-swarm-progress.ts agentSwarmPartialItemsFromArguments()
pub fn agent_swarm_partial_items_from_arguments(arguments_text: &str) -> Vec<String> {
    let Some(found) = PARTIAL_ITEMS_RE.find(arguments_text) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    let mut index = found.end();
    while index < arguments_text.len() {
        let Some(character) = arguments_text[index..].chars().next() else {
            break;
        };
        if character == ']' {
            return items;
        }
        if character != '"' {
            index += character.len_utf8();
            continue;
        }
        let parsed = parse_partial_json_string(arguments_text, index + 1);
        items.push(parsed.value);
        if !parsed.closed {
            return items;
        }
        index = parsed.next_index + 1;
    }
    items
}

// Original: agent-swarm-progress.ts agentSwarmPartialResumeItemsFromArguments()
fn agent_swarm_partial_resume_items_from_arguments(arguments_text: &str) -> Vec<String> {
    let Some(found) = PARTIAL_RESUME_ITEMS_RE.find(arguments_text) else {
        return Vec::new();
    };
    vec!["(resumed)".to_owned(); count_partial_json_object_entries(arguments_text, found.end())]
}

pub fn agent_swarm_description_from_args(args: &Map<String, Value>) -> &str {
    args.get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

// Original: agent-swarm-progress.ts agentSwarmPromptTemplateFromArgs()
fn agent_swarm_prompt_template_from_args(args: &Map<String, Value>) -> &str {
    args.get("prompt_template")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

// Original: agent-swarm-progress.ts agentSwarmPartialPromptTemplateFromArguments()
fn agent_swarm_partial_prompt_template_from_arguments(arguments_text: &str) -> String {
    let Some(found) = PARTIAL_PROMPT_TEMPLATE_RE.find(arguments_text) else {
        return String::new();
    };
    parse_partial_json_string(arguments_text, found.end()).value
}

// Original: agent-swarm-progress.ts countPartialJsonObjectEntries()
fn count_partial_json_object_entries(text: &str, start_index: usize) -> usize {
    let mut count = 0;
    let mut expect_key = true;
    let mut index = start_index;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        match character {
            '}' => return count,
            ',' => {
                expect_key = true;
                index += 1;
            }
            '"' => {
                let parsed = parse_partial_json_string(text, index + 1);
                if expect_key {
                    if parsed.closed || !parsed.value.is_empty() {
                        count += 1;
                    }
                    expect_key = false;
                }
                if !parsed.closed {
                    return count;
                }
                index = parsed.next_index + 1;
            }
            _ => index += character.len_utf8(),
        }
    }
    count
}

// Original: agent-swarm-progress.ts agentSwarmResultSummaryFromOutput()
pub fn agent_swarm_result_summary_from_output(output: &str) -> AgentSwarmResultSummary {
    let statuses = parse_agent_swarm_result_statuses(output);
    let mut completed = 0;
    let mut failed = 0;
    let mut aborted = 0;
    for status in &statuses {
        match status.status {
            AgentSwarmResultStatusKind::Completed => completed += 1,
            AgentSwarmResultStatusKind::Failed => failed += 1,
            AgentSwarmResultStatusKind::Cancelled => aborted += 1,
        }
    }
    AgentSwarmResultSummary {
        completed,
        failed,
        aborted,
        parsed: !statuses.is_empty(),
    }
}

fn parse_agent_swarm_result_statuses(output: &str) -> Vec<AgentSwarmResultStatus> {
    let xml = parse_agent_swarm_xml_result_statuses(output);
    if xml.is_empty() {
        parse_agent_swarm_legacy_result_statuses(output)
    } else {
        xml
    }
}

fn parse_agent_swarm_xml_result_statuses(output: &str) -> Vec<AgentSwarmResultStatus> {
    let mut result = Vec::new();
    let mut cursor = 0;
    let mut tag_index = 0;
    while let Some(captures) = SUBAGENT_TAG_RE.captures_at(output, cursor) {
        let Some(opening) = captures.get(0) else {
            break;
        };
        let Some(close_offset) = output[opening.end()..].find("</subagent>") else {
            break;
        };
        tag_index += 1;
        let attrs = captures.get(1).map_or("", |value| value.as_str());
        let index = XML_INDEX_RE
            .captures(attrs)
            .and_then(|captures| captures.get(1))
            .and_then(|value| value.as_str().parse::<usize>().ok())
            .filter(|index| *index > 0)
            .unwrap_or(tag_index);
        let status = XML_OUTCOME_RE
            .captures(attrs)
            .and_then(|captures| captures.get(1))
            .and_then(|value| match value.as_str() {
                "completed" => Some(AgentSwarmResultStatusKind::Completed),
                "failed" => Some(AgentSwarmResultStatusKind::Failed),
                "aborted" | "cancelled" => Some(AgentSwarmResultStatusKind::Cancelled),
                _ => None,
            });
        if let Some(status) = status {
            let body_start = opening.end();
            let body_end = body_start + close_offset;
            let body = &output[body_start..body_end];
            result.push(AgentSwarmResultStatus {
                index,
                completed_text: (status == AgentSwarmResultStatusKind::Completed)
                    .then(|| body.to_owned()),
                failure_text: (status == AgentSwarmResultStatusKind::Failed)
                    .then(|| body.to_owned()),
                status,
            });
        }
        cursor = opening.end() + close_offset + "</subagent>".len();
    }
    result
}

fn parse_agent_swarm_legacy_result_statuses(output: &str) -> Vec<AgentSwarmResultStatus> {
    let headers = LEGACY_AGENT_RE.captures_iter(output).collect::<Vec<_>>();
    let mut result = Vec::new();
    for (position, captures) in headers.iter().enumerate() {
        let Some(header) = captures.get(0) else {
            continue;
        };
        let end = headers
            .get(position + 1)
            .and_then(|next| next.get(0))
            .map_or(output.len(), |next| next.start());
        let block = &output[header.start()..end];
        let Some(status) = LEGACY_STATUS_RE
            .captures(block)
            .and_then(|captures| captures.get(1))
            .and_then(|value| match value.as_str() {
                "completed" => Some(AgentSwarmResultStatusKind::Completed),
                "failed" => Some(AgentSwarmResultStatusKind::Failed),
                "aborted" | "cancelled" => Some(AgentSwarmResultStatusKind::Cancelled),
                _ => None,
            })
        else {
            continue;
        };
        let Some(index) = captures
            .get(1)
            .and_then(|value| value.as_str().parse::<usize>().ok())
        else {
            continue;
        };
        result.push(AgentSwarmResultStatus {
            index,
            completed_text: (status == AgentSwarmResultStatusKind::Completed)
                .then(|| parse_agent_swarm_completed_text(block))
                .flatten(),
            failure_text: (status == AgentSwarmResultStatusKind::Failed)
                .then(|| parse_agent_swarm_failure_text(block))
                .flatten(),
            status,
        });
    }
    result
}

fn parse_agent_swarm_completed_text(block: &str) -> Option<String> {
    let marker = "\n[summary]\n";
    let marker_index = block.find(marker)?;
    normalize_final_output_text(&block[marker_index + marker.len()..])
}

fn parse_agent_swarm_failure_text(block: &str) -> Option<String> {
    let marker = "subagent error:";
    let start = block.find(marker)? + marker.len();
    normalize_failure_text(&block[start..])
}

#[derive(Clone, Default)]
pub struct AgentSwarmProgressOptions {
    pub description: String,
    pub request_render: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSwarmMemberSnapshot {
    pub id: String,
    pub agent_id: Option<String>,
    pub phase: AgentSwarmProgressEstimatorPhase,
    pub ticks: usize,
    pub item_text: String,
    pub latest_model_text: String,
    pub completed_text: Option<String>,
    pub failure_text: Option<String>,
    pub cancelled_label_text: Option<String>,
    pub suspended_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct AgentSwarmMember {
    id: String,
    agent_id: Option<String>,
    phase: AgentSwarmProgressEstimatorPhase,
    ticks: usize,
    item_text: String,
    latest_model_text: String,
    completed_text: Option<String>,
    failure_text: Option<String>,
    cancelled_label_text: Option<String>,
    suspended_reason: Option<String>,
    completed_at_ms: Option<f64>,
    failed_at_ms: Option<f64>,
}

/// Stateful AgentSwarm event model.
///
/// Original: `agent-swarm-progress.ts`, `AgentSwarmProgressComponent` state
/// transition methods. Rendering is implemented separately so event replay can
/// be tested independently from terminal styling.
pub struct AgentSwarmProgressComponent {
    members: Vec<AgentSwarmMember>,
    progress_estimator: AgentSwarmProgressEstimator,
    description: String,
    request_render: Option<Arc<dyn Fn() + Send + Sync>>,
    input_complete: bool,
    failed: bool,
    aborted: bool,
    items_started: bool,
    tool_call_active: bool,
    prompt_template_text: String,
    activity_spinner_text: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

impl AgentSwarmProgressComponent {
    pub fn new(options: AgentSwarmProgressOptions) -> Self {
        Self {
            members: Vec::new(),
            progress_estimator: AgentSwarmProgressEstimator::default(),
            description: options.description,
            request_render: options.request_render,
            input_complete: false,
            failed: false,
            aborted: false,
            items_started: false,
            tool_call_active: true,
            prompt_template_text: String::new(),
            activity_spinner_text: None,
        }
    }

    pub fn set_activity_spinner_text(
        &mut self,
        provider: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    ) {
        if self.tool_call_active {
            self.activity_spinner_text = provider;
            self.changed();
        }
    }

    pub fn activity_spinner_text(&self) -> Option<String> {
        self.activity_spinner_text
            .as_ref()
            .map(|provider| provider())
    }

    pub fn mark_tool_call_ended(&mut self) {
        self.tool_call_active = false;
        self.activity_spinner_text = None;
        self.changed();
    }

    pub fn is_tool_call_active(&self) -> bool {
        self.tool_call_active
    }

    pub fn is_request_streaming(&self) -> bool {
        !self.input_complete
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn prompt_template_text(&self) -> &str {
        &self.prompt_template_text
    }

    pub fn items_started(&self) -> bool {
        self.items_started
    }

    pub fn is_failed(&self) -> bool {
        self.failed
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    pub fn member_snapshots(&self) -> Vec<AgentSwarmMemberSnapshot> {
        self.members
            .iter()
            .map(|member| AgentSwarmMemberSnapshot {
                id: member.id.clone(),
                agent_id: member.agent_id.clone(),
                phase: member.phase,
                ticks: member.ticks,
                item_text: member.item_text.clone(),
                latest_model_text: member.latest_model_text.clone(),
                completed_text: member.completed_text.clone(),
                failure_text: member.failure_text.clone(),
                cancelled_label_text: member.cancelled_label_text.clone(),
                suspended_reason: member.suspended_reason.clone(),
            })
            .collect()
    }

    // Original: AgentSwarmProgressComponent.updateArgs()
    pub fn update_args(&mut self, args: &Map<String, Value>, streaming_arguments: Option<&str>) {
        let description = agent_swarm_description_from_args(args);
        if !description.is_empty() || self.description.is_empty() {
            self.description = description.to_owned();
        }

        let mut full_rows = agent_swarm_resume_items_from_args(args);
        full_rows.extend(agent_swarm_items_from_args(args));
        let mut partial_rows = Vec::new();
        if let Some(arguments) = streaming_arguments {
            partial_rows.extend(agent_swarm_partial_resume_items_from_arguments(arguments));
            partial_rows.extend(agent_swarm_partial_items_from_arguments(arguments));
        }
        if !full_rows.is_empty()
            || !partial_rows.is_empty()
            || streaming_arguments.is_some_and(agent_swarm_work_items_started_from_arguments)
        {
            self.items_started = true;
        }

        let full_prompt_template = agent_swarm_prompt_template_from_args(args);
        let partial_prompt_template = streaming_arguments
            .map(agent_swarm_partial_prompt_template_from_arguments)
            .unwrap_or_default();
        let prompt_template = if full_prompt_template.is_empty() {
            partial_prompt_template.as_str()
        } else {
            full_prompt_template
        };
        if !prompt_template.is_empty() || self.prompt_template_text.is_empty() {
            self.prompt_template_text = prompt_template.to_owned();
        }

        let item_count = full_rows.len().max(partial_rows.len());
        if item_count > 0 {
            self.ensure_member_count(item_count);
        }
        self.update_item_texts(&full_rows, &partial_rows);
        self.changed();
    }

    pub fn mark_input_complete(&mut self) {
        if !self.input_complete {
            self.input_complete = true;
            for member in &mut self.members {
                if member.phase == AgentSwarmProgressEstimatorPhase::Pending {
                    member.phase = AgentSwarmProgressEstimatorPhase::Queued;
                }
            }
            self.changed();
        }
    }

    pub fn register_subagent(
        &mut self,
        agent_id: &str,
        swarm_index: Option<usize>,
        _description: Option<&str>,
    ) {
        let index = self.find_member_for_subagent(agent_id, swarm_index);
        let member = &mut self.members[index];
        member.agent_id = Some(agent_id.to_owned());
        if member.phase == AgentSwarmProgressEstimatorPhase::Pending {
            member.phase = AgentSwarmProgressEstimatorPhase::Queued;
        }
        self.changed();
    }

    pub fn mark_started(&mut self, agent_id: &str) {
        let Some(index) = self.find_member_by_agent_id(agent_id) else {
            return;
        };
        let now_ms = now_ms();
        let member_id = self.members[index].id.clone();
        self.progress_estimator.mark_started(&member_id, now_ms);
        self.members[index].ticks = self.members[index].ticks.max(1);
        self.promote_to_running(index, Some(now_ms), false);
        self.changed();
    }

    pub fn record_tool_call(&mut self, agent_id: &str, tool_call_id: &str) {
        let Some(index) = self.find_member_by_agent_id(agent_id) else {
            return;
        };
        let member_id = self.members[index].id.clone();
        let result = self
            .progress_estimator
            .record_tool_call(&member_id, tool_call_id, now_ms());
        if !result.accepted {
            return;
        }
        self.members[index].ticks = result.raw_ticks;
        self.promote_to_running(index, None, false);
        self.changed();
    }

    pub fn append_model_delta(&mut self, agent_id: &str, delta: &str) {
        let Some(index) = self.find_member_by_agent_id(agent_id) else {
            return;
        };
        if delta.is_empty() {
            return;
        }
        self.members[index].latest_model_text.push_str(delta);
        trim_to_last_chars(&mut self.members[index].latest_model_text, 2_000);
        self.promote_to_running(index, Some(now_ms()), true);
        self.changed();
    }

    pub fn mark_completed(&mut self, agent_id: &str, completed_text: Option<&str>) {
        let Some(index) = self.find_member_by_agent_id(agent_id) else {
            return;
        };
        if matches!(
            self.members[index].phase,
            AgentSwarmProgressEstimatorPhase::Failed | AgentSwarmProgressEstimatorPhase::Cancelled
        ) {
            return;
        }
        self.complete_member(index, now_ms(), completed_text);
        self.changed();
    }

    pub fn mark_suspended(
        &mut self,
        agent_id: &str,
        _reason: &str,
        swarm_index: Option<usize>,
        _description: Option<&str>,
    ) {
        let index = self
            .find_member_by_agent_id(agent_id)
            .unwrap_or_else(|| self.find_member_for_subagent(agent_id, swarm_index));
        if matches!(
            self.members[index].phase,
            AgentSwarmProgressEstimatorPhase::Completed
                | AgentSwarmProgressEstimatorPhase::Cancelled
        ) {
            return;
        }
        let member_id = self.members[index].id.clone();
        self.members[index].agent_id = Some(agent_id.to_owned());
        self.progress_estimator.mark_queued(&member_id, now_ms());
        let member = &mut self.members[index];
        member.phase = AgentSwarmProgressEstimatorPhase::Suspended;
        clear_terminal_state(member);
        self.changed();
    }

    pub fn mark_failed(&mut self, agent_id: &str, failure_text: Option<&str>) {
        let Some(index) = self.find_member_by_agent_id(agent_id) else {
            return;
        };
        self.fail_member(index, now_ms(), failure_text);
        self.changed();
    }

    pub fn mark_swarm_failed(&mut self, failure_text: Option<&str>) {
        self.failed = true;
        self.aborted = false;
        let now_ms = now_ms();
        for index in 0..self.members.len() {
            if !is_terminal_phase(self.members[index].phase) {
                self.fail_member(index, now_ms, failure_text);
            }
        }
        self.changed();
    }

    pub fn mark_cancelled(&mut self, agent_id: &str) {
        let Some(index) = self.find_member_by_agent_id(agent_id) else {
            return;
        };
        self.cancel_member(index, now_ms());
        self.changed();
    }

    pub fn mark_active_cancelled(&mut self) {
        self.aborted = true;
        let now_ms = now_ms();
        for index in 0..self.members.len() {
            if !is_terminal_phase(self.members[index].phase) {
                self.cancel_member(index, now_ms);
            }
        }
        self.changed();
    }

    pub fn apply_result(&mut self, output: &str) -> bool {
        let statuses = parse_agent_swarm_result_statuses(output);
        if statuses.is_empty() {
            return false;
        }
        self.aborted = false;
        let now_ms = now_ms();
        for entry in statuses {
            self.ensure_member_count(entry.index);
            let index = entry.index - 1;
            match entry.status {
                AgentSwarmResultStatusKind::Completed => {
                    self.complete_member(index, now_ms, entry.completed_text.as_deref());
                }
                AgentSwarmResultStatusKind::Failed => {
                    self.fail_member(index, now_ms, entry.failure_text.as_deref());
                }
                AgentSwarmResultStatusKind::Cancelled => self.cancel_member(index, now_ms),
            }
        }
        self.changed();
        true
    }

    fn changed(&self) {
        if let Some(request_render) = &self.request_render {
            request_render();
        }
    }

    fn find_member_for_subagent(&mut self, agent_id: &str, swarm_index: Option<usize>) -> usize {
        if let Some(index) = self.find_member_by_agent_id(agent_id) {
            return index;
        }
        if let Some(index) = swarm_index.filter(|index| *index > 0) {
            self.ensure_member_count(index);
            return index - 1;
        }
        if let Some(index) = self
            .members
            .iter()
            .position(|member| member.agent_id.is_none())
        {
            return index;
        }
        let index = self.members.len();
        self.ensure_member_count(index + 1);
        index
    }

    fn find_member_by_agent_id(&self, agent_id: &str) -> Option<usize> {
        self.members
            .iter()
            .position(|member| member.agent_id.as_deref() == Some(agent_id))
    }

    fn ensure_member_count(&mut self, count: usize) {
        if count <= self.members.len() {
            return;
        }
        let phase = if self.input_complete {
            AgentSwarmProgressEstimatorPhase::Queued
        } else {
            AgentSwarmProgressEstimatorPhase::Pending
        };
        let now_ms = now_ms();
        for index in self.members.len()..count {
            let id = format!("{:03}", index + 1);
            self.progress_estimator.ensure_member(&id, now_ms);
            self.members.push(AgentSwarmMember {
                id,
                agent_id: None,
                phase,
                ticks: 0,
                item_text: String::new(),
                latest_model_text: String::new(),
                completed_text: None,
                failure_text: None,
                cancelled_label_text: None,
                suspended_reason: None,
                completed_at_ms: None,
                failed_at_ms: None,
            });
        }
    }

    fn update_item_texts(&mut self, full_items: &[String], partial_items: &[String]) {
        let count = full_items
            .len()
            .max(partial_items.len())
            .max(self.members.len());
        for index in 0..count {
            let Some(member) = self.members.get_mut(index) else {
                continue;
            };
            if let Some(item_text) = full_items.get(index).or_else(|| partial_items.get(index)) {
                member.item_text.clone_from(item_text);
            }
        }
    }

    fn promote_to_running(&mut self, index: usize, now_ms: Option<f64>, set_ticks: bool) {
        if matches!(
            self.members[index].phase,
            AgentSwarmProgressEstimatorPhase::Pending
                | AgentSwarmProgressEstimatorPhase::Queued
                | AgentSwarmProgressEstimatorPhase::Suspended
        ) {
            self.members[index].phase = AgentSwarmProgressEstimatorPhase::Running;
            if let Some(now_ms) = now_ms {
                let member_id = self.members[index].id.clone();
                self.progress_estimator.mark_started(&member_id, now_ms);
            }
            if set_ticks {
                self.members[index].ticks = self.members[index].ticks.max(1);
            }
        }
        self.members[index].suspended_reason = None;
    }

    fn complete_member(&mut self, index: usize, now_ms: f64, completed_text: Option<&str>) {
        let member_id = self.members[index].id.clone();
        if self.members[index].phase != AgentSwarmProgressEstimatorPhase::Completed {
            self.progress_estimator.mark_completed(&member_id, now_ms);
            self.members[index].completed_at_ms = Some(now_ms);
        }
        if let Some(text) = completed_text.and_then(normalize_final_output_text) {
            self.members[index].completed_text = Some(text);
        }
        let member = &mut self.members[index];
        member.phase = AgentSwarmProgressEstimatorPhase::Completed;
        member.failed_at_ms = None;
        member.failure_text = None;
        member.cancelled_label_text = None;
        member.suspended_reason = None;
    }

    fn fail_member(&mut self, index: usize, now_ms: f64, failure_text: Option<&str>) {
        let member_id = self.members[index].id.clone();
        if self.members[index].phase != AgentSwarmProgressEstimatorPhase::Failed {
            self.progress_estimator.mark_failed(&member_id, now_ms);
            self.members[index].failed_at_ms = Some(now_ms);
        }
        if let Some(text) = failure_text.and_then(normalize_failure_text) {
            self.members[index].failure_text = Some(text);
        }
        let member = &mut self.members[index];
        member.phase = AgentSwarmProgressEstimatorPhase::Failed;
        member.completed_at_ms = None;
        member.completed_text = None;
        member.cancelled_label_text = None;
        member.suspended_reason = None;
    }

    fn cancel_member(&mut self, index: usize, now_ms: f64) {
        let previous_phase = self.members[index].phase;
        let member_id = self.members[index].id.clone();
        self.progress_estimator.mark_cancelled(&member_id, now_ms);
        let running_label = running_cell_label_text(&self.members[index]);
        let member = &mut self.members[index];
        member.phase = AgentSwarmProgressEstimatorPhase::Cancelled;
        member.completed_at_ms = None;
        member.completed_text = None;
        member.failed_at_ms = None;
        member.failure_text = None;
        member.suspended_reason = None;
        member.cancelled_label_text = Some(match previous_phase {
            AgentSwarmProgressEstimatorPhase::Pending
            | AgentSwarmProgressEstimatorPhase::Queued
            | AgentSwarmProgressEstimatorPhase::Suspended => "Cancelled.".to_owned(),
            AgentSwarmProgressEstimatorPhase::Running => running_label,
            AgentSwarmProgressEstimatorPhase::Completed
            | AgentSwarmProgressEstimatorPhase::Failed
            | AgentSwarmProgressEstimatorPhase::Cancelled => "Aborted.".to_owned(),
        });
    }
}

fn clear_terminal_state(member: &mut AgentSwarmMember) {
    member.completed_at_ms = None;
    member.completed_text = None;
    member.failed_at_ms = None;
    member.failure_text = None;
    member.cancelled_label_text = None;
    member.suspended_reason = None;
}

fn is_terminal_phase(phase: AgentSwarmProgressEstimatorPhase) -> bool {
    matches!(
        phase,
        AgentSwarmProgressEstimatorPhase::Completed
            | AgentSwarmProgressEstimatorPhase::Failed
            | AgentSwarmProgressEstimatorPhase::Cancelled
    )
}

fn running_cell_label_text(member: &AgentSwarmMember) -> String {
    let latest = latest_non_empty_line(&member.latest_model_text);
    if latest.is_empty() {
        "Running".to_owned()
    } else {
        latest
    }
}

fn latest_non_empty_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(collapse_whitespace)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_final_output_text(text: &str) -> Option<String> {
    let normalized = collapse_whitespace(text);
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_failure_text(text: &str) -> Option<String> {
    let nested = nested_agent_swarm_failure_text(text).unwrap_or_else(|| text.to_owned());
    let normalized = collapse_whitespace(&nested);
    let stripped = strip_agent_swarm_prefix(&normalized);
    (!stripped.is_empty()).then_some(stripped)
}

fn nested_agent_swarm_failure_text(text: &str) -> Option<String> {
    if text.contains("<agent_swarm_result")
        && let Some(failure) = parse_agent_swarm_xml_result_statuses(text)
            .into_iter()
            .find_map(|entry| entry.failure_text)
    {
        return nested_agent_swarm_failure_text(&failure).or(Some(failure));
    }
    let lower = text.trim_start().to_ascii_lowercase();
    if !lower.starts_with("agent_swarm: failed") {
        return None;
    }
    let marker = "subagent error:";
    let marker_start = lower.find(marker)? + marker.len();
    let remaining = &text[marker_start..];
    let end = remaining.find("\n[agent ").unwrap_or(remaining.len());
    let failure = remaining[..end].trim().to_owned();
    nested_agent_swarm_failure_text(&failure).or(Some(failure))
}

fn strip_agent_swarm_prefix(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("agent_swarm:") {
        let offset = text.len() - rest.len();
        let tail = text[offset..].trim_start();
        for status in ["failed", "completed"] {
            if tail.to_ascii_lowercase().starts_with(status) {
                return tail[status.len()..].trim().to_owned();
            }
        }
        return tail.trim().to_owned();
    }
    text.trim().to_owned()
}

fn trim_to_last_chars(text: &mut String, max_chars: usize) {
    let count = text.chars().count();
    if count <= max_chars {
        return;
    }
    let start = text
        .char_indices()
        .nth(count - max_chars)
        .map_or(0, |(index, _)| index);
    text.drain(..start);
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64() * 1_000.0)
}

fn text_grid_layout(
    columns: usize,
    rows: usize,
    cell_width: usize,
    gap_width: usize,
    id_width: usize,
) -> AgentSwarmGridLayout {
    AgentSwarmGridLayout {
        render_text: true,
        bar_cells: bar_cells_for_text_cell_width(cell_width, id_width),
        columns,
        rows,
        cell_width,
        column_gap: gap_width,
        left_padding: 0,
    }
}

// Original: agent-swarm-progress.ts calculateAgentSwarmGridLayout()
pub fn calculate_agent_swarm_grid_layout(input: AgentSwarmGridLayoutInput) -> AgentSwarmGridLayout {
    let count = nonnegative_floor(input.count);
    let width = nonnegative_floor(input.width);
    let height = nonnegative_floor(input.height);
    let id_width = agent_swarm_grid_id_width(count);
    if count == 0 {
        return AgentSwarmGridLayout {
            render_text: true,
            bar_cells: 1,
            columns: 0,
            rows: 0,
            cell_width: 0,
            column_gap: 0,
            left_padding: 0,
        };
    }

    let text_columns =
        columns_for_cell_width(width, count, TEXT_CELL_PREFERRED_WIDTH, CELL_GAP_WIDTH);
    let text_rows = rows_for_columns(count, text_columns);
    let text_cell_width = grid_cell_width(width, text_columns, CELL_GAP_WIDTH);
    if text_rows <= height && text_cell_width >= min_text_cell_width(id_width) {
        return text_grid_layout(
            text_columns,
            text_rows,
            text_cell_width,
            CELL_GAP_WIDTH,
            id_width,
        );
    }
    let target_text_columns = if height == 0 {
        count
    } else {
        count.min(count.div_ceil(height))
    };
    let target_text_cell_width = grid_cell_width(width, target_text_columns, CELL_GAP_WIDTH);
    let target_text_rows = rows_for_columns(count, target_text_columns);
    if height > 0
        && target_text_rows <= height
        && target_text_cell_width >= min_text_cell_width(id_width)
    {
        return text_grid_layout(
            target_text_columns,
            target_text_rows,
            target_text_cell_width,
            CELL_GAP_WIDTH,
            id_width,
        );
    }

    let compact_columns =
        compact_columns_for_layout(width, count, height, id_width, CELL_GAP_WIDTH);
    let cell_budget = grid_cell_width(width, compact_columns, CELL_GAP_WIDTH);
    let bar_cells = compact_bar_cells_for_cell_width(cell_budget, id_width);
    AgentSwarmGridLayout {
        render_text: false,
        bar_cells,
        columns: compact_columns,
        rows: rows_for_columns(count, compact_columns),
        cell_width: compact_cell_width(id_width, bar_cells),
        column_gap: CELL_GAP_WIDTH,
        left_padding: 0,
    }
}

pub fn agent_swarm_grid_height_for_terminal_rows(
    rows: Option<f64>,
    following_rows: f64,
) -> Option<usize> {
    let rows = rows.filter(|rows| rows.is_finite())?;
    let rows_after_swarm = if following_rows.is_finite() {
        nonnegative_floor(following_rows)
    } else {
        0
    };
    Some(
        nonnegative_floor(rows)
            .saturating_sub(rows_after_swarm)
            .saturating_sub(AGENT_SWARM_NON_GRID_LINES),
    )
}

fn nonnegative_floor(value: f64) -> usize {
    if value.is_finite() && value > 0.0 {
        value.floor() as usize
    } else {
        0
    }
}

fn agent_swarm_grid_id_width(count: usize) -> usize {
    count.max(1).to_string().len().max(3)
}

fn columns_for_cell_width(
    width: usize,
    count: usize,
    cell_width: usize,
    gap_width: usize,
) -> usize {
    if count <= 1 {
        return count;
    }
    ((width + gap_width) / (cell_width.max(1) + gap_width)).clamp(1, count)
}

fn rows_for_columns(count: usize, columns: usize) -> usize {
    if count == 0 {
        0
    } else {
        count.div_ceil(columns.max(1))
    }
}

fn grid_cell_width(width: usize, columns: usize, gap_width: usize) -> usize {
    if columns == 0 {
        0
    } else {
        width
            .saturating_sub(gap_width.saturating_mul(columns.saturating_sub(1)))
            .checked_div(columns)
            .unwrap_or(0)
            .max(1)
    }
}

fn min_text_cell_width(id_width: usize) -> usize {
    id_width + TEXT_BRAILLE_BAR_MIN_WIDTH + 4 + MIN_LABEL_WIDTH
}

fn bar_cells_for_text_cell_width(cell_width: usize, id_width: usize) -> usize {
    let fixed_width = id_width + 1 + 2 + 1 + MIN_LABEL_WIDTH;
    let available = cell_width.saturating_sub(fixed_width);
    if available >= TEXT_BRAILLE_BAR_MIN_WIDTH {
        available.min(BRAILLE_BAR_MAX_WIDTH)
    } else {
        TEXT_BRAILLE_BAR_MIN_WIDTH
    }
}

fn compact_columns_for_layout(
    width: usize,
    count: usize,
    height: usize,
    id_width: usize,
    gap_width: usize,
) -> usize {
    let max_columns =
        columns_for_cell_width(width, count, compact_cell_width(id_width, 1), gap_width);
    if height == 0 {
        return max_columns;
    }
    let target_columns = count.min(count.div_ceil(height));
    target_columns.min(max_columns).max(1)
}

fn compact_bar_cells_for_cell_width(cell_width: usize, id_width: usize) -> usize {
    cell_width
        .saturating_sub(compact_fixed_width(id_width))
        .saturating_sub(COMPACT_TERMINAL_MARK_WIDTH)
        .max(1)
}

fn compact_cell_width(id_width: usize, bar_cells: usize) -> usize {
    compact_fixed_width(id_width) + bar_cells.max(1) + COMPACT_TERMINAL_MARK_WIDTH
}

fn compact_fixed_width(id_width: usize) -> usize {
    id_width + 1 + 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_description_items_and_partial_streaming_items() {
        let args = serde_json::json!({
            "description": "Review changed files",
            "items": ["src/a.ts", 123]
        });
        let args = args.as_object().expect("object");
        assert_eq!(
            agent_swarm_description_from_args(args),
            "Review changed files"
        );
        assert_eq!(agent_swarm_items_from_args(args), ["src/a.ts", "123"]);

        let partial = r#"{"items":["src/a.ts","src/\"b.ts","src/c"#;
        assert_eq!(agent_swarm_partial_items_count_from_arguments(partial), 3);
        assert_eq!(
            agent_swarm_partial_items_from_arguments(partial),
            ["src/a.ts", "src/\"b.ts", "src/c"]
        );
    }

    #[test]
    fn extracts_resume_items_and_partial_object_keys() {
        let args = serde_json::json!({
            "resume_agent_ids": {"first": "agent-1", "second": "agent-2"}
        });
        assert_eq!(
            agent_swarm_resume_items_from_args(args.as_object().expect("object")),
            ["(resumed)", "(resumed)"]
        );
        assert_eq!(
            agent_swarm_partial_resume_items_from_arguments(
                r#"{"resume_agent_ids":{"first":"agent-1","sec"#
            ),
            ["(resumed)", "(resumed)"]
        );
        assert!(agent_swarm_work_items_started_from_arguments(
            r#"{"resume_agent_ids": "#
        ));
        assert!(!agent_swarm_work_items_started_from_arguments(
            r#"{"description":"resume_agent_ids"}"#
        ));
    }

    #[test]
    fn extracts_complete_and_streaming_prompt_templates() {
        let args = serde_json::json!({"prompt_template": "Review {item}"});
        assert_eq!(
            agent_swarm_prompt_template_from_args(args.as_object().expect("object")),
            "Review {item}"
        );
        assert_eq!(
            agent_swarm_partial_prompt_template_from_arguments(
                r#"{"prompt_template":"Review\n{ite"#
            ),
            "Review\n{ite"
        );
        assert_eq!(
            agent_swarm_partial_prompt_template_from_arguments(r#"{"description":"Review"}"#),
            ""
        );
    }

    fn new_progress() -> AgentSwarmProgressComponent {
        AgentSwarmProgressComponent::new(AgentSwarmProgressOptions {
            description: "Review files".to_owned(),
            request_render: None,
        })
    }

    #[test]
    fn component_streams_items_then_registers_and_runs_members() {
        let mut progress = new_progress();
        progress.update_args(
            serde_json::json!({}).as_object().expect("object"),
            Some(r#"{"prompt_template":"Review {item}","items":["one","tw"#),
        );
        assert!(progress.is_request_streaming());
        assert!(progress.items_started());
        assert_eq!(progress.prompt_template_text(), "Review {item}");
        assert_eq!(
            progress
                .member_snapshots()
                .into_iter()
                .map(|member| (member.item_text, member.phase))
                .collect::<Vec<_>>(),
            [
                ("one".to_owned(), AgentSwarmProgressEstimatorPhase::Pending),
                ("tw".to_owned(), AgentSwarmProgressEstimatorPhase::Pending),
            ]
        );

        progress.mark_input_complete();
        progress.register_subagent("agent-2", Some(2), None);
        progress.mark_started("agent-2");
        progress.record_tool_call("agent-2", "read-1");
        progress.record_tool_call("agent-2", "read-1");
        let members = progress.member_snapshots();
        assert_eq!(members[0].phase, AgentSwarmProgressEstimatorPhase::Queued);
        assert_eq!(members[1].agent_id.as_deref(), Some("agent-2"));
        assert_eq!(members[1].phase, AgentSwarmProgressEstimatorPhase::Running);
        assert_eq!(members[1].ticks, 2);
    }

    #[test]
    fn component_preserves_suspension_terminal_and_cancellation_transitions() {
        let mut progress = new_progress();
        let args = serde_json::json!({"items": ["one", "two", "three"]});
        progress.update_args(args.as_object().expect("object"), None);
        progress.mark_input_complete();
        progress.register_subagent("one", Some(1), None);
        progress.register_subagent("two", Some(2), None);
        progress.register_subagent("three", Some(3), None);

        progress.mark_suspended("one", "rate limit", None, None);
        assert_eq!(
            progress.member_snapshots()[0].phase,
            AgentSwarmProgressEstimatorPhase::Suspended
        );
        progress.mark_started("one");
        progress.append_model_delta("one", "Inspecting\nlatest work");
        progress.mark_cancelled("one");
        progress.mark_completed("two", Some("  completed   summary "));
        progress.mark_failed("three", Some("agent_swarm: failed disk full"));
        let members = progress.member_snapshots();
        assert_eq!(
            members[0].phase,
            AgentSwarmProgressEstimatorPhase::Cancelled
        );
        assert_eq!(
            members[0].cancelled_label_text.as_deref(),
            Some("latest work")
        );
        assert_eq!(
            members[1].completed_text.as_deref(),
            Some("completed summary")
        );
        assert_eq!(members[2].failure_text.as_deref(), Some("disk full"));
    }

    #[test]
    fn component_applies_xml_and_legacy_results_with_output_text() {
        let mut progress = new_progress();
        assert!(progress.apply_result(concat!(
            "<agent_swarm_result>",
            "<subagent outcome=\"completed\">first summary</subagent>",
            "<subagent outcome=\"failed\">agent_swarm: failed nested error</subagent>",
            "<subagent outcome=\"aborted\"></subagent>",
            "</agent_swarm_result>"
        )));
        let members = progress.member_snapshots();
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].completed_text.as_deref(), Some("first summary"));
        assert_eq!(members[1].failure_text.as_deref(), Some("nested error"));
        assert_eq!(
            members[2].phase,
            AgentSwarmProgressEstimatorPhase::Cancelled
        );

        let mut legacy = new_progress();
        assert!(legacy.apply_result(
            "[agent 1]\nstatus: completed\n\n[summary]\nlegacy summary\n\n[agent 2]\nstatus: failed\nsubagent error: legacy failure"
        ));
        let members = legacy.member_snapshots();
        assert_eq!(members[0].completed_text.as_deref(), Some("legacy summary"));
        assert_eq!(members[1].failure_text.as_deref(), Some("legacy failure"));
        assert!(!legacy.apply_result("not a swarm result"));
    }

    #[test]
    fn summarizes_xml_and_legacy_swarm_results() {
        let xml = concat!(
            "<subagent index=\"1\" outcome=\"completed\">done</subagent>",
            "<subagent index=\"2\" outcome=\"failed\">boom</subagent>",
            "<subagent index=\"3\" outcome=\"aborted\">stop</subagent>"
        );
        assert_eq!(
            agent_swarm_result_summary_from_output(xml),
            AgentSwarmResultSummary {
                completed: 1,
                failed: 1,
                aborted: 1,
                parsed: true
            }
        );
        let legacy = "[agent 1]\nstatus: completed\n\n[agent 2]\nstatus: cancelled\n";
        assert_eq!(
            agent_swarm_result_summary_from_output(legacy),
            AgentSwarmResultSummary {
                completed: 1,
                failed: 0,
                aborted: 1,
                parsed: true
            }
        );
        assert!(!agent_swarm_result_summary_from_output("unparsed").parsed);
    }

    #[test]
    fn calculates_text_and_compact_grid_layouts() {
        let text = calculate_agent_swarm_grid_layout(AgentSwarmGridLayoutInput {
            width: 100.0,
            height: 3.0,
            count: 9.0,
        });
        assert!(text.render_text);
        assert_eq!((text.columns, text.rows), (3, 3));
        assert!(text.bar_cells >= 6);
        assert!(text.cell_width >= 22);

        let wider = calculate_agent_swarm_grid_layout(AgentSwarmGridLayoutInput {
            width: 120.0,
            height: 4.0,
            count: 20.0,
        });
        assert!(wider.render_text);
        assert_eq!((wider.columns, wider.rows), (5, 4));
        let narrower = calculate_agent_swarm_grid_layout(AgentSwarmGridLayoutInput {
            width: 117.0,
            height: 4.0,
            count: 20.0,
        });
        assert!(!narrower.render_text);
        assert_eq!((narrower.columns, narrower.rows), (5, 4));
        assert!(narrower.bar_cells > wider.bar_cells);

        let tight = calculate_agent_swarm_grid_layout(AgentSwarmGridLayoutInput {
            width: 100.0,
            height: 4.0,
            count: 40.0,
        });
        assert!(!tight.render_text);
        assert_eq!((tight.columns, tight.rows, tight.bar_cells), (10, 4, 1));
    }

    #[test]
    fn derives_available_grid_height() {
        assert_eq!(agent_swarm_grid_height_for_terminal_rows(None, 0.0), None);
        assert_eq!(
            agent_swarm_grid_height_for_terminal_rows(Some(10.0), 0.0),
            Some(4)
        );
        assert_eq!(
            agent_swarm_grid_height_for_terminal_rows(Some(20.0), 5.0),
            Some(9)
        );
        assert_eq!(
            agent_swarm_grid_height_for_terminal_rows(Some(4.0), 0.0),
            Some(0)
        );
        assert_eq!(
            agent_swarm_grid_height_for_terminal_rows(Some(f64::NAN), 0.0),
            None
        );
    }
}
