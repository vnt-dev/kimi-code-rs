use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, Text},
    theme::{ColorToken, current_theme},
};

pub struct StatusMessageComponent {
    text_component: Text,
    content: String,
    color: Option<ColorToken>,
}

impl StatusMessageComponent {
    pub fn new(content: impl Into<String>, color: Option<ColorToken>) -> Self {
        let content = content.into();
        let mut component = Self {
            text_component: Text::new(String::new(), 0, 0),
            content,
            color,
        };
        component.refresh_text();
        component
    }

    /// Original: status-message.ts StatusMessageComponent.updateContent()
    pub fn update_content(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.refresh_text();
    }

    fn refresh_text(&mut self) {
        let theme = current_theme();
        let colored = theme.fg(self.color.unwrap_or(ColorToken::TextDim), &self.content);
        let indented = colored
            .replace('\r', "")
            .split('\n')
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.text_component.set_text(indented);
    }
}

impl Component for StatusMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.text_component.render(width.max(1))
    }

    fn invalidate(&mut self) {
        self.refresh_text();
        self.text_component.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct NoticeMessageComponent {
    title_text: Text,
    detail_text: Option<Text>,
    title: String,
    detail: Option<String>,
}

impl NoticeMessageComponent {
    pub fn new(title: impl Into<String>, detail: Option<String>) -> Self {
        let title = title.into();
        let theme = current_theme();
        let title_text = Text::new(
            format!("  {}", theme.fg(ColorToken::TextStrong, &title)),
            0,
            0,
        );
        let detail_text = detail
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| Text::new(format!("  {}", theme.fg(ColorToken::TextDim, value)), 0, 0));
        Self {
            title_text,
            detail_text,
            title,
            detail,
        }
    }

    fn refresh_text(&mut self) {
        let theme = current_theme();
        self.title_text.set_text(format!(
            "  {}",
            theme.fg(ColorToken::TextStrong, &self.title)
        ));
        if let (Some(text), Some(detail)) = (&mut self.detail_text, &self.detail) {
            text.set_text(format!("  {}", theme.fg(ColorToken::TextDim, detail)));
        }
    }
}

impl Component for NoticeMessageComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let mut lines = vec![String::new()];
        lines.extend(self.title_text.render(width));
        if let Some(detail) = &mut self.detail_text {
            lines.extend(detail.render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        self.refresh_text();
        self.title_text.invalidate();
        if let Some(detail) = &mut self.detail_text {
            detail.invalidate();
        }
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

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut index = 0;
        while index < text.len() {
            if text.as_bytes()[index] == 0x1b && text.as_bytes().get(index + 1) == Some(&b'[') {
                index += 2;
                while index < text.len() && text.as_bytes()[index] != b'm' {
                    index += 1;
                }
                index = (index + 1).min(text.len());
            } else if let Some(character) = text[index..].chars().next() {
                output.push(character);
                index += character.len_utf8();
            } else {
                break;
            }
        }
        output
    }

    #[test]
    fn notice_renders_spacing_title_and_optional_detail() {
        let mut notice = NoticeMessageComponent::new(
            "Plan mode: ON",
            Some("Plan will be created here: /tmp/plans/test-plan.md".to_owned()),
        );
        let lines: Vec<_> = notice
            .render(120)
            .iter()
            .map(|line| strip_sgr(line))
            .collect();
        assert_eq!(lines[0], "");
        assert!(lines[1].contains("Plan mode: ON"));
        assert!(lines[2].contains("Plan will be created here"));

        let mut title_only = NoticeMessageComponent::new("Ready", Some(String::new()));
        assert_eq!(title_only.render(20).len(), 2);
    }

    #[test]
    fn status_strips_carriage_returns_and_indents_every_line() {
        let mut status =
            StatusMessageComponent::new("Error: boom\r\nmore\r", Some(ColorToken::Error));
        let lines: Vec<_> = status
            .render(120)
            .iter()
            .map(|line| strip_sgr(line).trim_end().to_owned())
            .collect();
        assert_eq!(lines, ["  Error: boom", "  more"]);
        assert!(lines.iter().all(|line| !line.contains('\r')));
    }

    #[test]
    fn status_updates_content_in_place() {
        let mut status = StatusMessageComponent::new("first", None);
        assert!(strip_sgr(&status.render(20)[0]).contains("first"));
        status.update_content("second\nthird");
        let lines = status.render(20);
        assert_eq!(lines.len(), 2);
        assert!(strip_sgr(&lines[0]).contains("second"));
        assert!(strip_sgr(&lines[1]).contains("third"));
    }
}
