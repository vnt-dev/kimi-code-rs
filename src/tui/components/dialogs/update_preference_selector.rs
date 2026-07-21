use std::any::Any;

use crate::tui::components::{Component, ComponentRole};

use super::{ChoiceOption, ChoicePickerComponent, ChoicePickerOptions};

/// Boolean automatic-update preference selector.
///
/// Original: `update-preference-selector.ts`,
/// `UpdatePreferenceSelectorComponent`.
pub struct UpdatePreferenceSelectorComponent {
    picker: ChoicePickerComponent,
}

impl UpdatePreferenceSelectorComponent {
    pub fn new<S, C>(current_value: bool, mut on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(bool) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        let options = vec![
            ChoiceOption::new("on", "On")
                .with_description("Install new versions in the background."),
            ChoiceOption::new("off", "Off").with_description("Show the install prompt instead."),
        ];
        let mut picker_options = ChoicePickerOptions::new(
            "Automatic updates",
            options,
            move |value| on_select(value == "on"),
            on_cancel,
        );
        picker_options.current_value = Some(if current_value { "on" } else { "off" }.to_owned());
        Self {
            picker: ChoicePickerComponent::new(picker_options),
        }
    }

    pub fn selected_value(&self) -> Option<bool> {
        self.picker.selected().map(|option| option.value == "on")
    }
}

impl Component for UpdatePreferenceSelectorComponent {
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
    fn maps_current_and_selected_string_values_to_booleans() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let callback = Arc::clone(&selected);
        let mut selector = UpdatePreferenceSelectorComponent::new(
            true,
            move |value| callback.lock().expect("selected").push(value),
            || {},
        );
        assert_eq!(selector.selected_value(), Some(true));
        selector.handle_input("\u{1b}[B");
        assert_eq!(selector.selected_value(), Some(false));
        selector.handle_input("\r");
        assert_eq!(*selected.lock().expect("selected"), [false]);

        let plain = selector
            .render(52)
            .into_iter()
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>();
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Install new versions in the background."))
        );
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Show the install prompt instead."))
        );
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
