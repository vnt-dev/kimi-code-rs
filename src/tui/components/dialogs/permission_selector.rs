use std::any::Any;

use crate::{
    sdk::types::PermissionMode,
    tui::components::{Component, ComponentRole},
};

use super::{ChoiceOption, ChoicePickerComponent, ChoicePickerOptions};

fn permission_options() -> Vec<ChoiceOption> {
    vec![
        ChoiceOption::new("manual", "Manual").with_description("Approve every action yourself."),
        ChoiceOption::new("yolo", "YOLO")
            .with_description("Auto-approve tool actions, but the agent may still ask questions."),
        ChoiceOption::new("auto", "Auto")
            .with_description("Fully autonomous — agent decides everything without asking."),
    ]
}

fn permission_mode_value(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Manual => "manual",
        PermissionMode::Yolo => "yolo",
        PermissionMode::Auto => "auto",
    }
}

fn parse_permission_mode(value: &str) -> Option<PermissionMode> {
    match value {
        "manual" => Some(PermissionMode::Manual),
        "yolo" => Some(PermissionMode::Yolo),
        "auto" => Some(PermissionMode::Auto),
        _ => None,
    }
}

/// SDK permission-mode selector.
///
/// Original: `permission-selector.ts`, `PermissionSelectorComponent`.
pub struct PermissionSelectorComponent {
    picker: ChoicePickerComponent,
}

impl PermissionSelectorComponent {
    pub fn new<S, C>(current_value: PermissionMode, mut on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(PermissionMode) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        let mut options = ChoicePickerOptions::new(
            "Select permission mode",
            permission_options(),
            move |value| {
                if let Some(mode) = parse_permission_mode(&value) {
                    on_select(mode);
                }
            },
            on_cancel,
        );
        options.current_value = Some(permission_mode_value(current_value).to_owned());
        Self {
            picker: ChoicePickerComponent::new(options),
        }
    }

    pub fn selected_mode(&self) -> Option<PermissionMode> {
        self.picker
            .selected()
            .and_then(|option| parse_permission_mode(&option.value))
    }
}

impl Component for PermissionSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.picker.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.picker.handle_input(data);
    }

    fn invalidate(&mut self) {
        self.picker.invalidate();
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
    fn maps_sdk_permission_modes_and_preserves_descriptions() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let callback = Arc::clone(&selected);
        let mut selector = PermissionSelectorComponent::new(
            PermissionMode::Yolo,
            move |mode| callback.lock().expect("selected").push(mode),
            || {},
        );
        assert_eq!(selector.selected_mode(), Some(PermissionMode::Yolo));
        selector.handle_input("\u{1b}[B");
        assert_eq!(selector.selected_mode(), Some(PermissionMode::Auto));
        selector.handle_input("\r");
        assert_eq!(*selected.lock().expect("selected"), [PermissionMode::Auto]);

        let plain = selector
            .render(76)
            .into_iter()
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>();
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Approve every action yourself."))
        );
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Fully autonomous — agent decides everything"))
        );
    }

    #[test]
    fn rejects_values_outside_the_closed_permission_enum() {
        assert_eq!(
            parse_permission_mode("manual"),
            Some(PermissionMode::Manual)
        );
        assert_eq!(parse_permission_mode("yolo"), Some(PermissionMode::Yolo));
        assert_eq!(parse_permission_mode("auto"), Some(PermissionMode::Auto));
        assert_eq!(parse_permission_mode("invalid"), None);
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
