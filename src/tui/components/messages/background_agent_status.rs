use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width, wrap_text_with_ansi},
    },
    theme::{ColorToken, current_theme},
    types::{BackgroundAgentStatusData, BackgroundAgentStatusPhase},
};

const MESSAGE_INDENT: &str = "  ";
const STATUS_BULLET: &str = "● ";
const FAILURE_MARK: &str = "✗ ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundAgentStatusComponent {
    data: BackgroundAgentStatusData,
}

impl BackgroundAgentStatusComponent {
    pub fn new(data: BackgroundAgentStatusData) -> Self {
        Self { data }
    }
}

impl Component for BackgroundAgentStatusComponent {
    /// Original: background-agent-status.ts BackgroundAgentStatusComponent.render()
    fn render(&mut self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }

        let tone = match self.data.phase {
            BackgroundAgentStatusPhase::Started => ColorToken::Primary,
            BackgroundAgentStatusPhase::Completed => ColorToken::Success,
            BackgroundAgentStatusPhase::Failed => ColorToken::Error,
        };
        let marker = if self.data.phase == BackgroundAgentStatusPhase::Failed {
            FAILURE_MARK
        } else {
            STATUS_BULLET
        };
        let theme = current_theme();
        let bullet = theme.fg(tone, marker);
        let mut text = theme.fg(tone, &self.data.headline);
        if let Some(detail) = self
            .data
            .detail
            .as_deref()
            .filter(|detail| !detail.is_empty())
        {
            text.push_str(&theme.fg(ColorToken::TextDim, &format!(" ({detail})")));
        }

        let content_width = width.saturating_sub(MESSAGE_INDENT.len()).max(1);
        let mut result = vec![String::new()];
        if !text.trim().is_empty() {
            for (index, mut line) in wrap_text_with_ansi(&text.replace('\t', "   "), content_width)
                .into_iter()
                .enumerate()
            {
                let padding = content_width.saturating_sub(visible_width(&line));
                line.push_str(&" ".repeat(padding));
                let prefix = if index == 0 { &bullet } else { MESSAGE_INDENT };
                result.push(truncate_to_width(
                    &format!("{prefix}{line}"),
                    width,
                    "…",
                    false,
                ));
            }
        }
        result
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::BackgroundAgentStatus
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(
        phase: BackgroundAgentStatusPhase,
        headline: &str,
        detail: Option<&str>,
    ) -> BackgroundAgentStatusData {
        BackgroundAgentStatusData {
            phase,
            headline: headline.to_owned(),
            detail: detail.map(str::to_owned),
        }
    }

    fn strip_sgr(text: &str) -> String {
        let mut result = String::new();
        let mut index = 0;
        while index < text.len() {
            if text.as_bytes()[index] == 0x1b && text.as_bytes().get(index + 1) == Some(&b'[') {
                index += 2;
                while index < text.len() && text.as_bytes()[index] != b'm' {
                    index += 1;
                }
                index = (index + 1).min(text.len());
            } else if let Some(character) = text[index..].chars().next() {
                result.push(character);
                index += character.len_utf8();
            } else {
                break;
            }
        }
        result
    }

    #[test]
    fn renders_started_completed_and_failed_markers() {
        let cases = [
            (
                BackgroundAgentStatusPhase::Started,
                "explore agent started in background",
                "Explore project structure",
                "● explore agent started in background (Explore project structure)",
            ),
            (
                BackgroundAgentStatusPhase::Completed,
                "explore agent completed in background",
                "Explore project structure",
                "● explore agent completed in background (Explore project structure)",
            ),
            (
                BackgroundAgentStatusPhase::Failed,
                "explore agent failed in background",
                "Explore project structure · boom",
                "✗ explore agent failed in background (Explore project structure · boom)",
            ),
        ];
        for (phase, headline, detail, expected) in cases {
            let mut component =
                BackgroundAgentStatusComponent::new(data(phase, headline, Some(detail)));
            let rendered = component.render(120);
            assert_eq!(rendered[0], "");
            assert_eq!(strip_sgr(&rendered[1]).trim_end(), expected);
            assert_eq!(component.role(), ComponentRole::BackgroundAgentStatus);
        }
    }

    #[test]
    fn wraps_and_keeps_status_lines_within_narrow_widths() {
        let mut component = BackgroundAgentStatusComponent::new(data(
            BackgroundAgentStatusPhase::Started,
            "explore agent started in background",
            Some("Explore project structure"),
        ));
        for width in [0, 1, 2, 4, 10, 39] {
            let lines = component.render(width);
            assert!(lines.iter().all(|line| visible_width(line) <= width));
            if width >= 4 {
                assert!(lines.len() > 2);
            }
        }
    }

    #[test]
    fn omits_empty_detail() {
        let mut component = BackgroundAgentStatusComponent::new(data(
            BackgroundAgentStatusPhase::Completed,
            "done",
            Some(""),
        ));
        let rendered = component.render(20);
        assert_eq!(strip_sgr(&rendered[1]).trim_end(), "● done");
    }
}
