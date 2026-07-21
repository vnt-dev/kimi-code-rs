use std::any::Any;

use crate::{
    sdk::types::{GoalActor, GoalChange, GoalChangeKind, GoalStatus},
    tui::{
        components::{Component, ComponentRole, render::truncate_to_width},
        theme::{ColorToken, current_theme},
    },
};

const STATUS_BULLET: &str = "● ";
const HEAD_INDENT: &str = "  ";
const DETAIL_INDENT: &str = "    ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalMarkerOptions {
    pub marker: String,
    pub text_token: ColorToken,
    pub expandable: bool,
    pub indent: String,
    pub leading_blank: bool,
}

impl Default for GoalMarkerOptions {
    fn default() -> Self {
        Self {
            marker: "◦".to_owned(),
            text_token: ColorToken::TextDim,
            expandable: true,
            indent: HEAD_INDENT.to_owned(),
            leading_blank: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalMarkerComponent {
    headline: String,
    detail: Option<String>,
    accent_token: ColorToken,
    expanded: bool,
    options: GoalMarkerOptions,
}

impl GoalMarkerComponent {
    pub fn new(
        headline: impl Into<String>,
        detail: Option<String>,
        accent_token: ColorToken,
    ) -> Self {
        Self::with_options(headline, detail, accent_token, GoalMarkerOptions::default())
    }

    pub fn with_options(
        headline: impl Into<String>,
        detail: Option<String>,
        accent_token: ColorToken,
        options: GoalMarkerOptions,
    ) -> Self {
        Self {
            headline: headline.into(),
            detail,
            accent_token,
            expanded: false,
            options,
        }
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    fn clamp_to_width(&self, mut lines: Vec<String>, width: usize) -> Vec<String> {
        if self.options.leading_blank {
            lines.insert(0, String::new());
        }
        if width == 0 {
            return vec![String::new(); lines.len()];
        }
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }
}

impl Component for GoalMarkerComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let dot = theme.fg(self.accent_token, &self.options.marker);
        let head = theme.fg(self.options.text_token, &self.headline);
        let Some(detail) = self.detail.as_deref().filter(|detail| !detail.is_empty()) else {
            return self
                .clamp_to_width(vec![format!("{}{dot} {head}", self.options.indent)], width);
        };
        if !self.options.expandable {
            return self
                .clamp_to_width(vec![format!("{}{dot} {head}", self.options.indent)], width);
        }
        if !self.expanded {
            return self.clamp_to_width(
                vec![format!(
                    "{}{dot} {head} {}",
                    self.options.indent,
                    theme.fg(ColorToken::TextMuted, "(ctrl+o)")
                )],
                width,
            );
        }

        let mut lines = vec![format!("{}{dot} {head}", self.options.indent)];
        let wrap_width = width.saturating_sub(DETAIL_INDENT.len()).max(20);
        lines.extend(
            wrap_words(detail, wrap_width)
                .into_iter()
                .map(|line| format!("{DETAIL_INDENT}{}", theme.fg(ColorToken::TextDim, &line))),
        );
        self.clamp_to_width(lines, width)
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Original: goal-markers.ts buildGoalMarker()
pub fn build_goal_marker(
    change: &GoalChange,
    expanded: bool,
    actor: Option<GoalActor>,
) -> Option<GoalMarkerComponent> {
    if change.kind != GoalChangeKind::Lifecycle {
        return None;
    }
    let (headline, accent) = match change.status? {
        GoalStatus::Paused => (
            paused_headline(change.reason.as_deref(), actor),
            ColorToken::Warning,
        ),
        GoalStatus::Active => (resumed_headline(actor).to_owned(), ColorToken::Primary),
        GoalStatus::Blocked => {
            let mut marker = GoalMarkerComponent::new(
                "Goal blocked",
                change.reason.clone(),
                ColorToken::Warning,
            );
            marker.set_expanded(expanded);
            return Some(marker);
        }
        GoalStatus::Complete => return None,
    };
    let options = GoalMarkerOptions {
        marker: STATUS_BULLET.trim_end().to_owned(),
        text_token: accent,
        expandable: false,
        indent: String::new(),
        leading_blank: true,
    };
    let mut marker = GoalMarkerComponent::with_options(headline, None, accent, options);
    marker.set_expanded(expanded);
    Some(marker)
}

fn paused_headline(reason: Option<&str>, actor: Option<GoalActor>) -> String {
    if reason == Some("Paused after interruption") {
        return "Goal paused due to user's interruption".to_owned();
    }
    if actor == Some(GoalActor::User) {
        return "Goal paused by the user.".to_owned();
    }
    if let Some(reason) = reason.filter(|reason| reason.starts_with("Paused ")) {
        return format!("Goal {}", lowercase_first(reason));
    }
    if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
        return format!("Goal paused: {reason}");
    }
    if actor == Some(GoalActor::Model) {
        return "Goal paused by the agent.".to_owned();
    }
    "Goal paused".to_owned()
}

fn resumed_headline(actor: Option<GoalActor>) -> &'static str {
    match actor {
        Some(GoalActor::User) => "Goal resumed by the user.",
        Some(GoalActor::Model) => "Goal resumed by the agent.",
        _ => "Goal resumed",
    }
}

fn lowercase_first(text: &str) -> String {
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    format!("{}{}", first.to_lowercase(), characters.as_str())
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in normalized {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if candidate.encode_utf16().count() > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current = word.to_owned();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::render::visible_width;

    fn lifecycle(status: GoalStatus, reason: Option<&str>) -> GoalChange {
        GoalChange {
            kind: GoalChangeKind::Lifecycle,
            status: Some(status),
            reason: reason.map(str::to_owned),
            stats: None,
            actor: None,
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
    fn builds_lifecycle_markers_and_skips_completion() {
        for (status, expected) in [
            (GoalStatus::Paused, "Goal paused"),
            (GoalStatus::Active, "Goal resumed"),
            (GoalStatus::Blocked, "Goal blocked"),
        ] {
            let mut marker = build_goal_marker(&lifecycle(status, None), false, None);
            assert!(
                marker.as_mut().is_some_and(
                    |marker| strip_sgr(&marker.render(80).join("\n")).contains(expected)
                )
            );
        }
        let completion = GoalChange {
            kind: GoalChangeKind::Completion,
            status: Some(GoalStatus::Complete),
            reason: None,
            stats: None,
            actor: None,
        };
        assert!(build_goal_marker(&completion, false, None).is_none());
    }

    #[test]
    fn attributes_pause_and_resume_and_avoids_repeated_paused() {
        let mut interrupted = build_goal_marker(
            &lifecycle(GoalStatus::Paused, Some("Paused after interruption")),
            false,
            Some(GoalActor::Runtime),
        );
        assert_eq!(
            interrupted
                .as_mut()
                .map(|marker| strip_sgr(&marker.render(80).join("\n")))
                .as_deref(),
            Some("\n● Goal paused due to user's interruption")
        );
        let mut runtime = build_goal_marker(
            &lifecycle(
                GoalStatus::Paused,
                Some("Paused after runtime error: socket hang up"),
            ),
            false,
            Some(GoalActor::Runtime),
        );
        assert_eq!(
            runtime
                .as_mut()
                .map(|marker| strip_sgr(&marker.render(80).join("\n")))
                .as_deref(),
            Some("\n● Goal paused after runtime error: socket hang up")
        );
        let mut resumed = build_goal_marker(
            &lifecycle(GoalStatus::Active, None),
            false,
            Some(GoalActor::User),
        );
        assert!(resumed.as_mut().is_some_and(|marker| {
            strip_sgr(&marker.render(80).join("\n")).contains("resumed by the user")
        }));
    }

    #[test]
    fn hides_expandable_detail_until_expanded() {
        let mut marker = GoalMarkerComponent::new(
            "Goal: no progress",
            Some("still spinning".to_owned()),
            ColorToken::Warning,
        );
        let collapsed = strip_sgr(&marker.render(80).join("\n"));
        assert!(collapsed.contains("(ctrl+o)"));
        assert!(!collapsed.contains("still spinning"));
        marker.set_expanded(true);
        let expanded = strip_sgr(&marker.render(80).join("\n"));
        assert!(expanded.contains("still spinning"));
        assert!(!expanded.contains("(ctrl+o)"));
    }

    #[test]
    fn clamps_long_markers_and_zero_width() {
        let mut marker = build_goal_marker(
            &lifecycle(
                GoalStatus::Paused,
                Some(&format!(
                    "Paused after provider API error: {}",
                    "x".repeat(200)
                )),
            ),
            false,
            Some(GoalActor::Runtime),
        );
        let lines = marker
            .as_mut()
            .map(|marker| marker.render(80))
            .unwrap_or_default();
        assert!(lines.iter().all(|line| visible_width(line) <= 80));
        assert_eq!(
            marker
                .as_mut()
                .map(|marker| marker.render(0))
                .unwrap_or_default(),
            ["", ""]
        );
    }
}
