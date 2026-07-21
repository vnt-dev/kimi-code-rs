use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, Text},
    theme::{ColorToken, current_theme},
    types::SkillActivationTrigger,
};

use super::activation_preview::args_preview;

pub struct SkillActivationComponent {
    head_text: Text,
    preview_text: Option<Text>,
    name: String,
    args: Option<String>,
    pub trigger: Option<SkillActivationTrigger>,
}

impl SkillActivationComponent {
    pub fn new(
        name: impl Into<String>,
        args: Option<String>,
        trigger: Option<SkillActivationTrigger>,
    ) -> Self {
        let name = name.into();
        let head_text = Text::new(render_head(&name), 0, 0);
        let preview_text =
            args_preview(args.as_deref()).map(|preview| Text::new(render_preview(&preview), 0, 0));
        Self {
            head_text,
            preview_text,
            name,
            args,
            trigger,
        }
    }
}

impl Component for SkillActivationComponent {
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
        self.head_text.set_text(render_head(&self.name));
        self.head_text.invalidate();
        if let (Some(text), Some(preview)) =
            (&mut self.preview_text, args_preview(self.args.as_deref()))
        {
            text.set_text(render_preview(&preview));
            text.invalidate();
        }
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::SkillActivation
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn render_head(name: &str) -> String {
    let theme = current_theme();
    format!(
        "{}{}",
        theme.bold_fg(ColorToken::Primary, "▶ Activated skill: "),
        theme.bold_fg(ColorToken::RoleUser, name)
    )
}

fn render_preview(preview: &str) -> String {
    format!("  {}", current_theme().fg(ColorToken::TextDim, preview))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::render::visible_width;

    fn plain(text: &str) -> String {
        let mut result = String::new();
        let mut in_escape = false;
        for character in text.chars() {
            if character == '\u{1b}' {
                in_escape = true;
            } else if in_escape && character == 'm' {
                in_escape = false;
            } else if !in_escape {
                result.push(character);
            }
        }
        result
    }

    #[test]
    fn renders_skill_name_args_and_role() {
        let mut component = SkillActivationComponent::new(
            "review",
            Some("  src/lib.rs  ".to_owned()),
            Some(SkillActivationTrigger::UserSlash),
        );
        let lines = component.render(80);
        assert_eq!(plain(&lines[0]), "");
        assert!(plain(&lines[1]).contains("▶ Activated skill: review"));
        assert!(plain(&lines[2]).contains("  src/lib.rs"));
        assert_eq!(component.role(), ComponentRole::SkillActivation);
        assert_eq!(component.trigger, Some(SkillActivationTrigger::UserSlash));
        assert!(lines.iter().all(|line| visible_width(line) <= 80));
    }

    #[test]
    fn omits_blank_args() {
        let mut component = SkillActivationComponent::new("review", Some("  ".to_owned()), None);
        assert_eq!(component.render(40).len(), 2);
    }
}
