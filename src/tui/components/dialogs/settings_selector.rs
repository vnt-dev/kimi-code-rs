use std::any::Any;

use crate::tui::components::{Component, ComponentRole};

use super::{ChoiceOption, ChoicePickerComponent, ChoicePickerOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSelection {
    Model,
    Theme,
    Editor,
    Permission,
    Experiments,
    Upgrade,
    Usage,
}

impl SettingsSelection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Theme => "theme",
            Self::Editor => "editor",
            Self::Permission => "permission",
            Self::Experiments => "experiments",
            Self::Upgrade => "upgrade",
            Self::Usage => "usage",
        }
    }
}

fn parse_settings_selection(value: &str) -> Option<SettingsSelection> {
    match value {
        "model" => Some(SettingsSelection::Model),
        "theme" => Some(SettingsSelection::Theme),
        "editor" => Some(SettingsSelection::Editor),
        "permission" => Some(SettingsSelection::Permission),
        "experiments" => Some(SettingsSelection::Experiments),
        "upgrade" => Some(SettingsSelection::Upgrade),
        "usage" => Some(SettingsSelection::Usage),
        _ => None,
    }
}

fn settings_options() -> Vec<ChoiceOption> {
    [
        (
            SettingsSelection::Model,
            "Model",
            "Switch the active model and thinking mode.",
        ),
        (
            SettingsSelection::Permission,
            "Permission",
            "Choose how tool actions are approved.",
        ),
        (
            SettingsSelection::Theme,
            "Theme",
            "Change the terminal UI theme.",
        ),
        (
            SettingsSelection::Editor,
            "Editor",
            "Set the external editor command.",
        ),
        (
            SettingsSelection::Experiments,
            "Experiments",
            "Turn experimental features on or off.",
        ),
        (
            SettingsSelection::Upgrade,
            "Automatic updates",
            "Turn automatic CLI updates on or off.",
        ),
        (
            SettingsSelection::Usage,
            "Usage",
            "Show session tokens, context window, and plan quotas.",
        ),
    ]
    .into_iter()
    .map(|(value, label, description)| {
        ChoiceOption::new(value.as_str(), label).with_description(description)
    })
    .collect()
}

/// Root settings-dialog selector.
///
/// Original: `settings-selector.ts`, `SettingsSelectorComponent`.
pub struct SettingsSelectorComponent {
    picker: ChoicePickerComponent,
}

impl SettingsSelectorComponent {
    pub fn new<S, C>(mut on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(SettingsSelection) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        let options = ChoicePickerOptions::new(
            "Settings",
            settings_options(),
            move |value| {
                if let Some(selection) = parse_settings_selection(&value) {
                    on_select(selection);
                }
            },
            on_cancel,
        );
        Self {
            picker: ChoicePickerComponent::new(options),
        }
    }

    pub fn selected_setting(&self) -> Option<SettingsSelection> {
        self.picker
            .selected()
            .and_then(|option| parse_settings_selection(&option.value))
    }
}

impl Component for SettingsSelectorComponent {
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
    fn preserves_setting_order_and_dispatches_typed_selection() {
        assert_eq!(
            settings_options()
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            [
                "model",
                "permission",
                "theme",
                "editor",
                "experiments",
                "upgrade",
                "usage"
            ]
        );

        let selected = Arc::new(Mutex::new(Vec::new()));
        let callback = Arc::clone(&selected);
        let mut selector = SettingsSelectorComponent::new(
            move |value| callback.lock().expect("selected").push(value),
            || {},
        );
        for _ in 0..4 {
            selector.handle_input("\u{1b}[B");
        }
        assert_eq!(
            selector.selected_setting(),
            Some(SettingsSelection::Experiments)
        );
        selector.handle_input("\r");
        assert_eq!(
            *selected.lock().expect("selected"),
            [SettingsSelection::Experiments]
        );
    }

    #[test]
    fn closed_selection_parser_rejects_unknown_values() {
        for selection in [
            SettingsSelection::Model,
            SettingsSelection::Theme,
            SettingsSelection::Editor,
            SettingsSelection::Permission,
            SettingsSelection::Experiments,
            SettingsSelection::Upgrade,
            SettingsSelection::Usage,
        ] {
            assert_eq!(
                parse_settings_selection(selection.as_str()),
                Some(selection)
            );
        }
        assert_eq!(parse_settings_selection("other"), None);
    }
}
