use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AgentSwarmResultStatus {
    index: usize,
    status: AgentSwarmResultStatusKind,
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

pub fn agent_swarm_partial_items_count_from_arguments(arguments_text: &str) -> usize {
    agent_swarm_partial_items_from_arguments(arguments_text).len()
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

pub fn agent_swarm_description_from_args(args: &Map<String, Value>) -> &str {
    args.get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
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
            result.push(AgentSwarmResultStatus { index, status });
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
        result.push(AgentSwarmResultStatus { index, status });
    }
    result
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
