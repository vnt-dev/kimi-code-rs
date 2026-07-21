use std::any::Any;

use crate::{
    sdk::types::{GoalSnapshot, GoalStatus},
    tui::{
        components::{
            Component, ComponentRole, Text,
            render::{truncate_to_width, visible_width, wrap_text_with_ansi},
        },
        theme::{ColorToken, current_theme},
    },
    utils::usage::usage_format::format_token_count,
};

use super::{goal_format::format_goal_elapsed, usage_panel::UsagePanelComponent};

const MESSAGE_INDENT: &str = "  ";
const STATUS_BULLET: &str = "● ";
const WRAP_WIDTH: usize = 72;
const MAX_OBJECTIVE_LINES: usize = 6;
const MAX_CRITERION_LINES: usize = 3;
const LABEL_WIDTH: usize = 11;

fn render_lifecycle_line(label: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let theme = current_theme();
    let marker = theme.bold_fg(ColorToken::Primary, STATUS_BULLET);
    let mut text = Text::new(theme.bold_fg(ColorToken::Primary, label), 0, 0);
    let content_width = width.saturating_sub(visible_width(STATUS_BULLET)).max(1);
    let mut lines = vec![String::new()];
    lines.extend(
        text.render(content_width)
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                format!(
                    "{}{}",
                    if index == 0 { &marker } else { MESSAGE_INDENT },
                    line.trim_end()
                )
            }),
    );
    lines
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GoalSetMessageComponent;

impl Component for GoalSetMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        render_lifecycle_line("Goal set", width)
    }

    fn invalidate(&mut self) {}
    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UpcomingGoalAddedMessageComponent;

impl Component for UpcomingGoalAddedMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        render_lifecycle_line(
            "Upcoming goal added. It will start after the current goal is complete.",
            width,
        )
    }

    fn invalidate(&mut self) {}
    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalCompletionMessageComponent {
    message: String,
}

impl GoalCompletionMessageComponent {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Component for GoalCompletionMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let normalized = self.message.trim().replace("\r\n", "\n");
        let mut message_lines = normalized.split('\n');
        let headline = message_lines.next().unwrap_or_default();
        if headline.is_empty() {
            return Vec::new();
        }
        let theme = current_theme();
        let bullet = theme.bold_fg(ColorToken::Success, STATUS_BULLET);
        let bullet_width = visible_width(STATUS_BULLET);
        let content_width = width.saturating_sub(bullet_width).max(1);
        let mut lines = vec![String::new()];
        let mut headline_text = Text::new(theme.bold_fg(ColorToken::Success, headline), 0, 0);
        lines.extend(
            headline_text
                .render(content_width)
                .into_iter()
                .enumerate()
                .map(|(index, line)| {
                    format!(
                        "{}{}",
                        if index == 0 { &bullet } else { MESSAGE_INDENT },
                        line
                    )
                }),
        );
        let detail = message_lines.collect::<Vec<_>>().join("\n");
        let detail = detail.trim();
        if !detail.is_empty() {
            let mut detail_text = Text::new(theme.fg(ColorToken::TextDim, detail), 0, 0);
            lines.extend(
                detail_text
                    .render(content_width)
                    .into_iter()
                    .map(|line| format!("{MESSAGE_INDENT}{line}")),
            );
        }
        lines
    }

    fn invalidate(&mut self) {}
    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalStatusMessageComponent {
    goal: GoalSnapshot,
}

impl GoalStatusMessageComponent {
    pub fn new(goal: GoalSnapshot) -> Self {
        Self { goal }
    }
}

impl Component for GoalStatusMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let panel_content_width = width.saturating_sub(6).max(1);
        let goal = self.goal.clone();
        let title = goal_panel_title(&goal);
        let mut panel = UsagePanelComponent::new(
            move || build_goal_report_lines_with_width(&goal, panel_content_width),
            ColorToken::Primary,
            title,
        );
        let mut lines = vec![String::new()];
        lines.extend(panel.render(width));
        lines
    }

    fn invalidate(&mut self) {}
    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn goal_panel_title(goal: &GoalSnapshot) -> String {
    format!(" Goal · {} ", goal.status.as_str())
}

/// Original: goal-panel.ts buildGoalReportLines()
pub fn build_goal_report_lines(goal: &GoalSnapshot) -> Vec<String> {
    build_goal_report_lines_with_width(goal, WRAP_WIDTH)
}

pub fn build_goal_report_lines_with_width(goal: &GoalSnapshot, wrap_width: usize) -> Vec<String> {
    let status_color = status_token(goal.status);
    let theme = current_theme();
    let is_complete = goal.status == GoalStatus::Complete;
    let reason = goal.terminal_reason.as_deref();
    let show_reason = (goal.status == GoalStatus::Paused && reason.is_some())
        || goal.status == GoalStatus::Blocked
        || is_complete;
    let mut lines = Vec::new();
    let blockquote_width = wrap_width.saturating_sub(visible_width("▌ ")).max(1);
    for line in wrap_capped(&goal.objective, blockquote_width, MAX_OBJECTIVE_LINES) {
        lines.push(format!(
            "{} {}",
            theme.fg(status_color, "▌"),
            theme.fg(ColorToken::Text, &line)
        ));
    }
    if let Some(criterion) = &goal.completion_criterion {
        for line in wrap_capped(
            &format!("✓ {criterion}"),
            blockquote_width,
            MAX_CRITERION_LINES,
        ) {
            lines.push(format!(
                "{} {}",
                theme.fg(status_color, "▌"),
                theme.fg(ColorToken::TextDim, &line)
            ));
        }
    }
    lines.push(String::new());

    if show_reason {
        let mut value = theme.fg(status_color, goal.status.as_str());
        if let Some(reason) = reason {
            value.push_str(&theme.fg(ColorToken::TextDim, &format!(" — {reason}")));
        }
        lines.push(report_row("Status", &value));
    }
    lines.push(report_row(
        "Running",
        &theme.fg(ColorToken::Text, &format_goal_elapsed(goal.wall_clock_ms)),
    ));
    lines.push(report_row(
        "Turns",
        &theme.fg(ColorToken::Text, &goal.turns_used.to_string()),
    ));
    lines.push(report_row(
        "Tokens",
        &theme.fg(
            ColorToken::Text,
            &format_token_count(goal.tokens_used as f64),
        ),
    ));
    if !is_complete {
        if let Some(stop) = format_stop_row(goal) {
            lines.push(report_row("Stop", &theme.fg(ColorToken::Text, &stop)));
        } else {
            lines.push(theme.fg(
                ColorToken::TextDim,
                "No stop condition — runs until evaluated complete.",
            ));
        }
    }
    lines
}

fn report_row(label: &str, value: &str) -> String {
    format!(
        "{}{}",
        current_theme().fg(ColorToken::TextDim, &format!("{label:<LABEL_WIDTH$}")),
        value
    )
}

fn format_stop_row(goal: &GoalSnapshot) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(turn_budget) = goal.budget.turn_budget {
        parts.push(format!(
            "after {turn_budget} turns ({}/{turn_budget})",
            goal.turns_used
        ));
    }
    if let Some(token_budget) = goal.budget.token_budget {
        parts.push(format!(
            "at {} tokens",
            format_token_count(token_budget as f64)
        ));
    }
    if let Some(wall_clock_budget) = goal.budget.wall_clock_budget_ms {
        parts.push(format!("after {}", format_goal_elapsed(wall_clock_budget)));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn status_token(status: GoalStatus) -> ColorToken {
    match status {
        GoalStatus::Active => ColorToken::Primary,
        GoalStatus::Complete => ColorToken::Success,
        GoalStatus::Blocked => ColorToken::Warning,
        GoalStatus::Paused => ColorToken::TextDim,
    }
}

fn wrap_capped(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut lines = wrap_text_with_ansi(normalized.trim(), width.max(1));
    if lines.is_empty() {
        return vec![String::new()];
    }
    if lines.len() <= max_lines {
        return lines;
    }
    lines.truncate(max_lines);
    if let Some(last) = lines.last_mut() {
        *last = truncate_to_width(&format!("{last}…"), width.max(1), "…", false);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::types::GoalBudgetReport;

    fn goal() -> GoalSnapshot {
        GoalSnapshot {
            goal_id: "g1".to_owned(),
            objective: "Ship the goal status box".to_owned(),
            completion_criterion: None,
            status: GoalStatus::Active,
            turns_used: 7,
            tokens_used: 128_400,
            wall_clock_ms: 252_000,
            budget: GoalBudgetReport {
                token_budget: None,
                turn_budget: None,
                wall_clock_budget_ms: None,
                remaining_tokens: None,
                remaining_turns: None,
                remaining_wall_clock_ms: None,
                token_budget_reached: false,
                turn_budget_reached: false,
                wall_clock_budget_reached: false,
                over_budget: false,
            },
            terminal_reason: None,
        }
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
    fn builds_active_goal_report_and_stop_conditions() {
        let mut goal = goal();
        let plain = strip_sgr(&build_goal_report_lines(&goal).join("\n"));
        assert!(plain.contains("▌ Ship the goal status box"));
        assert!(plain.contains("4m 12s"));
        assert!(plain.contains("125k"));
        assert!(plain.contains("No stop condition"));

        goal.budget.turn_budget = Some(20);
        let plain = strip_sgr(&build_goal_report_lines(&goal).join("\n"));
        assert!(plain.contains("after 20 turns (7/20)"));
        assert!(!plain.contains("No stop condition"));
    }

    #[test]
    fn renders_complete_and_paused_status_reasons() {
        let mut complete = goal();
        complete.status = GoalStatus::Complete;
        complete.terminal_reason = Some("all done".to_owned());
        let plain = strip_sgr(&build_goal_report_lines(&complete).join("\n"));
        assert!(plain.contains("complete — all done"));
        assert!(!plain.contains("Stop"));

        let mut paused = goal();
        paused.status = GoalStatus::Paused;
        paused.terminal_reason = Some("Paused after provider rate limit".to_owned());
        assert!(
            strip_sgr(&build_goal_report_lines(&paused).join("\n"))
                .contains("paused — Paused after provider rate limit")
        );
    }

    #[test]
    fn renders_lifecycle_completion_and_status_components() {
        let mut set = GoalSetMessageComponent;
        assert_eq!(strip_sgr(&set.render(60).join("\n")), "\n● Goal set");
        let mut upcoming = UpcomingGoalAddedMessageComponent;
        assert!(strip_sgr(&upcoming.render(80).join("\n")).contains("Upcoming goal added"));

        let mut complete = GoalCompletionMessageComponent::new(
            "✓ Goal complete.\nWorked 1 turn over 2m28s, using 766.9k tokens.",
        );
        let complete = strip_sgr(&complete.render(80).join("\n"));
        assert!(complete.contains("● ✓ Goal complete."));
        assert!(complete.contains("  Worked 1 turn"));

        let mut status = GoalStatusMessageComponent::new(goal());
        let rendered = status.render(80);
        assert_eq!(rendered[0], "");
        assert!(strip_sgr(&rendered[1]).contains("╭ Goal · active "));
    }

    #[test]
    fn caps_long_objectives_and_keeps_components_within_narrow_widths() {
        let mut goal = goal();
        goal.objective = "word ".repeat(200);
        assert!(strip_sgr(&build_goal_report_lines(&goal).join("\n")).contains('…'));
        goal.objective = "管理飞书日历的技能描述 ".repeat(4);
        let mut status = GoalStatusMessageComponent::new(goal);
        for width in [39, 24, 20, 10] {
            assert!(
                status
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
        let mut set = GoalSetMessageComponent;
        for width in [39, 20, 10, 4] {
            assert!(
                set.render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }
}
