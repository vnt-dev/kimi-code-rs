use std::any::Any;

use crate::tui::components::{Component, ComponentRole};

use super::{StartPermissionOption, StartPermissionPromptComponent, StartPermissionPromptOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwarmStartPermissionChoice {
    Auto,
    Yolo,
    Manual,
}

fn swarm_options() -> Vec<StartPermissionOption<SwarmStartPermissionChoice>> {
    vec![
        StartPermissionOption::new(
            SwarmStartPermissionChoice::Auto,
            "Switch to Auto and start",
            "Best for swarm tasks. Tools are approved automatically, and questions are skipped.",
        ),
        StartPermissionOption::new(
            SwarmStartPermissionChoice::Yolo,
            "Switch to YOLO and start",
            "Tools and plan changes are approved automatically. Kimi Code may still ask you questions.",
        ),
        StartPermissionOption::new(
            SwarmStartPermissionChoice::Manual,
            "Start in Manual",
            "Keep approvals on. Kimi Code may stop and wait for you during the swarm task.",
        ),
    ]
}

fn notice_lines() -> Vec<String> {
    [
        "Manual mode asks you before Kimi Code runs commands, edits files, or takes other risky actions.",
        "Manual mode can block swarm work while agents are running.",
        "You can go back without losing your command.",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Swarm-specific start permission confirmation.
///
/// Original: `swarm-start-permission-prompt.ts`,
/// `SwarmStartPermissionPromptComponent`.
pub struct SwarmStartPermissionPromptComponent {
    prompt: StartPermissionPromptComponent<SwarmStartPermissionChoice>,
}

impl SwarmStartPermissionPromptComponent {
    pub fn new<S, C>(on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(SwarmStartPermissionChoice) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            prompt: StartPermissionPromptComponent::new(StartPermissionPromptOptions::new(
                "Start a swarm task with approvals on?",
                notice_lines(),
                swarm_options(),
                on_select,
                on_cancel,
            )),
        }
    }

    pub fn selected_choice(&self) -> Option<SwarmStartPermissionChoice> {
        self.prompt.selected().copied()
    }
}

impl Component for SwarmStartPermissionPromptComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.prompt.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.prompt.handle_input(data);
    }

    fn invalidate(&mut self) {
        self.prompt.invalidate();
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
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn preserves_swarm_choices_copy_and_risk_notice() {
        assert_eq!(
            swarm_options()
                .iter()
                .map(|option| option.value)
                .collect::<Vec<_>>(),
            [
                SwarmStartPermissionChoice::Auto,
                SwarmStartPermissionChoice::Yolo,
                SwarmStartPermissionChoice::Manual
            ]
        );
        let selected = Arc::new(Mutex::new(Vec::new()));
        let callback = Arc::clone(&selected);
        let mut prompt = SwarmStartPermissionPromptComponent::new(
            move |choice| callback.lock().expect("selected").push(choice),
            || {},
        );
        prompt.handle_input("\u{1b}[B");
        prompt.handle_input("\u{1b}[B");
        assert_eq!(
            prompt.selected_choice(),
            Some(SwarmStartPermissionChoice::Manual)
        );
        prompt.handle_input("\r");
        assert_eq!(
            *selected.lock().expect("selected"),
            [SwarmStartPermissionChoice::Manual]
        );

        let plain = prompt
            .render(72)
            .into_iter()
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>();
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Manual mode can block swarm work"))
        );
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Best for swarm tasks."))
        );
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
