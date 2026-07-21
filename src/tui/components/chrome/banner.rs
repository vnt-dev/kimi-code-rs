use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{visible_width, wrap_text_with_ansi},
    },
    theme::{ColorToken, current_theme},
    types::BannerState,
};

const PREFIX_STAR: &str = "✦";
const PADDING: &str = " ";

/// Original: `src/tui/components/chrome/banner.ts`, `BannerComponent`.
pub struct BannerComponent {
    state: BannerState,
}

impl BannerComponent {
    pub fn new(state: BannerState) -> Self {
        Self { state }
    }

    fn render_banner(&self, width: usize) -> Vec<String> {
        if width < 1 {
            return vec![String::new()];
        }
        let theme = current_theme();
        let tag_text = self.state.tag.as_deref().unwrap_or_default();
        let tag_label = if tag_text.is_empty() {
            String::new()
        } else {
            format!("{PREFIX_STAR} {tag_text}")
        };
        let tag_styled = if tag_label.is_empty() {
            String::new()
        } else {
            theme.bold_fg(ColorToken::Primary, &tag_label)
        };
        let tag_display = if tag_styled.is_empty() {
            String::new()
        } else {
            format!("{tag_styled}{PADDING}")
        };
        let tag_width = visible_width(&tag_display);
        let show_tag = tag_width > 0 && tag_width < width;
        let body_indent = if show_tag {
            " ".repeat(tag_width)
        } else {
            String::new()
        };
        let description_indent_width = if show_tag {
            visible_width(&format!("{PREFIX_STAR}{PADDING}"))
        } else {
            0
        };
        let description_indent = " ".repeat(description_indent_width);
        let body_width = width.saturating_sub(if show_tag { tag_width } else { 0 });
        let description_width = width.saturating_sub(description_indent_width);
        if body_width == 0 {
            return vec![String::new()];
        }

        let mut result = Vec::new();
        for (segment_index, segment) in self.state.main_text.split('\n').enumerate() {
            for (line_index, line) in wrap_text_with_ansi(segment, body_width)
                .into_iter()
                .enumerate()
            {
                let line = theme.bold_fg(ColorToken::TextStrong, &line);
                if segment_index == 0 && line_index == 0 && show_tag {
                    result.push(format!("{tag_display}{line}"));
                } else {
                    result.push(format!("{body_indent}{line}"));
                }
            }
        }
        if let Some(sub_text) = self.state.sub_text.as_deref() {
            for segment in sub_text.split('\n') {
                let available = if description_width == 0 {
                    body_width
                } else {
                    description_width
                };
                for line in wrap_text_with_ansi(segment, available) {
                    result.push(format!(
                        "{description_indent}{}",
                        theme.fg(ColorToken::TextDim, &line)
                    ));
                }
            }
        }
        result.push(String::new());
        result
    }
}

impl Component for BannerComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_banner(width)
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
    use regex::Regex;

    use super::*;
    use crate::tui::{components::render::visible_width, types::BannerDisplay};

    fn state(tag: Option<&str>, main: &str, sub: Option<&str>) -> BannerState {
        BannerState {
            key: "component-banner".to_owned(),
            tag: tag.map(str::to_owned),
            main_text: main.to_owned(),
            sub_text: sub.map(str::to_owned),
            display: BannerDisplay::Always,
            ttl_hours: None,
        }
    }

    fn strip_terminal(text: &str) -> String {
        Regex::new(r"\x1b\[[0-9;]*m")
            .expect("ANSI regex")
            .replace_all(text, "")
            .into_owned()
    }

    #[test]
    fn renders_tag_main_subtext_and_trailing_blank() {
        let mut banner = BannerComponent::new(state(
            Some("What's new:"),
            "This is the main banner message for testing purposes.",
            Some("This is a short subtext line."),
        ));
        let lines = banner.render(80);
        let plain = lines
            .iter()
            .map(|line| strip_terminal(line))
            .collect::<Vec<_>>();
        assert_eq!(plain.len(), 3);
        assert!(plain[0].contains("✦ What's new: This is the main banner"));
        assert!(plain[1].contains("This is a short subtext"));
        assert_eq!(plain[2], "");
        assert!(!plain[0].contains("What's new::"));
    }

    #[test]
    fn wraps_and_aligns_main_continuations_and_subtext() {
        let mut banner = BannerComponent::new(state(
            Some("New:"),
            "Line 1 with a lot of content",
            Some("Sub text"),
        ));
        let lines = banner.render(20);
        let plain = lines
            .iter()
            .map(|line| strip_terminal(line))
            .collect::<Vec<_>>();
        assert!(plain.iter().all(|line| visible_width(line) <= 20));
        assert!(plain[0].contains("✦ New:"));
        assert_eq!(
            plain.iter().filter(|line| line.contains("✦ New:")).count(),
            1
        );
        let main_start = plain[0].find("Line 1").expect("main text");
        let continuation = plain
            .iter()
            .find(|line| line.contains("lot of"))
            .expect("continuation");
        assert_eq!(
            visible_width(&continuation[..continuation.find("lot of").expect("text")]),
            visible_width(&plain[0][..main_start])
        );
        let sub = plain
            .iter()
            .find(|line| line.contains("Sub text"))
            .expect("subtext");
        assert_eq!(
            visible_width(&sub[..sub.find("Sub text").expect("text")]),
            2
        );
    }

    #[test]
    fn drops_unfittable_tag_and_handles_narrow_widths() {
        let source = state(
            Some("What's new:"),
            "This is the main banner message",
            Some("details"),
        );
        for width in [0, 1, 2, 3, 5, 10] {
            let mut banner = BannerComponent::new(source.clone());
            let lines = banner.render(width);
            assert!(!lines.is_empty());
            assert!(lines.iter().all(|line| visible_width(line) <= width));
            if width == 5 {
                let plain = strip_terminal(&lines.join("\n"));
                assert!(!plain.contains("✦"));
                assert!(!plain.contains("What's new"));
            }
        }
    }

    #[test]
    fn supports_no_tag_no_subtext_and_explicit_newlines() {
        let mut banner = BannerComponent::new(state(None, "Line 1\nLine 2", None));
        let plain = banner
            .render(80)
            .iter()
            .map(|line| strip_terminal(line))
            .collect::<Vec<_>>();
        assert_eq!(plain, ["Line 1", "Line 2", ""]);
    }
}
