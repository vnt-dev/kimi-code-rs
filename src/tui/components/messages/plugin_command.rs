use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, Text},
    theme::{ColorToken, current_theme},
};

use super::activation_preview::args_preview;

pub struct PluginCommandComponent {
    head_text: Text,
    preview_text: Option<Text>,
    label: String,
    args: Option<String>,
}

impl PluginCommandComponent {
    pub fn new(
        plugin_id: impl AsRef<str>,
        command_name: impl AsRef<str>,
        args: Option<String>,
    ) -> Self {
        let label = format!("/{}:{}", plugin_id.as_ref(), command_name.as_ref());
        let head_text = Text::new(render_head(&label), 0, 0);
        let preview_text =
            args_preview(args.as_deref()).map(|preview| Text::new(render_preview(&preview), 0, 0));
        Self {
            head_text,
            preview_text,
            label,
            args,
        }
    }
}

impl Component for PluginCommandComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let mut lines = vec![String::new()];
        lines.extend(self.head_text.render(width));
        if let Some(preview) = &mut self.preview_text {
            lines.extend(preview.render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        self.head_text.set_text(render_head(&self.label));
        self.head_text.invalidate();
        if let (Some(text), Some(preview)) =
            (&mut self.preview_text, args_preview(self.args.as_deref()))
        {
            text.set_text(render_preview(&preview));
            text.invalidate();
        }
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::PluginCommand
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn render_head(label: &str) -> String {
    let theme = current_theme();
    format!(
        "{}{}",
        theme.bold_fg(ColorToken::Primary, "▶ Invoked command: "),
        theme.bold_fg(ColorToken::RoleUser, label)
    )
}

fn render_preview(preview: &str) -> String {
    format!("  {}", current_theme().fg(ColorToken::TextDim, preview))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::render::{visible_width, wrap_text_with_ansi};

    fn visible_text(text: &str) -> String {
        wrap_text_with_ansi(text, usize::MAX)
            .join("")
            .chars()
            .fold(
                (String::new(), false),
                |(mut output, mut escape), character| {
                    if character == '\u{1b}' {
                        escape = true;
                    } else if escape && character == 'm' {
                        escape = false;
                    } else if !escape {
                        output.push(character);
                    }
                    (output, escape)
                },
            )
            .0
    }

    #[test]
    fn renders_namespaced_label_preview_and_role() {
        let mut component =
            PluginCommandComponent::new("deploy", "prod", Some(" --force ".to_owned()));
        let lines = component.render(80);
        assert_eq!(lines[0], "");
        assert!(visible_text(&lines[1]).contains("▶ Invoked command: /deploy:prod"));
        assert!(visible_text(&lines[2]).contains("  --force"));
        assert_eq!(component.role(), ComponentRole::PluginCommand);
        assert!(lines.iter().all(|line| visible_width(line) <= 80));
    }

    #[test]
    fn truncates_long_preview_and_omits_missing_args() {
        let mut long = PluginCommandComponent::new("p", "c", Some("x".repeat(201)));
        let rendered = long.render(240);
        assert!(visible_text(&rendered[2]).trim_end().ends_with('…'));

        let mut absent = PluginCommandComponent::new("p", "c", None);
        assert_eq!(absent.render(40).len(), 2);
    }
}
