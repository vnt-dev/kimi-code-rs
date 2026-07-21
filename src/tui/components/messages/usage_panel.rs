use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    theme::{ColorToken, current_theme},
};

const LEFT_MARGIN: usize = 2;
const SIDE_PADDING: usize = 1;
const BOX_OVERHEAD: usize = LEFT_MARGIN + 2 + 2 * SIDE_PADDING;

type LineBuilder = dyn Fn() -> Vec<String> + Send;

pub struct UsagePanelComponent {
    build_lines: Box<LineBuilder>,
    border_token: ColorToken,
    title: String,
    lines: Vec<String>,
}

impl UsagePanelComponent {
    pub fn new(
        build_lines: impl Fn() -> Vec<String> + Send + 'static,
        border_token: ColorToken,
        title: impl Into<String>,
    ) -> Self {
        let build_lines: Box<LineBuilder> = Box::new(build_lines);
        let lines = build_lines();
        Self {
            build_lines,
            border_token,
            title: title.into(),
            lines,
        }
    }

    pub fn usage(build_lines: impl Fn() -> Vec<String> + Send + 'static) -> Self {
        Self::new(build_lines, ColorToken::Primary, " Usage ")
    }
}

impl Component for UsagePanelComponent {
    /// Original: usage-panel.ts UsagePanelComponent.render()
    fn render(&mut self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }
        if width < BOX_OVERHEAD + 1 {
            let mut output = vec![truncate_to_width(self.title.trim(), width, "…", false)];
            output.extend(
                self.lines
                    .iter()
                    .map(|line| truncate_to_width(line, width, "…", false)),
            );
            return output;
        }

        let available_interior = width - BOX_OVERHEAD;
        let longest_line = self
            .lines
            .iter()
            .map(|line| visible_width(line))
            .max()
            .unwrap_or(0);
        let content_width = available_interior
            .min(longest_line.max(visible_width(&self.title)))
            .max(1);
        let horizontal_length = content_width + 2 * SIDE_PADDING;
        let title = truncate_to_width(&self.title, horizontal_length, "…", false);
        let trailing_dashes = horizontal_length.saturating_sub(visible_width(&title));
        let theme = current_theme();
        let paint = |text: &str| theme.fg(self.border_token, text);
        let indent = " ".repeat(LEFT_MARGIN);
        let mut output = vec![format!(
            "{indent}{}{}{}{}",
            paint("╭"),
            paint(&title),
            paint(&"─".repeat(trailing_dashes)),
            paint("╮")
        )];
        for line in &self.lines {
            let clipped = if visible_width(line) > content_width {
                truncate_to_width(line, content_width, "…", false)
            } else {
                line.clone()
            };
            let padding = content_width.saturating_sub(visible_width(&clipped));
            output.push(format!(
                "{indent}{} {clipped}{} {}",
                paint("│"),
                " ".repeat(padding),
                paint("│")
            ));
        }
        output.push(format!(
            "{indent}{}",
            paint(&format!("╰{}╯", "─".repeat(horizontal_length)))
        ));
        output
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }

    fn invalidate(&mut self) {
        self.lines = (self.build_lines)();
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
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

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
    fn wraps_lines_in_a_titled_bordered_panel() {
        let mut component = UsagePanelComponent::usage(|| vec!["Session usage".to_owned()]);
        let output = component
            .render(80)
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert!(output[0].contains(" Usage "));
        assert!(output[1].contains("Session usage"));
        assert!(output.last().is_some_and(|line| line.contains('╰')));
    }

    #[test]
    fn truncates_long_lines_and_handles_every_narrow_width() {
        let mut component = UsagePanelComponent::usage(|| {
            vec![format!("error: {}", "x".repeat(200)), "second".to_owned()]
        });
        for width in [60, 39, 24, 20, 10, 4, 1, 0] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }

    #[test]
    fn invalidate_rebuilds_cached_body_lines() {
        let count = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&count);
        let mut component = UsagePanelComponent::usage(move || {
            vec![format!(
                "build={}",
                captured.fetch_add(1, Ordering::Relaxed)
            )]
        });
        assert!(
            component
                .render(80)
                .iter()
                .any(|line| strip_sgr(line).contains("build=0"))
        );
        component.invalidate();
        assert!(
            component
                .render(80)
                .iter()
                .any(|line| strip_sgr(line).contains("build=1"))
        );
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }
}
