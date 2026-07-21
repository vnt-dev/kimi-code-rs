use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::tui::{
    components::media::diff_preview::{DiffLineKind, compute_diff_lines},
    types::{ToolCallBlockData, ToolResultBlockData},
};

use super::{goal::goal_status_chip, media::read_media_chip, types::str_arg};

static WEB_RESULT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:\d+\.|[-*])\s+").expect("web result line regex must compile")
});

pub type ChipProvider = fn(&ToolCallBlockData, &ToolResultBlockData) -> String;

pub fn count_non_empty_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.split('\n').filter(|line| !line.is_empty()).count()
    }
}

fn pluralize(count: usize, singular: &str, plural: Option<&str>) -> String {
    let noun = if count == 1 {
        singular.to_owned()
    } else {
        plural
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{singular}s"))
    };
    format!("{count} {noun}")
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1_024 {
        format!("{bytes} B")
    } else if bytes < 1_024 * 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_024.0 / 1_024.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditStats {
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteStats {
    pub lines: usize,
}

// Original: tool-renderers/chip.ts computeEditStats()
pub fn compute_edit_stats(args: &Map<String, Value>) -> EditStats {
    let old_string = str_arg(args, &["old_string"]);
    let new_string = str_arg(args, &["new_string"]);
    if old_string.is_empty() && new_string.is_empty() {
        return EditStats {
            added: 0,
            removed: 0,
        };
    }
    let mut added = 0;
    let mut removed = 0;
    for line in compute_diff_lines(old_string, new_string, 1, 1, false) {
        match line.kind {
            DiffLineKind::Add => added += 1,
            DiffLineKind::Delete => removed += 1,
            DiffLineKind::Context => {}
        }
    }
    EditStats { added, removed }
}

// Original: tool-renderers/chip.ts computeWriteStats()
pub fn compute_write_stats(args: &Map<String, Value>) -> WriteStats {
    let content = str_arg(args, &["content"]);
    let normalized = content.strip_suffix('\n').unwrap_or(content);
    WriteStats {
        lines: if normalized.is_empty() {
            0
        } else {
            normalized.split('\n').count()
        },
    }
}

pub fn format_edit_chip(stats: EditStats) -> String {
    let mut parts = Vec::new();
    if stats.added > 0 {
        parts.push(format!("+{}", stats.added));
    }
    if stats.removed > 0 {
        parts.push(format!("-{}", stats.removed));
    }
    parts.join(" ")
}

pub fn format_write_chip(stats: WriteStats) -> String {
    pluralize(stats.lines, "line", None)
}

fn edit_chip(tool_call: &ToolCallBlockData, _result: &ToolResultBlockData) -> String {
    let stats = compute_edit_stats(&tool_call.args);
    if stats.added == 0 && stats.removed == 0 {
        String::new()
    } else {
        format_edit_chip(stats)
    }
}

fn write_chip(tool_call: &ToolCallBlockData, _result: &ToolResultBlockData) -> String {
    format_write_chip(compute_write_stats(&tool_call.args))
}

fn read_chip(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    pluralize(count_non_empty_lines(&result.output), "line", None)
}

fn grep_chip(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    let matches = count_non_empty_lines(&result.output);
    if matches == 0 {
        "no matches".to_owned()
    } else {
        pluralize(matches, "match", Some("matches"))
    }
}

fn glob_chip(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    let files = count_non_empty_lines(&result.output);
    if files == 0 {
        "no files".to_owned()
    } else {
        pluralize(files, "file", None)
    }
}

fn fetch_chip(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    format_bytes(result.output.len())
}

fn web_search_chip(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    let lines = result
        .output
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let results = lines
        .iter()
        .filter(|line| WEB_RESULT_RE.is_match(line))
        .count();
    if results == 0 {
        if lines.is_empty() {
            "no results".to_owned()
        } else {
            "web result".to_owned()
        }
    } else {
        pluralize(results, "result", None)
    }
}

fn goal_status_output_chip(_tool_call: &ToolCallBlockData, result: &ToolResultBlockData) -> String {
    if result.is_error.unwrap_or(false) {
        String::new()
    } else {
        goal_status_chip(&result.output)
    }
}

// Original: tool-renderers/chip.ts pickChip()
pub fn pick_chip(tool_name: &str) -> Option<ChipProvider> {
    match tool_name {
        "Edit" => Some(edit_chip),
        "Write" => Some(write_chip),
        "Read" => Some(read_chip),
        "ReadMediaFile" => Some(read_media_chip),
        "Grep" => Some(grep_chip),
        "Glob" => Some(glob_chip),
        "FetchURL" => Some(fetch_chip),
        "WebSearch" => Some(web_search_chip),
        "CreateGoal" | "GetGoal" => Some(goal_status_output_chip),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    fn call(name: &str, arguments: Value) -> ToolCallBlockData {
        ToolCallBlockData {
            id: "tc".to_owned(),
            name: name.to_owned(),
            args: args(arguments),
            description: None,
            streaming_arguments: None,
            streaming_started_at_ms: None,
            step: None,
            turn_id: None,
            truncated: None,
        }
    }

    fn result(output: &str, is_error: bool) -> ToolResultBlockData {
        ToolResultBlockData {
            tool_call_id: "tc".to_owned(),
            output: output.to_owned(),
            is_error: Some(is_error),
            synthetic: None,
        }
    }

    fn chip_for(name: &str, arguments: Value, output: &str) -> String {
        let call = call(name, arguments);
        pick_chip(name).map_or_else(String::new, |provider| {
            provider(&call, &result(output, false))
        })
    }

    #[test]
    fn registry_selects_expected_chip_providers() {
        for name in [
            "Edit",
            "Write",
            "Read",
            "ReadMediaFile",
            "Grep",
            "Glob",
            "FetchURL",
            "WebSearch",
            "CreateGoal",
            "GetGoal",
        ] {
            assert!(pick_chip(name).is_some(), "missing provider for {name}");
        }
        for name in ["Bash", "Think", "SetGoalBudget", "UpdateGoal", "Unknown"] {
            assert!(pick_chip(name).is_none(), "unexpected provider for {name}");
        }
    }

    #[test]
    fn computes_edit_and_write_stats() {
        assert_eq!(
            compute_edit_stats(&Map::new()),
            EditStats {
                added: 0,
                removed: 0
            }
        );
        let edit = compute_edit_stats(&args(serde_json::json!({
            "old_string": "a\nb\nc",
            "new_string": "a\nB\nc\nd"
        })));
        assert!(edit.added > 0);
        assert!(edit.removed > 0);
        assert_eq!(
            compute_edit_stats(&args(serde_json::json!({
                "old_string": "",
                "new_string": "x\ny\nz"
            }))),
            EditStats {
                added: 3,
                removed: 0
            }
        );

        assert_eq!(compute_write_stats(&Map::new()), WriteStats { lines: 0 });
        assert_eq!(
            compute_write_stats(&args(serde_json::json!({"content": "hello"}))),
            WriteStats { lines: 1 }
        );
        for content in ["a\nb\n", "a\nb"] {
            assert_eq!(
                compute_write_stats(&args(serde_json::json!({"content": content}))),
                WriteStats { lines: 2 }
            );
        }
    }

    #[test]
    fn formats_edit_write_read_search_and_fetch_chips() {
        let edit = chip_for(
            "Edit",
            serde_json::json!({"old_string": "a\nb", "new_string": "a\nB\nc"}),
            "replaced",
        );
        assert!(edit.contains('+'));
        assert!(edit.contains('-'));
        assert_eq!(
            chip_for(
                "Write",
                serde_json::json!({"content": "a\nb\nc\n"}),
                "wrote"
            ),
            "3 lines"
        );
        assert_eq!(chip_for("Read", serde_json::json!({}), "1\tfoo"), "1 line");
        assert_eq!(
            chip_for("Grep", serde_json::json!({}), "a.ts\nb.ts\nc.ts"),
            "3 matches"
        );
        assert_eq!(chip_for("Grep", serde_json::json!({}), ""), "no matches");
        assert_eq!(
            chip_for("Glob", serde_json::json!({}), "a.ts\nb.ts"),
            "2 files"
        );
        assert_eq!(
            chip_for("FetchURL", serde_json::json!({}), "hello world"),
            "11 B"
        );
        assert_eq!(
            chip_for(
                "WebSearch",
                serde_json::json!({}),
                "1. Alpha\n2. Beta\n* Gamma"
            ),
            "3 results"
        );
        assert_eq!(
            chip_for("WebSearch", serde_json::json!({}), "unstructured"),
            "web result"
        );
        assert_eq!(
            chip_for("WebSearch", serde_json::json!({}), ""),
            "no results"
        );
    }

    #[test]
    fn formats_goal_status_and_suppresses_goal_errors() {
        assert_eq!(
            chip_for(
                "GetGoal",
                serde_json::json!({}),
                r#"{"goal":{"status":"active"}}"#
            ),
            "active"
        );
        assert_eq!(
            chip_for("GetGoal", serde_json::json!({}), r#"{"goal":null}"#),
            "no goal"
        );
        let call = call("GetGoal", serde_json::json!({}));
        assert_eq!(
            pick_chip("GetGoal").expect("goal chip")(&call, &result("failed", true)),
            ""
        );
    }
}
