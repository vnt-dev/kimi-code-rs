use std::{any::Any, collections::BTreeMap};

use crate::{
    sdk::model_alias::{ModelAlias, effective_model_alias},
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, visible_width},
        },
        easter_eggs::dance::{is_rainbow_dancing, render_dance_welcome_header},
        theme::{ColorToken, current_theme},
    },
};

const LOGO: [&str; 2] = ["▐█▛█▛█▌", "▐█████▌"];

#[derive(Debug, Clone)]
pub struct WelcomeState {
    pub version: String,
    pub work_dir: String,
    pub session_id: String,
    pub model: String,
    pub available_models: BTreeMap<String, ModelAlias>,
    pub mcp_servers_summary: Option<String>,
}

/// Original: `src/tui/components/chrome/welcome.ts`, `WelcomeComponent`.
pub struct WelcomeComponent {
    state: WelcomeState,
}

impl WelcomeComponent {
    pub fn new(state: WelcomeState) -> Self {
        Self { state }
    }

    pub fn set_state(&mut self, state: WelcomeState) {
        self.state = state;
    }

    fn render_welcome(&self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let logged_out = self.state.model.is_empty();
        let active_model = self
            .state
            .available_models
            .get(&self.state.model)
            .map(|model| effective_model_alias(model, None));
        let model_value = if logged_out {
            "not set, run /login or /provider".to_owned()
        } else {
            active_model
                .as_ref()
                .and_then(|model| model.display_name.clone())
                .or_else(|| active_model.as_ref().map(|model| model.model.clone()))
                .unwrap_or_else(|| self.state.model.clone())
        };
        if width < 24 {
            let title = theme.bold_fg(ColorToken::Primary, "Welcome to Kimi Code!");
            let prompt = if logged_out {
                theme.fg(
                    ColorToken::Warning,
                    "Run /login or /provider to get started.",
                )
            } else {
                theme.fg(ColorToken::TextDim, "Send /help for help information.")
            };
            let model = if logged_out {
                theme.fg(ColorToken::Warning, &model_value)
            } else {
                model_value
            };
            return [String::new(), title, prompt, format!("Model: {model}")]
                .into_iter()
                .map(|line| truncate_to_width(&line, width, "…", false))
                .collect();
        }

        let inner_width = width.saturating_sub(4).max(1);
        let logo_width = LOGO.iter().map(|row| visible_width(row)).max().unwrap_or(0);
        let text_width = inner_width.saturating_sub(logo_width + 2).max(4);
        let first_row = truncate_to_width(
            &theme.bold_fg(ColorToken::Primary, "Welcome to Kimi Code!"),
            text_width,
            "…",
            false,
        );
        let second_row = truncate_to_width(
            &theme.fg(
                ColorToken::TextDim,
                if logged_out {
                    "Run /login or /provider to get started."
                } else {
                    "Send /help for help information."
                },
            ),
            text_width,
            "…",
            false,
        );
        let mut content = if is_rainbow_dancing() {
            render_dance_welcome_header(LOGO, text_width, &second_row)
        } else {
            vec![
                format!(
                    "{}  {first_row}",
                    theme.fg(ColorToken::Primary, &format!("{:<logo_width$}", LOGO[0]))
                ),
                format!(
                    "{}  {second_row}",
                    theme.fg(ColorToken::Primary, &format!("{:<logo_width$}", LOGO[1]))
                ),
            ]
        };
        let label = |text: &str| theme.bold_fg(ColorToken::TextDim, text);
        let model_value = if logged_out {
            theme.fg(ColorToken::Warning, &model_value)
        } else {
            model_value
        };
        content.push(String::new());
        content.extend([
            format!("{}{}", label("Directory: "), self.state.work_dir),
            format!("{}{}", label("Session:   "), self.state.session_id),
            format!("{}{}", label("Model:     "), model_value),
            format!("{}{}", label("Version:   "), self.state.version),
        ]);
        if let Some(summary) = self
            .state
            .mcp_servers_summary
            .as_deref()
            .filter(|summary| !summary.is_empty())
        {
            content.push(format!("{}{}", label("MCP:       "), summary));
        }

        let border = |text: &str| theme.fg(ColorToken::Primary, text);
        let horizontal = "─".repeat(width - 2);
        let mut lines = vec![
            String::new(),
            border(&format!("╭{horizontal}╮")),
            format!("{}{}{}", border("│"), " ".repeat(width - 2), border("│")),
        ];
        for line in content {
            let line = truncate_to_width(&line, inner_width, "…", false);
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

impl Component for WelcomeComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_welcome(width)
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Welcome
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, LazyLock, Mutex};

    use regex::Regex;

    use super::*;
    use crate::tui::easter_eggs::dance::RainbowDance;

    static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn state(model: &str) -> WelcomeState {
        WelcomeState {
            version: "1.2.3".to_owned(),
            work_dir: "/tmp/project".to_owned(),
            session_id: "ses-1".to_owned(),
            model: model.to_owned(),
            available_models: BTreeMap::new(),
            mcp_servers_summary: None,
        }
    }

    fn strip(text: &str) -> String {
        Regex::new(r"\x1b\[[0-9;]*m")
            .expect("ANSI regex")
            .replace_all(text, "")
            .into_owned()
    }

    #[test]
    fn renders_box_logo_information_and_optional_mcp() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let _dance = RainbowDance::new(Arc::new(|| {}));
        let mut state = state("kimi-k2");
        state.mcp_servers_summary = Some("2 connected".to_owned());
        let mut welcome = WelcomeComponent::new(state);
        let output = strip(&welcome.render(80).join("\n"));
        for expected in [
            "▐█▛█▛█▌",
            "Welcome to Kimi Code!",
            "Directory: /tmp/project",
            "Session:   ses-1",
            "Model:     kimi-k2",
            "Version:   1.2.3",
            "MCP:       2 connected",
        ] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
        assert_eq!(welcome.role(), ComponentRole::Welcome);
    }

    #[test]
    fn logged_out_and_narrow_variants_stay_bounded() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let _dance = RainbowDance::new(Arc::new(|| {}));
        let mut welcome = WelcomeComponent::new(state(""));
        for width in [0, 1, 2, 4, 10, 23, 39, 80] {
            assert!(
                welcome
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
        let output = strip(&welcome.render(23).join("\n"));
        assert!(output.contains("Welcome"));
        assert!(output.contains("Run /login"));
    }

    #[test]
    fn rainbow_view_changes_header_colors() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        let mut dance = RainbowDance::new(Arc::new(|| {}));
        let mut welcome = WelcomeComponent::new(state("kimi-k2"));
        let normal = welcome.render(80)[3..5].join("\n");
        dance.start(true);
        let rainbow = welcome.render(80)[3..5].join("\n");
        assert_ne!(normal, rainbow);
        assert!(rainbow.matches("38;2;").count() >= 5);
        dance.stop();
    }
}
