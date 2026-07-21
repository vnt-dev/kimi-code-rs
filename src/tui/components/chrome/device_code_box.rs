use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    theme::{ColorToken, current_theme},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCodeBoxParams {
    pub title: String,
    pub url: String,
    pub code: String,
    pub hint: Option<String>,
}

/// OAuth device-code panel rendered inside the transcript.
///
/// Original: `src/tui/components/chrome/device-code-box.ts`.
pub struct DeviceCodeBoxComponent {
    params: DeviceCodeBoxParams,
}

impl DeviceCodeBoxComponent {
    pub fn new(params: DeviceCodeBoxParams) -> Self {
        Self { params }
    }

    fn render_box(&self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }
        let theme = current_theme();
        let inner_width = width.saturating_sub(4).max(1);
        let title = truncate_to_width(
            &theme.bold_fg(ColorToken::TextStrong, &self.params.title),
            inner_width,
            "…",
            false,
        );
        let prompt = truncate_to_width(
            &theme.fg(
                ColorToken::TextDim,
                "Visit the URL below in your browser to authorize:",
            ),
            inner_width,
            "…",
            false,
        );
        let url = truncate_to_width(
            &theme.fg(ColorToken::Primary, &self.params.url),
            inner_width,
            "…",
            false,
        );
        let code = truncate_to_width(
            &format!(
                "{}{}",
                theme.bold_fg(ColorToken::TextDim, "Verification code:  "),
                theme.bold_fg(ColorToken::Accent, &self.params.code)
            ),
            inner_width,
            "…",
            false,
        );
        let mut content = vec![title, String::new(), prompt, url, String::new(), code];
        if let Some(hint) = self.params.hint.as_deref().filter(|hint| !hint.is_empty()) {
            content.push(String::new());
            content.push(truncate_to_width(
                &theme.fg(ColorToken::TextDim, hint),
                inner_width,
                "…",
                false,
            ));
        }
        if width < 4 {
            let mut lines = vec![String::new()];
            lines.extend(
                content
                    .into_iter()
                    .map(|line| truncate_to_width(&line, width, "…", false)),
            );
            return lines;
        }

        let border = |text: &str| theme.fg(ColorToken::Primary, text);
        let horizontal = "─".repeat(width - 2);
        let mut lines = vec![
            String::new(),
            border(&format!("╭{horizontal}╮")),
            format!("{}{}{}", border("│"), " ".repeat(width - 2), border("│")),
        ];
        for line in content {
            let right_padding = inner_width.saturating_sub(visible_width(&line));
            lines.push(format!(
                "{}  {line}{}{}",
                border("│"),
                " ".repeat(right_padding),
                border("│")
            ));
        }
        lines.push(format!(
            "{}{}{}",
            border("│"),
            " ".repeat(width - 2),
            border("│")
        ));
        lines.push(border(&format!("╰{horizontal}╯")));
        lines.push(String::new());
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }
}

impl Component for DeviceCodeBoxComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_box(width)
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

    fn component(hint: Option<&str>) -> DeviceCodeBoxComponent {
        DeviceCodeBoxComponent::new(DeviceCodeBoxParams {
            title: "Sign in to Kimi Code".to_owned(),
            url: "https://www.kimi.com/code/authorize_device?user_code=N32D-W3YD".to_owned(),
            code: "N32D-W3YD".to_owned(),
            hint: hint.map(str::to_owned),
        })
    }

    fn strip(text: &str) -> String {
        Regex::new(r"\x1b\[[0-9;]*m")
            .expect("ANSI regex")
            .replace_all(text, "")
            .into_owned()
    }

    #[test]
    fn frames_title_url_code_and_hint() {
        let mut component = component(Some("Press Ctrl-C to cancel"));
        let lines = component
            .render(80)
            .iter()
            .map(|line| strip(line))
            .collect::<Vec<_>>();
        let joined = lines.join("\n");
        assert!(lines[1].starts_with('╭') && lines[1].ends_with('╮'));
        assert!(lines[lines.len() - 2].starts_with('╰') && lines[lines.len() - 2].ends_with('╯'));
        for expected in [
            "Sign in to Kimi Code",
            "https://www.kimi.com",
            "N32D-W3YD",
            "Press Ctrl-C to cancel",
            "Verification code",
        ] {
            assert!(joined.contains(expected));
        }
    }

    #[test]
    fn truncates_url_and_omits_absent_hint() {
        let mut component = component(None);
        let lines = component
            .render(40)
            .iter()
            .map(|line| strip(line))
            .collect::<Vec<_>>();
        let url = lines
            .iter()
            .find(|line| line.contains("https://"))
            .expect("URL line");
        assert!(url.contains('…'));
        assert!(!lines.join("\n").contains("Press Ctrl-C"));
    }

    #[test]
    fn bounds_every_line_at_narrow_widths() {
        let mut component = component(Some("Press Ctrl-C to cancel"));
        for width in [0, 1, 2, 3, 4, 10, 20, 39] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }
}
