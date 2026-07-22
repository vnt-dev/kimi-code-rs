use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::{
    cli::prompt_session::{
        ApprovalDecision as CoreApprovalDecision, ApprovalRequest, ApprovalResponse, ApprovalScope,
    },
    tui::{
        components::dialogs::{
            ApprovalPanelResponse, GoalStartMode, GoalStartPermissionChoice, goal_start_options,
        },
        reverse_rpc::types::{
            ApprovalDecision, ApprovalPanelChoice, ApprovalPanelData, DiffDisplayBlock,
            DisplayBlock, FileContentDisplayBlock, FileOperation, InvocationKind,
        },
    },
};

// Original:
//   apps/kimi-code/src/tui/reverse-rpc/approval/adapter.ts
//   adaptApprovalRequest()
pub fn adapt_approval_request(event: &ApprovalRequest) -> ApprovalPanelData {
    let (display, description) = resolve_display(&event.tool_name, &event.display, &event.action);
    ApprovalPanelData {
        id: event.tool_call_id.clone(),
        tool_call_id: event.tool_call_id.clone(),
        tool_name: event.tool_name.clone(),
        action: event.action.clone(),
        description,
        display,
        choices: adapt_choices(&event.tool_name, &event.display),
    }
}

fn resolve_display(tool_name: &str, display: &Value, action: &str) -> (Vec<DisplayBlock>, String) {
    if display_kind(display) == Some("generic")
        && let Some(detail) = display.get("detail").and_then(Value::as_object)
        && let Some(resolved) = extract_from_args(tool_name, detail)
    {
        return resolved;
    }
    (adapt_display(display), describe_approval(display, action))
}

fn extract_from_args(
    tool_name: &str,
    detail: &Map<String, Value>,
) -> Option<(Vec<DisplayBlock>, String)> {
    if let Some(command) = string_field(detail, "command") {
        let description = string_field(detail, "description").map(str::to_owned);
        return Some((
            vec![DisplayBlock::Shell {
                language: string_field(detail, "language")
                    .unwrap_or("bash")
                    .to_owned(),
                command: command.to_owned(),
                cwd: string_field(detail, "cwd").map(str::to_owned),
                description: description.clone(),
                danger: detect_danger(command).map(str::to_owned),
            }],
            description.unwrap_or_default(),
        ));
    }

    let old_string = string_field(detail, "old_string");
    let new_string = string_field(detail, "new_string");
    if let (Some(old_text), Some(new_text)) = (old_string, new_string) {
        let path = string_field(detail, "file_path")
            .or_else(|| string_field(detail, "path"))
            .unwrap_or_default();
        return Some((
            vec![DisplayBlock::Diff(DiffDisplayBlock {
                path: path.to_owned(),
                old_text: old_text.to_owned(),
                new_text: new_text.to_owned(),
                old_start: None,
                new_start: None,
                is_summary: None,
            })],
            String::new(),
        ));
    }

    let file_path = string_field(detail, "file_path").or_else(|| string_field(detail, "path"));
    if let (Some(path), Some(content)) = (file_path, string_field(detail, "content")) {
        return Some((
            vec![DisplayBlock::FileContent(FileContentDisplayBlock {
                path: path.to_owned(),
                content: content.to_owned(),
                language: None,
            })],
            String::new(),
        ));
    }
    if let Some(url) = string_field(detail, "url") {
        return Some((
            vec![DisplayBlock::UrlFetch {
                url: url.to_owned(),
                method: string_field(detail, "method").map(str::to_owned),
            }],
            String::new(),
        ));
    }
    if let Some(query) = string_field(detail, "query") {
        return Some((
            vec![DisplayBlock::Search {
                query: query.to_owned(),
                scope: None,
            }],
            String::new(),
        ));
    }
    if let Some(pattern) = string_field(detail, "pattern") {
        return Some((
            vec![DisplayBlock::Search {
                query: pattern.to_owned(),
                scope: string_field(detail, "path").map(str::to_owned),
            }],
            String::new(),
        ));
    }
    file_path.map(|path| {
        (
            vec![DisplayBlock::FileOp {
                operation: infer_file_op(tool_name),
                path: path.to_owned(),
                detail: None,
            }],
            String::new(),
        )
    })
}

fn string_field<'a>(detail: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    detail.get(key).and_then(Value::as_str)
}

fn infer_file_op(tool_name: &str) -> FileOperation {
    let lower = tool_name.to_lowercase();
    if lower.contains("glob") {
        FileOperation::Glob
    } else if lower.contains("grep") {
        FileOperation::Grep
    } else if lower.contains("edit") {
        FileOperation::Edit
    } else if lower.contains("write") {
        FileOperation::Write
    } else {
        FileOperation::Read
    }
}

// Original: adaptPanelResponse()
pub fn adapt_panel_response(response: &ApprovalPanelResponse) -> ApprovalResponse {
    let (decision, scope) = match response.response {
        ApprovalDecision::ApprovedForSession => {
            (CoreApprovalDecision::Approved, Some(ApprovalScope::Session))
        }
        ApprovalDecision::Approved => (CoreApprovalDecision::Approved, None),
        ApprovalDecision::Rejected => (CoreApprovalDecision::Rejected, None),
        ApprovalDecision::Cancelled => (CoreApprovalDecision::Cancelled, None),
    };
    ApprovalResponse {
        decision,
        scope,
        feedback: response.feedback.clone(),
        selected_label: response.selected_label.clone(),
    }
}

fn describe_approval(display: &Value, action: &str) -> String {
    match display_kind(display).unwrap_or_default() {
        "plan_review" => String::new(),
        "goal_start" => "Start a goal?".to_owned(),
        "generic" => display
            .get("detail")
            .and_then(Value::as_str)
            .filter(|detail| !detail.is_empty())
            .or_else(|| display.get("summary").and_then(Value::as_str))
            .unwrap_or(action)
            .to_owned(),
        "command" => first_string(display, &["description", "command"])
            .unwrap_or(action)
            .to_owned(),
        "diff" => prefixed_description("edit", display.get("path")),
        "file_io" => {
            let operation = display
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("file");
            prefixed_description(operation, display.get("path"))
        }
        "task_stop" => prefixed_description(
            "stop task:",
            display
                .get("task_description")
                .or_else(|| display.get("task_id")),
        ),
        "agent_call" => format!(
            "spawn {}",
            display
                .get("agent_name")
                .and_then(Value::as_str)
                .unwrap_or("agent")
        ),
        "skill_call" => prefixed_description("invoke skill", display.get("skill_name")),
        "url_fetch" => prefixed_description("fetch", display.get("url")),
        "search" => prefixed_description("search:", display.get("query")),
        "todo_list" => format!(
            "update todo list ({} items)",
            display
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        ),
        "task" => format!(
            "{} task {}: {}",
            display
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("background"),
            display
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            display
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )
        .trim()
        .to_owned(),
        _ => action.to_owned(),
    }
}

fn prefixed_description(prefix: &str, value: Option<&Value>) -> String {
    format!(
        "{prefix} {}",
        value.and_then(Value::as_str).unwrap_or_default()
    )
    .trim()
    .to_owned()
}

fn first_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

static DANGER_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            r"(?i)\brm\s+(-[a-z]*[rRfF][a-z]*|--recursive|--force)",
            "recursive delete",
        ),
        (r"(?i)\bsudo\b", "sudo"),
        (
            r"(?i)\b(curl|wget)\b[^|]*\|\s*(sh|bash|zsh)\b",
            "pipe to shell",
        ),
        (r"(?i)\bdd\b[^|]*\bof=", "dd write"),
        (r"(?i)\bmkfs\b", "mkfs"),
        (r"(?i)>\s*/dev/(sd|nvme|disk|hd)", "write to raw device"),
        (r"(?i)\bchmod\s+-R?\s*777\b", "chmod 777"),
        (r"(?i):\(\)\s*\{\s*:\|:&\s*\}", "fork bomb"),
    ]
    .into_iter()
    .filter_map(|(pattern, label)| Regex::new(pattern).ok().map(|regex| (regex, label)))
    .collect()
});

fn detect_danger(command: &str) -> Option<&'static str> {
    DANGER_PATTERNS
        .iter()
        .find_map(|(pattern, label)| pattern.is_match(command).then_some(*label))
}

fn adapt_display(display: &Value) -> Vec<DisplayBlock> {
    match display_kind(display).unwrap_or_default() {
        "command" => {
            let command = display
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            vec![DisplayBlock::Shell {
                language: display
                    .get("language")
                    .and_then(Value::as_str)
                    .unwrap_or("bash")
                    .to_owned(),
                command: command.to_owned(),
                cwd: optional_string(display, "cwd"),
                description: optional_string(display, "description"),
                danger: detect_danger(command).map(str::to_owned),
            }]
        }
        "diff" => vec![diff_block(
            string_value(display, "path"),
            string_value(display, "before"),
            string_value(display, "after"),
        )],
        "file_io" => adapt_file_io(display),
        "url_fetch" => vec![DisplayBlock::UrlFetch {
            url: string_value(display, "url"),
            method: optional_string(display, "method"),
        }],
        "search" => vec![DisplayBlock::Search {
            query: string_value(display, "query"),
            scope: optional_string(display, "scope"),
        }],
        "agent_call" => vec![DisplayBlock::Invocation {
            kind: InvocationKind::Agent,
            name: string_value(display, "agent_name"),
            description: optional_string(display, "prompt"),
        }],
        "skill_call" => vec![DisplayBlock::Invocation {
            kind: InvocationKind::Skill,
            name: string_value(display, "skill_name"),
            description: optional_string(display, "args"),
        }],
        "task_stop" => vec![DisplayBlock::Brief {
            text: format!(
                "Stop task {}: {}",
                string_value(display, "task_id"),
                string_value(display, "task_description")
            ),
        }],
        "goal_start" => {
            let mut lines = vec![format!(
                "Start goal: {}",
                string_value(display, "objective")
            )];
            if let Some(criterion) = display
                .get("completionCriterion")
                .and_then(Value::as_str)
                .filter(|criterion| !criterion.is_empty())
            {
                lines.push(format!("Done when: {criterion}"));
            }
            vec![DisplayBlock::Brief {
                text: lines.join("\n"),
            }]
        }
        _ => Vec::new(),
    }
}

fn adapt_file_io(display: &Value) -> Vec<DisplayBlock> {
    let path = string_value(display, "path");
    let operation = display
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if operation == "write"
        && let Some(content) = display.get("content").and_then(Value::as_str)
    {
        return vec![DisplayBlock::FileContent(FileContentDisplayBlock {
            path,
            content: content.to_owned(),
            language: None,
        })];
    }
    if operation == "edit"
        && let (Some(before), Some(after)) = (
            display.get("before").and_then(Value::as_str),
            display.get("after").and_then(Value::as_str),
        )
    {
        return vec![diff_block(path, before.to_owned(), after.to_owned())];
    }
    vec![DisplayBlock::FileOp {
        operation: parse_file_operation(operation),
        path,
        detail: optional_string(display, "detail"),
    }]
}

fn diff_block(path: String, old_text: String, new_text: String) -> DisplayBlock {
    DisplayBlock::Diff(DiffDisplayBlock {
        path,
        old_text,
        new_text,
        old_start: None,
        new_start: None,
        is_summary: None,
    })
}

fn parse_file_operation(value: &str) -> FileOperation {
    match value {
        "write" => FileOperation::Write,
        "edit" => FileOperation::Edit,
        "glob" => FileOperation::Glob,
        "grep" => FileOperation::Grep,
        _ => FileOperation::Read,
    }
}

fn display_kind(display: &Value) -> Option<&str> {
    display.get("kind").and_then(Value::as_str)
}

fn string_value(display: &Value, key: &str) -> String {
    display
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn optional_string(display: &Value, key: &str) -> Option<String> {
    display.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn adapt_choices(tool_name: &str, display: &Value) -> Vec<ApprovalPanelChoice> {
    if tool_name == "ExitPlanMode" || display_kind(display) == Some("plan_review") {
        return adapt_plan_review_choices(display);
    }
    if display_kind(display) == Some("goal_start") {
        return adapt_goal_start_choices(display);
    }
    default_approval_choices()
}

fn default_approval_choices() -> Vec<ApprovalPanelChoice> {
    vec![
        choice("Approve once", ApprovalDecision::Approved),
        choice(
            "Approve for this session",
            ApprovalDecision::ApprovedForSession,
        ),
        choice("Reject", ApprovalDecision::Rejected),
        ApprovalPanelChoice {
            requires_feedback: true,
            ..choice("Reject with feedback", ApprovalDecision::Rejected)
        },
    ]
}

fn adapt_goal_start_choices(display: &Value) -> Vec<ApprovalPanelChoice> {
    let mode = match display.get("mode").and_then(Value::as_str) {
        Some("yolo") => GoalStartMode::Yolo,
        _ => GoalStartMode::Manual,
    };
    goal_start_options(mode)
        .into_iter()
        .map(|option| {
            let (response, selected_label) = match option.value {
                GoalStartPermissionChoice::Auto => (ApprovalDecision::Approved, "auto"),
                GoalStartPermissionChoice::Yolo => (ApprovalDecision::Approved, "yolo"),
                GoalStartPermissionChoice::Manual => (ApprovalDecision::Approved, "manual"),
                GoalStartPermissionChoice::Cancel => (ApprovalDecision::Cancelled, "cancel"),
            };
            ApprovalPanelChoice {
                label: option.label,
                response,
                selected_label: Some(selected_label.to_owned()),
                requires_feedback: false,
                description: Some(option.description),
            }
        })
        .collect()
}

fn adapt_plan_review_choices(display: &Value) -> Vec<ApprovalPanelChoice> {
    let mut choices = display
        .get("options")
        .and_then(Value::as_array)
        .filter(|options| options.len() >= 2)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| option.get("label").and_then(Value::as_str))
                .map(|label| ApprovalPanelChoice {
                    label: label.to_owned(),
                    response: ApprovalDecision::Approved,
                    selected_label: Some(label.to_owned()),
                    requires_feedback: false,
                    description: None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            vec![selected_choice(
                "Approve",
                ApprovalDecision::Approved,
                false,
            )]
        });
    choices.extend([
        selected_choice("Reject", ApprovalDecision::Rejected, false),
        selected_choice("Revise", ApprovalDecision::Rejected, true),
    ]);
    choices
}

fn choice(label: &str, response: ApprovalDecision) -> ApprovalPanelChoice {
    ApprovalPanelChoice {
        label: label.to_owned(),
        response,
        selected_label: None,
        requires_feedback: false,
        description: None,
    }
}

fn selected_choice(
    label: &str,
    response: ApprovalDecision,
    requires_feedback: bool,
) -> ApprovalPanelChoice {
    ApprovalPanelChoice {
        selected_label: Some(label.to_owned()),
        requires_feedback,
        ..choice(label, response)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(
        tool_call_id: &str,
        tool_name: &str,
        action: &str,
        display: Value,
    ) -> ApprovalRequest {
        ApprovalRequest {
            turn_id: None,
            tool_call_id: tool_call_id.to_owned(),
            tool_name: tool_name.to_owned(),
            action: action.to_owned(),
            display,
        }
    }

    #[test]
    fn generic_command_becomes_shell_block_with_first_matching_danger() {
        let adapted = adapt_approval_request(&request(
            "tc-1",
            "EnterPlanMode",
            "run",
            json!({"kind":"generic","summary":"run","detail":{"command":"sudo rm -rf /tmp/cache","cwd":"/tmp"}}),
        ));
        assert!(matches!(
            &adapted.display[..],
            [DisplayBlock::Shell { language, command, cwd: Some(cwd), danger: Some(danger), .. }]
                if language == "bash" && command == "sudo rm -rf /tmp/cache" && cwd == "/tmp" && danger == "recursive delete"
        ));
        assert_eq!(
            adapted
                .choices
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            [
                "Approve once",
                "Approve for this session",
                "Reject",
                "Reject with feedback"
            ]
        );
    }

    #[test]
    fn generic_edit_and_write_emit_previewable_blocks_only() {
        let edit = adapt_approval_request(&request(
            "edit",
            "Edit",
            "edit",
            json!({"kind":"generic","detail":{"file_path":"src/foo.ts","old_string":"a\nb","new_string":"a\nB"}}),
        ));
        assert!(
            matches!(&edit.display[..], [DisplayBlock::Diff(block)] if block.path == "src/foo.ts")
        );
        let write = adapt_approval_request(&request(
            "write",
            "Write",
            "write",
            json!({"kind":"generic","detail":{"file_path":"src/new.ts","content":"const x = 1;"}}),
        ));
        assert!(
            matches!(&write.display[..], [DisplayBlock::FileContent(block)] if block.path == "src/new.ts")
        );
    }

    #[test]
    fn file_io_write_edit_and_read_choose_matching_block_types() {
        let write = adapt_approval_request(&request(
            "w",
            "Write",
            "write",
            json!({"kind":"file_io","operation":"write","path":"new.rs","content":"fn main() {}"}),
        ));
        assert!(matches!(&write.display[..], [DisplayBlock::FileContent(_)]));
        let edit = adapt_approval_request(&request(
            "e",
            "Edit",
            "edit",
            json!({"kind":"file_io","operation":"edit","path":"x.rs","before":"a","after":"b"}),
        ));
        assert!(matches!(&edit.display[..], [DisplayBlock::Diff(_)]));
        let read = adapt_approval_request(&request(
            "r",
            "Read",
            "read",
            json!({"kind":"file_io","operation":"read","path":"x.rs"}),
        ));
        assert!(
            matches!(&read.display[..], [DisplayBlock::FileOp { operation: FileOperation::Read, path, .. }] if path == "x.rs")
        );
    }

    #[test]
    fn plan_review_hides_content_and_builds_default_or_custom_choices() {
        let default = adapt_approval_request(&request(
            "p",
            "ExitPlanMode",
            "review",
            json!({"kind":"plan_review","plan":"# Plan"}),
        ));
        assert!(default.display.is_empty());
        assert_eq!(
            default
                .choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["Approve", "Reject", "Revise"]
        );
        let custom = adapt_approval_request(&request(
            "p2",
            "ExitPlanMode",
            "review",
            json!({"kind":"plan_review","options":[{"label":"A"},{"label":"B"}]}),
        ));
        assert_eq!(
            custom
                .choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["A", "B", "Reject", "Revise"]
        );
        assert!(custom.choices[3].requires_feedback);
    }

    #[test]
    fn goal_start_reuses_permission_menu_and_previews_completion_criterion() {
        let adapted = adapt_approval_request(&request(
            "g",
            "CreateGoal",
            "create",
            json!({"kind":"goal_start","objective":"Fix auth","completionCriterion":"tests pass","mode":"manual"}),
        ));
        assert!(
            matches!(&adapted.display[..], [DisplayBlock::Brief { text }] if text == "Start goal: Fix auth\nDone when: tests pass")
        );
        assert_eq!(
            adapted
                .choices
                .iter()
                .filter_map(|choice| choice.selected_label.as_deref())
                .collect::<Vec<_>>(),
            ["auto", "yolo", "manual", "cancel"]
        );
        assert_eq!(
            adapted.choices.last().map(|choice| choice.response),
            Some(ApprovalDecision::Cancelled)
        );
        assert!(
            adapted
                .choices
                .iter()
                .all(|choice| choice.description.is_some())
        );
    }

    #[test]
    fn approved_for_session_panel_response_maps_to_core_scope() {
        let response = adapt_panel_response(&ApprovalPanelResponse {
            response: ApprovalDecision::ApprovedForSession,
            feedback: Some("looks good".to_owned()),
            selected_label: Some("Approve for this session".to_owned()),
        });
        assert_eq!(response.decision, CoreApprovalDecision::Approved);
        assert_eq!(response.scope, Some(ApprovalScope::Session));
        assert_eq!(response.feedback.as_deref(), Some("looks good"));
        assert_eq!(
            response.selected_label.as_deref(),
            Some("Approve for this session")
        );
    }
}
