use std::any::Any;

use crate::tui::components::{Component, ComponentRole};

use super::{StartPermissionOption, StartPermissionPromptComponent, StartPermissionPromptOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStartMode {
    Manual,
    Yolo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStartPermissionChoice {
    Auto,
    Yolo,
    Manual,
    Cancel,
}

pub fn goal_start_options(
    mode: GoalStartMode,
) -> Vec<StartPermissionOption<GoalStartPermissionChoice>> {
    let auto = StartPermissionOption::new(
        GoalStartPermissionChoice::Auto,
        "Switch to Auto and start",
        "Best if you want Kimi Code to keep working while you are away. Tools are approved automatically, and questions are skipped.",
    );
    let cancel = StartPermissionOption::new(
        GoalStartPermissionChoice::Cancel,
        "Do not start",
        "Return to the input box with your goal command.",
    );
    match mode {
        GoalStartMode::Manual => vec![
            auto,
            StartPermissionOption::new(
                GoalStartPermissionChoice::Yolo,
                "Switch to YOLO and start",
                "Tools and plan changes are approved automatically. Kimi Code may still ask you questions.",
            ),
            StartPermissionOption::new(
                GoalStartPermissionChoice::Manual,
                "Start in Manual",
                "Keep approvals on. Kimi Code will ask before risky actions, so the goal may stop and wait for you.",
            ),
            cancel,
        ],
        GoalStartMode::Yolo => vec![
            auto,
            StartPermissionOption::new(
                GoalStartPermissionChoice::Yolo,
                "Keep YOLO and start",
                "Tools and plan changes stay approved automatically. Kimi Code may still ask you questions.",
            ),
            cancel,
        ],
    }
}

fn goal_notice_lines(mode: GoalStartMode) -> Vec<String> {
    let lines = match mode {
        GoalStartMode::Manual => [
            "Manual mode asks you before Kimi Code runs commands, edits files, or takes other risky actions.",
            "Manual mode is not suitable for unattended goal work.",
            "You can go back without losing your command.",
        ],
        GoalStartMode::Yolo => [
            "YOLO mode approves tools and plan changes automatically.",
            "YOLO mode can still stop for questions.",
            "Switch to Auto if you want questions skipped during goal work.",
        ],
    };
    lines.into_iter().map(str::to_owned).collect()
}

/// Goal-specific start permission confirmation.
///
/// Original: `goal-start-permission-prompt.ts`,
/// `GoalStartPermissionPromptComponent`.
pub struct GoalStartPermissionPromptComponent {
    prompt: StartPermissionPromptComponent<GoalStartPermissionChoice>,
}

impl GoalStartPermissionPromptComponent {
    pub fn new<S, C>(mode: GoalStartMode, on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(GoalStartPermissionChoice) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        let title = match mode {
            GoalStartMode::Manual => "Start a goal with approvals on?",
            GoalStartMode::Yolo => "Start a goal in YOLO mode?",
        };
        Self {
            prompt: StartPermissionPromptComponent::new(StartPermissionPromptOptions::new(
                title,
                goal_notice_lines(mode),
                goal_start_options(mode),
                on_select,
                on_cancel,
            )),
        }
    }

    pub fn selected_choice(&self) -> Option<GoalStartPermissionChoice> {
        self.prompt.selected().copied()
    }
}

impl Component for GoalStartPermissionPromptComponent {
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
    fn manual_mode_exposes_all_four_choices() {
        let options = goal_start_options(GoalStartMode::Manual);
        assert_eq!(
            options
                .iter()
                .map(|option| option.value)
                .collect::<Vec<_>>(),
            [
                GoalStartPermissionChoice::Auto,
                GoalStartPermissionChoice::Yolo,
                GoalStartPermissionChoice::Manual,
                GoalStartPermissionChoice::Cancel,
            ]
        );
        assert_eq!(options[1].label, "Switch to YOLO and start");
    }

    #[test]
    fn yolo_mode_removes_manual_and_can_return_cancel_choice() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let callback = Arc::clone(&selected);
        let mut prompt = GoalStartPermissionPromptComponent::new(
            GoalStartMode::Yolo,
            move |choice| callback.lock().expect("selected").push(choice),
            || {},
        );
        assert_eq!(
            goal_start_options(GoalStartMode::Yolo)
                .iter()
                .map(|option| option.value)
                .collect::<Vec<_>>(),
            [
                GoalStartPermissionChoice::Auto,
                GoalStartPermissionChoice::Yolo,
                GoalStartPermissionChoice::Cancel,
            ]
        );
        prompt.handle_input("\u{1b}[B");
        prompt.handle_input("\u{1b}[B");
        prompt.handle_input("\r");
        assert_eq!(
            *selected.lock().expect("selected"),
            [GoalStartPermissionChoice::Cancel]
        );

        let plain = prompt
            .render(72)
            .into_iter()
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>();
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Start a goal in YOLO mode?"))
        );
        assert!(
            plain
                .iter()
                .any(|line| line.contains("YOLO mode can still stop for questions."))
        );
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Keep YOLO and start"))
        );
        assert!(!plain.iter().any(|line| line.contains("Start in Manual")));
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
