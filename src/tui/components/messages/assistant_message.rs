use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        markdown::{Markdown, MarkdownOptions},
        render::{truncate_to_width, visible_width},
    },
    theme::{ColorToken, current_theme},
    utils::render_cache::is_render_cache_enabled,
};

const MESSAGE_INDENT: &str = "  ";
const STATUS_BULLET: &str = "● ";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssistantMarkdownOptions {
    pub transient: bool,
}

/// Markdown-backed assistant transcript message.
///
/// Original:
/// `src/tui/components/messages/assistant-message.ts`,
/// `AssistantMessageComponent`.
pub struct AssistantMessageComponent {
    markdown: Option<Markdown>,
    markdown_transient: bool,
    last_text: String,
    last_transient: bool,
    show_bullet: bool,
    render_cache: Option<(usize, Vec<String>)>,
}

impl Default for AssistantMessageComponent {
    fn default() -> Self {
        Self::new(true)
    }
}

impl AssistantMessageComponent {
    pub fn new(show_bullet: bool) -> Self {
        Self {
            markdown: None,
            markdown_transient: false,
            last_text: String::new(),
            last_transient: false,
            show_bullet,
            render_cache: None,
        }
    }

    pub fn set_show_bullet(&mut self, show: bool) {
        if self.show_bullet != show {
            self.show_bullet = show;
            self.mark_render_dirty();
        }
    }

    // Original: AssistantMessageComponent.updateContent().
    pub fn update_content(&mut self, text: &str, options: AssistantMarkdownOptions) {
        let display_text = text.trim();
        let transient = options.transient;
        if display_text == self.last_text && transient == self.last_transient {
            return;
        }

        self.last_text = display_text.to_owned();
        self.last_transient = transient;
        self.mark_render_dirty();
        if display_text.is_empty() {
            self.markdown = None;
            self.markdown_transient = false;
            return;
        }

        if self.markdown.is_none() || self.markdown_transient != transient {
            self.markdown = Some(Markdown::new(
                display_text,
                0,
                0,
                MarkdownOptions { transient },
            ));
            self.markdown_transient = transient;
        } else if let Some(markdown) = &mut self.markdown {
            markdown.set_text(display_text);
        }
    }

    fn mark_render_dirty(&mut self) {
        self.render_cache = None;
    }
}

impl Component for AssistantMessageComponent {
    // Original: AssistantMessageComponent.render().
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.last_text.trim().is_empty() {
            return Vec::new();
        }
        if width == 0 {
            return vec![String::new()];
        }
        if is_render_cache_enabled()
            && let Some((cached_width, lines)) = &self.render_cache
            && *cached_width == width
        {
            return lines.clone();
        }

        let prefix = if self.show_bullet {
            STATUS_BULLET
        } else {
            MESSAGE_INDENT
        };
        let content_width = width.saturating_sub(visible_width(prefix)).max(1);
        let content_lines = self
            .markdown
            .as_mut()
            .map_or_else(Vec::new, |markdown| markdown.render(content_width));

        let mut lines = vec![String::new()];
        for (index, content_line) in content_lines.into_iter().enumerate() {
            let line_prefix = if index == 0 && self.show_bullet {
                current_theme().fg(ColorToken::Text, STATUS_BULLET)
            } else {
                MESSAGE_INDENT.to_owned()
            };
            lines.push(truncate_to_width(
                &format!("{line_prefix}{content_line}"),
                width,
                "…",
                false,
            ));
        }
        if is_render_cache_enabled() {
            self.render_cache = Some((width, lines.clone()));
        }
        lines
    }

    // Original: AssistantMessageComponent.invalidate().
    fn invalidate(&mut self) {
        self.mark_render_dirty();
        self.markdown = (!self.last_text.trim().is_empty()).then(|| {
            Markdown::new(
                self.last_text.trim(),
                0,
                0,
                MarkdownOptions {
                    transient: self.last_transient,
                },
            )
        });
        self.markdown_transient = self.last_transient;
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::AssistantMessage
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use regex::Regex;
    use std::sync::LazyLock;

    use super::*;

    fn strip(text: &str) -> String {
        static SGR: LazyLock<Regex> =
            LazyLock::new(|| Regex::new("\\x1b\\[[0-9;]*m").expect("valid SGR regex"));
        SGR.replace_all(text, "").into_owned()
    }

    #[test]
    fn uses_stable_bullet_without_stealing_content_width() {
        assert_eq!(visible_width(STATUS_BULLET), 2);
        let mut component = AssistantMessageComponent::default();
        component.update_content("abcdef", AssistantMarkdownOptions::default());
        let lines = component
            .render(8)
            .into_iter()
            .map(|line| strip(&line))
            .collect::<Vec<_>>();
        assert_eq!(lines, ["", "● abcdef"]);
        assert_eq!(visible_width(&lines[1]), 8);
    }

    #[test]
    fn keeps_lines_within_every_narrow_width() {
        let mut component = AssistantMessageComponent::default();
        component.update_content("abcdef", AssistantMarkdownOptions::default());
        for width in [1, 2, 4, 10, 39] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }

    #[test]
    fn preserves_literal_hook_result_xml() {
        let mut component = AssistantMessageComponent::default();
        component.update_content(
            "<hook_result hook_event=\"UserPromptSubmit\">\n{}\n</hook_result>",
            AssistantMarkdownOptions::default(),
        );
        let output = strip(&component.render(80).join("\n"));
        assert!(output.contains("<hook_result hook_event=\"UserPromptSubmit\">"));
        assert!(output.contains("</hook_result>"));
    }

    #[test]
    fn reuses_markdown_for_text_updates_but_rebuilds_for_transient_changes() {
        let mut component = AssistantMessageComponent::default();
        component.update_content("hello", AssistantMarkdownOptions::default());
        let initial_options = component.markdown.as_ref().unwrap().options();
        component.update_content("hello world", AssistantMarkdownOptions::default());
        assert_eq!(
            component.markdown.as_ref().unwrap().options(),
            initial_options
        );

        component.update_content(
            "```ts\nconst x = 1\n```",
            AssistantMarkdownOptions { transient: true },
        );
        assert!(component.markdown.as_ref().unwrap().options().transient);
        component.update_content(
            "```ts\nconst x = 1\n```",
            AssistantMarkdownOptions { transient: false },
        );
        assert!(!component.markdown.as_ref().unwrap().options().transient);
        assert!(component.render(80).join("\n").contains("\x1b[34m"));
    }

    #[test]
    fn hides_bullet_caches_and_invalidates() {
        let mut component = AssistantMessageComponent::new(false);
        component.update_content("hello", AssistantMarkdownOptions::default());
        let first = component.render(20);
        assert_eq!(strip(&first[1]), "  hello");
        assert_eq!(first, component.render(20));
        component.invalidate();
        assert!(component.render_cache.is_none());
        assert_eq!(first, component.render(20));
    }
}
