use serde_json::{Map, Value};

use crate::{
    tui::{
        components::{Component, Text},
        theme::{ColorToken, current_theme},
        types::{ToolCallBlockData, ToolResultBlockData},
    },
    utils::usage::usage_format::format_token_count,
};

use super::{
    super::goal_format::{format_goal_elapsed, pluralize_goal_count},
    truncated::render_truncated,
    types::{RenderedComponents, RendererContext},
};

const STATUS_BULLET: &str = "● ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalToolName {
    CreateGoal,
    GetGoal,
    SetGoalBudget,
    UpdateGoal,
}

impl GoalToolName {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "CreateGoal" => Some(Self::CreateGoal),
            "GetGoal" => Some(Self::GetGoal),
            "SetGoalBudget" => Some(Self::SetGoalBudget),
            "UpdateGoal" => Some(Self::UpdateGoal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GoalToolHeaderOptions<'a> {
    pub tool_call: &'a ToolCallBlockData,
    pub result: Option<&'a ToolResultBlockData>,
    pub bullet: &'a str,
    pub chip: &'a str,
}

#[derive(Debug)]
struct GoalSnapshotView {
    objective: String,
    status: String,
    turns_used: u64,
    tokens_used: f64,
    wall_clock_ms: u64,
    terminal_reason: Option<String>,
}

pub fn is_goal_tool_name(tool_name: &str) -> bool {
    GoalToolName::parse(tool_name).is_some()
}

pub fn goal_summary(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    if result.is_error.unwrap_or(false) {
        return render_truncated(tool_call, result, context);
    }
    match GoalToolName::parse(&tool_call.name) {
        Some(GoalToolName::CreateGoal | GoalToolName::GetGoal) => {
            render_goal_snapshot(tool_call, result, context)
        }
        Some(GoalToolName::SetGoalBudget | GoalToolName::UpdateGoal) => Vec::new(),
        None => render_truncated(tool_call, result, context),
    }
}

// Original: tool-renderers/goal.ts buildGoalToolHeader()
pub fn build_goal_tool_header(options: GoalToolHeaderOptions<'_>) -> Option<String> {
    let tool_name = GoalToolName::parse(&options.tool_call.name)?;
    let failed = options
        .result
        .and_then(|result| result.is_error)
        .unwrap_or(false);
    let tone = if failed {
        ColorToken::Error
    } else {
        ColorToken::Primary
    };
    let theme = current_theme();
    let label = theme.bold_fg(
        tone,
        &goal_tool_label(tool_name, options.result, &options.tool_call.args),
    );
    let marker = if options.result.is_some() && !failed {
        theme.fg(ColorToken::Primary, STATUS_BULLET)
    } else {
        options.bullet.to_owned()
    };
    let argument = if tool_name == GoalToolName::UpdateGoal {
        None
    } else {
        format_goal_tool_argument(tool_name, &options.tool_call.args)
    };
    let argument = argument.map_or_else(String::new, |argument| {
        theme.dim_fg(ColorToken::TextDim, &format!(" ({argument})"))
    });
    Some(format!("{marker}{label}{argument}{}", options.chip))
}

fn format_goal_budget_arg(args: &Map<String, Value>) -> Option<String> {
    let value = args.get("value")?.as_f64()?;
    let unit = args.get("unit")?.as_str()?;
    if !value.is_finite() || unit.is_empty() {
        return None;
    }
    let normalized = if matches!(unit, "turns" | "tokens") {
        value.round().max(1.0)
    } else {
        value
    };
    let singular = unit.strip_suffix('s').unwrap_or(unit);
    let noun = if normalized == 1.0 { singular } else { unit };
    Some(format!("{normalized} {noun}"))
}

pub fn goal_status_chip(output: &str) -> String {
    match parse_goal_value(output) {
        Some(Some(goal)) => string_field(&goal, "status").unwrap_or_default().to_owned(),
        Some(None) => "no goal".to_owned(),
        None => String::new(),
    }
}

fn render_goal_snapshot(
    tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    let Some(parsed) = parse_goal_tool_output(&result.output) else {
        return render_truncated(tool_call, result, context);
    };
    let theme = current_theme();
    let Some(goal) = parsed else {
        return vec![Box::new(Text::new(
            theme.dim_fg(ColorToken::TextDim, "  No current goal."),
            0,
            0,
        ))];
    };
    let mut lines = vec![format!(
        "  {}",
        theme.fg(
            ColorToken::Text,
            &format!(
                "Goal {}: {}",
                goal.status,
                truncate_one_line(&goal.objective, 96)
            )
        )
    )];
    lines.push(format!(
        "    {}",
        theme.dim_fg(ColorToken::TextDim, &format_goal_stats(&goal))
    ));
    if let Some(reason) = goal
        .terminal_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        lines.push(format!("    {}", theme.dim_fg(ColorToken::TextDim, reason)));
    }
    lines
        .into_iter()
        .map(|line| Box::new(Text::new(line, 0, 0)) as Box<dyn Component>)
        .collect()
}

fn goal_tool_label(
    tool_name: GoalToolName,
    result: Option<&ToolResultBlockData>,
    args: &Map<String, Value>,
) -> String {
    let failed = result.and_then(|result| result.is_error).unwrap_or(false);
    let finished = result.is_some();
    match tool_name {
        GoalToolName::CreateGoal => match (failed, finished) {
            (true, _) => "Could not start goal".to_owned(),
            (false, true) => "Started goal".to_owned(),
            (false, false) => "Starting goal".to_owned(),
        },
        GoalToolName::GetGoal => match (failed, finished) {
            (true, _) => "Could not check goal".to_owned(),
            (false, true) => "Checked goal".to_owned(),
            (false, false) => "Checking goal".to_owned(),
        },
        GoalToolName::SetGoalBudget => match (failed, finished) {
            (true, _) => "Could not set goal budget".to_owned(),
            (false, true) => "Set goal budget".to_owned(),
            (false, false) => "Setting goal budget".to_owned(),
        },
        GoalToolName::UpdateGoal => {
            let suffix = string_arg(args, "status").unwrap_or("status");
            match (failed, finished) {
                (true, _) => format!("Could not report goal {suffix}"),
                (false, true) => format!("Reported goal {suffix}"),
                (false, false) => format!("Reporting goal {suffix}"),
            }
        }
    }
}

fn format_goal_tool_argument(tool_name: GoalToolName, args: &Map<String, Value>) -> Option<String> {
    match tool_name {
        GoalToolName::CreateGoal => {
            string_arg(args, "objective").map(|objective| truncate_one_line(objective, 60))
        }
        GoalToolName::SetGoalBudget => format_goal_budget_arg(args),
        GoalToolName::UpdateGoal => string_arg(args, "status").map(str::to_owned),
        GoalToolName::GetGoal => None,
    }
}

fn parse_goal_tool_output(output: &str) -> Option<Option<GoalSnapshotView>> {
    let goal = parse_goal_value(output)?;
    let Some(goal) = goal else {
        return Some(None);
    };
    Some(Some(GoalSnapshotView {
        objective: string_field(&goal, "objective")?.to_owned(),
        status: string_field(&goal, "status")?.to_owned(),
        turns_used: number_field(&goal, "turnsUsed").max(0.0) as u64,
        tokens_used: number_field(&goal, "tokensUsed"),
        wall_clock_ms: number_field(&goal, "wallClockMs").max(0.0) as u64,
        terminal_reason: string_field(&goal, "terminalReason").map(str::to_owned),
    }))
}

fn parse_goal_value(output: &str) -> Option<Option<Map<String, Value>>> {
    let mut parsed = serde_json::from_str::<Value>(output)
        .ok()?
        .as_object()?
        .clone();
    match parsed.remove("goal")? {
        Value::Null => Some(None),
        Value::Object(goal) => Some(Some(goal)),
        _ => None,
    }
}

fn format_goal_stats(goal: &GoalSnapshotView) -> String {
    [
        pluralize_goal_count(goal.turns_used, "turn", None),
        format!("{} tokens", format_token_count(goal.tokens_used)),
        format_goal_elapsed(goal.wall_clock_ms),
    ]
    .join(" · ")
}

fn truncate_one_line(text: &str, max: usize) -> String {
    let first_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if first_line.chars().count() <= max {
        return first_line;
    }
    let kept = first_line
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>();
    format!("{kept}…")
}

fn string_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn string_field<'a>(record: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    record.get(key).and_then(Value::as_str)
}

fn number_field(record: &Map<String, Value>, key: &str) -> f64 {
    record
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Value) -> ToolCallBlockData {
        ToolCallBlockData {
            id: "tc".to_owned(),
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
            tool_call_id: "tc".to_owned(),
            output: output.to_owned(),
            is_error: Some(is_error),
            synthetic: None,
        }
    }

    fn goal_output(goal: Value) -> String {
        serde_json::json!({"goal": goal}).to_string()
    }

    fn render(mut components: RenderedComponents) -> String {
        components
            .iter_mut()
            .flat_map(|component| component.render(120))
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut escape = false;
        for character in text.chars() {
            if character == '\u{1b}' {
                escape = true;
            } else if escape && character == 'm' {
                escape = false;
            } else if !escape {
                output.push(character);
            }
        }
        output
    }

    #[test]
    fn identifies_only_goal_tools() {
        for name in ["CreateGoal", "GetGoal", "SetGoalBudget", "UpdateGoal"] {
            assert!(is_goal_tool_name(name));
        }
        assert!(!is_goal_tool_name("Read"));
    }

    #[test]
    fn renders_goal_snapshots_null_goals_and_invalid_fallbacks() {
        let get = call("GetGoal", serde_json::json!({}));
        let output = goal_output(serde_json::json!({
            "objective": "Ship feature X",
            "status": "active",
            "turnsUsed": 2,
            "tokensUsed": 1234,
            "wallClockMs": 61000,
            "terminalReason": "waiting for review"
        }));
        let rendered = render(goal_summary(
            &get,
            &result(&output, false),
            RendererContext::default(),
        ));
        assert!(rendered.contains("Goal active: Ship feature X"));
        assert!(rendered.contains("2 turns · 1.2k tokens · 1m 01s"));
        assert!(rendered.contains("waiting for review"));
        assert!(!rendered.contains("objective"));

        let none = render(goal_summary(
            &get,
            &result(r#"{"goal":null}"#, false),
            RendererContext::default(),
        ));
        assert!(none.contains("No current goal."));

        let invalid = render(goal_summary(
            &get,
            &result("not json", false),
            RendererContext::default(),
        ));
        assert!(invalid.contains("not json"));
    }

    #[test]
    fn successful_mutations_have_no_body_and_errors_are_visible() {
        let update = call("UpdateGoal", serde_json::json!({"status": "complete"}));
        assert!(
            render(goal_summary(
                &update,
                &result("Goal marked complete.", false),
                RendererContext::default(),
            ))
            .is_empty()
        );
        assert!(
            render(goal_summary(
                &update,
                &result("goal update failed", true),
                RendererContext::default(),
            ))
            .contains("goal update failed")
        );
    }

    #[test]
    fn builds_goal_headers_and_budget_arguments() {
        let budget = call(
            "SetGoalBudget",
            serde_json::json!({"value": 10, "unit": "turns"}),
        );
        let success = result("set", false);
        let header = build_goal_tool_header(GoalToolHeaderOptions {
            tool_call: &budget,
            result: Some(&success),
            bullet: "fallback ",
            chip: " · chip",
        })
        .map(|header| strip_sgr(&header))
        .expect("goal header");
        assert_eq!(header, "● Set goal budget (10 turns) · chip");

        let update = call("UpdateGoal", serde_json::json!({"status": "complete"}));
        let pending = build_goal_tool_header(GoalToolHeaderOptions {
            tool_call: &update,
            result: None,
            bullet: "◦ ",
            chip: "",
        })
        .map(|header| strip_sgr(&header))
        .expect("goal header");
        assert_eq!(pending, "◦ Reporting goal complete");
        assert!(
            build_goal_tool_header(GoalToolHeaderOptions {
                tool_call: &call("Read", serde_json::json!({})),
                result: None,
                bullet: "",
                chip: "",
            })
            .is_none()
        );
    }

    #[test]
    fn parses_status_chips() {
        assert_eq!(
            goal_status_chip(&goal_output(serde_json::json!({"status": "active"}))),
            "active"
        );
        assert_eq!(goal_status_chip(r#"{"goal":null}"#), "no goal");
        assert_eq!(goal_status_chip("invalid"), "");
    }
}
