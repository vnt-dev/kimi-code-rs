use std::any::Any;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use super::{
    Component, ComponentRole,
    media::code_highlight::highlight_lines,
    render::{truncate_to_width, wrap_text_with_ansi},
};
use crate::tui::theme::{ColorToken, current_theme};

const BULLET: &str = "• ";
const QUOTE_PREFIX: &str = "│ ";
const CODE_PREFIX: &str = "│ ";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MarkdownOptions {
    pub transient: bool,
}

#[derive(Debug, Clone)]
struct CodeBlock {
    language: Option<String>,
    content: String,
}

#[derive(Debug, Clone, Copy)]
struct ListState {
    next_number: Option<u64>,
}

/// Cached, terminal-oriented CommonMark renderer.
///
/// Original infrastructure:
/// `packages/pi-tui/src/components/markdown.ts`, `Markdown`.
#[derive(Debug, Clone)]
pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    options: MarkdownOptions,
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
}

impl Markdown {
    pub fn new(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        options: MarkdownOptions,
    ) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            options,
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.text != text {
            self.text = text;
            self.clear_cache();
        }
    }

    pub fn options(&self) -> MarkdownOptions {
        self.options
    }

    fn clear_cache(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }

    fn cache(&mut self, width: usize, lines: Vec<String>) -> Vec<String> {
        self.cached_text = Some(self.text.clone());
        self.cached_width = Some(width);
        self.cached_lines = Some(lines.clone());
        lines
    }
}

impl Component for Markdown {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.cached_text.as_deref() == Some(&self.text)
            && self.cached_width == Some(width)
            && let Some(lines) = &self.cached_lines
        {
            return lines.clone();
        }
        if self.text.trim().is_empty() {
            return self.cache(width, Vec::new());
        }

        let content_width = width
            .saturating_sub(self.padding_x.saturating_mul(2))
            .max(1);
        let logical_lines = MarkdownRenderer::new(content_width, self.options).render(&self.text);
        let left = " ".repeat(self.padding_x);
        let mut lines = vec![String::new(); self.padding_y];
        for logical_line in logical_lines {
            for line in wrap_text_with_ansi(&logical_line, content_width) {
                lines.push(truncate_to_width(
                    &format!("{left}{line}"),
                    width,
                    "…",
                    false,
                ));
            }
        }
        lines.resize(lines.len() + self.padding_y, String::new());
        self.cache(width, lines)
    }

    fn invalidate(&mut self) {
        self.clear_cache();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct MarkdownRenderer {
    width: usize,
    options: MarkdownOptions,
    lines: Vec<String>,
    current: String,
    quote_depth: usize,
    lists: Vec<ListState>,
    code_block: Option<CodeBlock>,
    link_destinations: Vec<String>,
}

impl MarkdownRenderer {
    fn new(width: usize, options: MarkdownOptions) -> Self {
        Self {
            width,
            options,
            lines: Vec::new(),
            current: String::new(),
            quote_depth: 0,
            lists: Vec::new(),
            code_block: None,
            link_destinations: Vec::new(),
        }
    }

    fn render(mut self, markdown: &str) -> Vec<String> {
        let options = Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES;
        for event in Parser::new_ext(markdown, options) {
            self.event(event);
        }
        self.finish_line();
        while self.lines.last().is_some_and(String::is_empty) {
            self.lines.pop();
        }
        self.lines
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some(code_block) = &mut self.code_block {
            match event {
                Event::End(TagEnd::CodeBlock) => self.finish_code_block(),
                Event::Text(text) | Event::Code(text) => code_block.content.push_str(&text),
                Event::SoftBreak | Event::HardBreak => code_block.content.push('\n'),
                _ => {}
            }
            return;
        }

        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => self
                .current
                .push_str(&current_theme().fg(ColorToken::Primary, &code)),
            Event::Html(html) | Event::InlineHtml(html) => self.push_text(&html),
            Event::SoftBreak => self.current.push(' '),
            Event::HardBreak => self.finish_line(),
            Event::Rule => {
                self.finish_line();
                self.lines
                    .push(current_theme().fg(ColorToken::Border, &"─".repeat(self.width.max(1))));
            }
            Event::TaskListMarker(checked) => {
                self.current.push_str(if checked { "[x] " } else { "[ ] " });
            }
            Event::FootnoteReference(reference) => {
                self.current.push_str(&format!("[^{reference}]"));
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => self.push_text(&math),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.ensure_prefix(),
            Tag::Heading { .. } => {
                self.ensure_prefix();
                self.current.push_str("\x1b[1m");
            }
            Tag::BlockQuote(_) => self.quote_depth += 1,
            Tag::CodeBlock(kind) => {
                self.finish_line();
                let language = match kind {
                    CodeBlockKind::Indented => None,
                    CodeBlockKind::Fenced(info) => info
                        .split_ascii_whitespace()
                        .next()
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned),
                };
                self.code_block = Some(CodeBlock {
                    language,
                    content: String::new(),
                });
            }
            Tag::List(start) => self.lists.push(ListState { next_number: start }),
            Tag::Item => {
                self.finish_line();
                self.ensure_prefix();
                let prefix = self.lists.last_mut().map_or_else(
                    || BULLET.to_owned(),
                    |list| match &mut list.next_number {
                        Some(number) => {
                            let prefix = format!("{number}. ");
                            *number = number.saturating_add(1);
                            prefix
                        }
                        None => BULLET.to_owned(),
                    },
                );
                self.current.push_str(&prefix);
            }
            Tag::Emphasis => self.current.push_str("\x1b[3m"),
            Tag::Strong => self.current.push_str("\x1b[1m"),
            Tag::Strikethrough => self.current.push_str("\x1b[9m"),
            Tag::Link { dest_url, .. } => {
                self.current.push_str("\x1b[4m");
                self.link_destinations.push(dest_url.into_string());
            }
            Tag::Image { dest_url, .. } => {
                self.current.push('[');
                self.link_destinations.push(dest_url.into_string());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.finish_line();
                self.blank_line();
            }
            TagEnd::Heading(_) => {
                self.current.push_str("\x1b[22m");
                self.finish_line();
                self.blank_line();
            }
            TagEnd::BlockQuote(_) => {
                self.finish_line();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.finish_line();
                self.lists.pop();
                self.blank_line();
            }
            TagEnd::Item => self.finish_line(),
            TagEnd::Emphasis => self.current.push_str("\x1b[23m"),
            TagEnd::Strong => self.current.push_str("\x1b[22m"),
            TagEnd::Strikethrough => self.current.push_str("\x1b[29m"),
            TagEnd::Link => {
                self.current.push_str("\x1b[24m");
                if let Some(destination) = self.link_destinations.pop()
                    && !destination.is_empty()
                {
                    self.current.push_str(
                        &current_theme().fg(ColorToken::TextMuted, &format!(" ({destination})")),
                    );
                }
            }
            TagEnd::Image => {
                self.current.push(']');
                if let Some(destination) = self.link_destinations.pop()
                    && !destination.is_empty()
                {
                    self.current.push_str(&format!(" ({destination})"));
                }
            }
            _ => {}
        }
    }

    fn ensure_prefix(&mut self) {
        if self.current.is_empty() && self.quote_depth > 0 {
            self.current.push_str(
                &current_theme().fg(ColorToken::TextDim, &QUOTE_PREFIX.repeat(self.quote_depth)),
            );
        }
    }

    fn push_text(&mut self, text: &str) {
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                self.finish_line();
            }
            self.ensure_prefix();
            self.current.push_str(part);
        }
    }

    fn finish_code_block(&mut self) {
        let Some(code_block) = self.code_block.take() else {
            return;
        };
        let content = code_block.content.trim_end_matches('\n');
        let highlighted = if self.options.transient {
            content.split('\n').map(str::to_owned).collect()
        } else {
            highlight_lines(content, code_block.language.as_deref())
        };
        let prefix = current_theme().fg(ColorToken::TextMuted, CODE_PREFIX);
        self.lines.extend(
            highlighted
                .into_iter()
                .map(|line| format!("{prefix}{line}")),
        );
        self.blank_line();
    }

    fn finish_line(&mut self) {
        if !self.current.is_empty() {
            self.lines.push(std::mem::take(&mut self.current));
        }
    }

    fn blank_line(&mut self) {
        if self.lines.last().is_some_and(|line| !line.is_empty()) {
            self.lines.push(String::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::render::visible_width;

    fn plain(text: &str) -> String {
        let mut result = String::new();
        let mut index = 0;
        while index < text.len() {
            if text.as_bytes()[index] == 0x1b {
                index += 1;
                if text.as_bytes().get(index) == Some(&b'[') {
                    index += 1;
                    while index < text.len() && text.as_bytes()[index] != b'm' {
                        index += 1;
                    }
                    index = (index + 1).min(text.len());
                    continue;
                }
            }
            let character = text[index..].chars().next().unwrap();
            result.push(character);
            index += character.len_utf8();
        }
        result
    }

    #[test]
    fn renders_headings_lists_and_inline_markup() {
        let mut markdown = Markdown::new(
            "### Hello **bold**\n\n- one\n- two\n\n1. first\n2. second",
            0,
            0,
            MarkdownOptions::default(),
        );
        let output = plain(&markdown.render(80).join("\n"));
        assert!(output.contains("Hello bold"));
        assert!(!output.contains("###"));
        assert!(output.contains("• one"));
        assert!(output.contains("1. first"));
        assert!(output.contains("2. second"));
    }

    #[test]
    fn preserves_literal_html_and_link_destinations() {
        let mut markdown = Markdown::new(
            "<hook_result>\n{}\n</hook_result>\n\n[docs](https://example.com)",
            0,
            0,
            MarkdownOptions::default(),
        );
        let output = plain(&markdown.render(80).join("\n"));
        assert!(output.contains("<hook_result>"));
        assert!(output.contains("</hook_result>"));
        assert!(output.contains("docs (https://example.com)"));
    }

    #[test]
    fn final_code_blocks_highlight_and_transient_blocks_stay_plain() {
        let source = "```typescript\nconst value = 'text';\n```";
        let mut final_markdown = Markdown::new(source, 0, 0, MarkdownOptions::default());
        let mut transient_markdown =
            Markdown::new(source, 0, 0, MarkdownOptions { transient: true });
        let final_output = final_markdown.render(80).join("\n");
        let transient_output = transient_markdown.render(80).join("\n");
        assert!(final_output.contains("\x1b[34m"));
        assert!(!transient_output.contains("\x1b[34m"));
        assert!(plain(&final_output).contains("│ const value = 'text';"));
    }

    #[test]
    fn caches_updates_invalidates_and_respects_width() {
        let mut markdown = Markdown::new(
            "a fairly long line with words",
            1,
            0,
            MarkdownOptions::default(),
        );
        let first = markdown.render(10);
        assert_eq!(first, markdown.render(10));
        assert!(first.iter().all(|line| visible_width(line) <= 10));

        markdown.set_text("replacement");
        assert!(plain(&markdown.render(20).join("\n")).contains("replacement"));
        markdown.invalidate();
        assert!(markdown.cached_lines.is_none());
    }
}
