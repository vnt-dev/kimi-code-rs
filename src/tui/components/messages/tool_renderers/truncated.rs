use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, Text, render::truncate_to_width},
    theme::{ColorToken, current_theme},
    types::{ToolCallBlockData, ToolResultBlockData},
};

pub use super::types::PREVIEW_LINES;
use super::types::{RenderedComponents, RendererContext};

const DEFAULT_INDENT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TruncatedOutputOptions {
    pub expanded: bool,
    pub is_error: bool,
    pub max_lines: Option<usize>,
    pub indent: Option<usize>,
    pub expand_hint: Option<bool>,
    pub tail: Option<bool>,
    pub color: Option<ColorToken>,
}

pub fn trim_trailing_empty_lines(lines: &[String]) -> Vec<String> {
    let end = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(0, |index| index + 1);
    lines[..end].to_vec()
}

pub struct TruncatedOutputComponent {
    text_component: Text,
    expanded: bool,
    max_lines: usize,
    indent: usize,
    expand_hint: bool,
    tail: bool,
}

impl TruncatedOutputComponent {
    pub fn new(output: &str, options: TruncatedOutputOptions) -> Self {
        let split = output.split('\n').map(str::to_owned).collect::<Vec<_>>();
        let cleaned = trim_trailing_empty_lines(&split).join("\n");
        let color = if options.is_error {
            ColorToken::Error
        } else {
            options.color.unwrap_or(ColorToken::TextDim)
        };
        let indent = options.indent.unwrap_or(DEFAULT_INDENT);
        Self {
            text_component: Text::new(current_theme().fg(color, &cleaned), indent, 0),
            expanded: options.expanded,
            max_lines: options.max_lines.unwrap_or(PREVIEW_LINES),
            indent,
            expand_hint: options.expand_hint.unwrap_or(true),
            tail: options.tail.unwrap_or(false),
        }
    }

    fn render_hint(&self, width: usize, hint: &str) -> String {
        let indent_width = self.indent.min(width);
        let hint_width = width.saturating_sub(indent_width);
        format!(
            "{}{}",
            " ".repeat(indent_width),
            current_theme().dim(&truncate_to_width(hint, hint_width, "…", false))
        )
    }
}

impl Component for TruncatedOutputComponent {
    /// Original: tool-renderers/truncated.ts TruncatedOutputComponent.render()
    fn render(&mut self, width: usize) -> Vec<String> {
        let content_lines = self.text_component.render(width);
        if self.expanded || content_lines.len() <= self.max_lines {
            return content_lines;
        }

        let remaining = content_lines.len() - self.max_lines;
        if self.tail {
            let mut lines =
                vec![self.render_hint(width, &format!("... ({remaining} earlier lines)"))];
            lines.extend(content_lines.into_iter().skip(remaining));
            return lines;
        }

        let mut lines = content_lines
            .into_iter()
            .take(self.max_lines)
            .collect::<Vec<_>>();
        let hint = if self.expand_hint {
            format!("... ({remaining} more lines, ctrl+o to expand)")
        } else {
            format!("... ({remaining} more lines)")
        };
        lines.push(self.render_hint(width, &hint));
        lines
    }

    fn invalidate(&mut self) {
        self.text_component.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Original: tool-renderers/truncated.ts renderTruncated()
pub fn render_truncated(
    _tool_call: &ToolCallBlockData,
    result: &ToolResultBlockData,
    context: RendererContext,
) -> RenderedComponents {
    if result.output.is_empty() {
        return Vec::new();
    }
    vec![Box::new(TruncatedOutputComponent::new(
        &result.output,
        TruncatedOutputOptions {
            expanded: context.expanded,
            is_error: result.is_error.unwrap_or(false),
            ..TruncatedOutputOptions::default()
        },
    ))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::render::visible_width;

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
    fn trims_only_trailing_empty_lines() {
        let lines = vec!["a".to_owned(), String::new(), "b".to_owned(), String::new()];
        assert_eq!(trim_trailing_empty_lines(&lines), ["a", "", "b"]);
        assert!(trim_trailing_empty_lines(&[String::new()]).is_empty());
    }

    #[test]
    fn indents_content_and_collapsed_hint() {
        let mut component = TruncatedOutputComponent::new(
            "a\nb\nc\nd\ne",
            TruncatedOutputOptions {
                max_lines: Some(2),
                indent: Some(6),
                ..TruncatedOutputOptions::default()
            },
        );
        let lines = component
            .render(80)
            .iter()
            .map(|line| strip_sgr(line).trim_end().to_owned())
            .collect::<Vec<_>>();
        assert!(lines[0].starts_with("      a"));
        assert!(lines[1].starts_with("      b"));
        assert_eq!(lines[2], "      ... (3 more lines, ctrl+o to expand)");
    }

    #[test]
    fn supports_tail_no_expand_hint_and_expanded_modes() {
        let mut tail = TruncatedOutputComponent::new(
            "a\nb\nc\nd",
            TruncatedOutputOptions {
                max_lines: Some(2),
                tail: Some(true),
                ..TruncatedOutputOptions::default()
            },
        );
        let tail_text = tail
            .render(60)
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert!(tail_text[0].contains("2 earlier lines"));
        assert!(tail_text[1].contains('c'));
        assert!(tail_text[2].contains('d'));

        let mut fixed = TruncatedOutputComponent::new(
            "a\nb\nc\nd",
            TruncatedOutputOptions {
                max_lines: Some(2),
                expand_hint: Some(false),
                ..TruncatedOutputOptions::default()
            },
        );
        assert!(strip_sgr(&fixed.render(60)[2]).contains("2 more lines)"));
        assert!(!strip_sgr(&fixed.render(60)[2]).contains("ctrl+o"));

        let mut expanded = TruncatedOutputComponent::new(
            "a\nb\nc\nd",
            TruncatedOutputOptions {
                expanded: true,
                max_lines: Some(2),
                ..TruncatedOutputOptions::default()
            },
        );
        assert_eq!(expanded.render(60).len(), 4);
    }

    #[test]
    fn truncates_wrapped_visual_lines_and_fits_hint_width() {
        let mut component = TruncatedOutputComponent::new(
            &"x".repeat(500),
            TruncatedOutputOptions {
                max_lines: Some(3),
                ..TruncatedOutputOptions::default()
            },
        );
        let lines = component.render(20);
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|line| visible_width(line) <= 20));
        assert!(strip_sgr(&lines[3]).contains("... ("));
    }

    #[test]
    fn preserves_literal_system_and_image_tags() {
        let mut component = TruncatedOutputComponent::new(
            "<system>literal text from a user file</system>\n<image path=\"/tmp/x.png\">",
            TruncatedOutputOptions {
                expanded: true,
                ..TruncatedOutputOptions::default()
            },
        );
        let output = strip_sgr(&component.render(80).join("\n"));
        assert!(output.contains("<system>literal text from a user file</system>"));
        assert!(output.contains("<image path=\"/tmp/x.png\">"));
    }
}
