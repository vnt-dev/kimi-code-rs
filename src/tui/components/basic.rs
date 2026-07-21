use std::any::Any;

use super::{
    core::{Component, ComponentRole},
    render::{visible_width, wrap_text_with_ansi},
};

type BackgroundFn = dyn Fn(&str) -> String + Send;

/// Original:
///   packages/pi-tui/src/components/text.ts
///   Text
pub struct Text {
    text: String,
    padding_x: usize,
    padding_y: usize,
    custom_background: Option<Box<BackgroundFn>>,
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
}

impl Text {
    pub fn new(text: impl Into<String>, padding_x: usize, padding_y: usize) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            custom_background: None,
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.clear_cache();
    }

    pub fn set_custom_background(&mut self, custom_background: Option<Box<BackgroundFn>>) {
        self.custom_background = custom_background;
        self.clear_cache();
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

impl Default for Text {
    fn default() -> Self {
        Self::new(String::new(), 1, 1)
    }
}

impl Component for Text {
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

        let normalized = self.text.replace('\t', "   ");
        let content_width = width
            .saturating_sub(self.padding_x.saturating_mul(2))
            .max(1);
        let left_margin = " ".repeat(self.padding_x);
        let right_margin = left_margin.clone();
        let mut content_lines = Vec::new();
        for line in wrap_text_with_ansi(&normalized, content_width) {
            let line_with_margins = format!("{left_margin}{line}{right_margin}");
            content_lines.push(self.apply_background_and_padding(line_with_margins, width));
        }

        let empty_line = " ".repeat(width);
        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(self.apply_background_and_padding(empty_line.clone(), width));
        }
        result.extend(content_lines);
        for _ in 0..self.padding_y {
            result.push(self.apply_background_and_padding(empty_line.clone(), width));
        }
        self.cache(width, result)
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

impl Text {
    fn apply_background_and_padding(&self, mut line: String, width: usize) -> String {
        let padding = width.saturating_sub(visible_width(&line));
        line.push_str(&" ".repeat(padding));
        self.custom_background
            .as_ref()
            .map_or(line.clone(), |background| background(&line))
    }
}

/// Original:
///   packages/pi-tui/src/components/spacer.ts
///   Spacer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spacer {
    lines: usize,
}

impl Spacer {
    pub fn new(lines: usize) -> Self {
        Self { lines }
    }

    pub fn set_lines(&mut self, lines: usize) {
        self.lines = lines;
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new(1)
    }
}

impl Component for Spacer {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![String::new(); self.lines]
    }

    fn invalidate(&mut self) {}

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

    #[test]
    fn text_wraps_tabs_adds_margins_and_pads_to_width() {
        let mut text = Text::new("hello\tworld", 1, 1);
        let lines = text.render(10);
        assert_eq!(lines.first().map(String::as_str), Some("          "));
        assert_eq!(lines.last().map(String::as_str), Some("          "));
        assert!(lines.iter().all(|line| visible_width(line) == 10));
        assert_eq!(lines[1].trim(), "hello");
        assert_eq!(lines[2].trim(), "world");
    }

    #[test]
    fn text_updates_invalidates_and_skips_blank_content() {
        let mut text = Text::new("first", 0, 0);
        assert_eq!(text.render(8), ["first   "]);
        assert_eq!(text.render(8), ["first   "]);
        text.set_text("second");
        assert_eq!(text.render(8), ["second  "]);
        text.set_text(" \n\t");
        assert!(text.render(8).is_empty());
    }

    #[test]
    fn custom_background_receives_full_padded_line() {
        let mut text = Text::new("ok", 0, 0);
        text.set_custom_background(Some(Box::new(|line| format!("<{line}>"))));
        assert_eq!(text.render(4), ["<ok  >"]);
    }

    #[test]
    fn spacer_renders_configured_empty_lines() {
        let mut spacer = Spacer::new(2);
        assert_eq!(spacer.render(80), ["", ""]);
        spacer.set_lines(1);
        assert_eq!(spacer.render(1), [""]);
    }
}
