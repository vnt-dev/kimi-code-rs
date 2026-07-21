use std::any::Any;

use crate::tui::components::{Component, ComponentRole};

use super::{ChoiceOption, ChoicePickerComponent, ChoicePickerOptions};

fn editor_options() -> Vec<ChoiceOption> {
    vec![
        ChoiceOption::new("code --wait", "VS Code (code --wait)"),
        ChoiceOption::new("vim", "Vim"),
        ChoiceOption::new("nvim", "Neovim"),
        ChoiceOption::new("nano", "Nano"),
        ChoiceOption::new("", "Auto-detect ($VISUAL / $EDITOR)"),
    ]
}

/// External-editor command selector.
///
/// Original: `editor-selector.ts`, `EditorSelectorComponent`.
pub struct EditorSelectorComponent {
    picker: ChoicePickerComponent,
}

impl EditorSelectorComponent {
    pub fn new<S, C>(current_value: impl Into<String>, on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(String) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        let mut options = ChoicePickerOptions::new(
            "Select external editor",
            editor_options(),
            on_select,
            on_cancel,
        );
        options.current_value = Some(current_value.into());
        Self {
            picker: ChoicePickerComponent::new(options),
        }
    }

    pub fn selected_command(&self) -> Option<&str> {
        self.picker.selected().map(|option| option.value.as_str())
    }
}

impl Component for EditorSelectorComponent {
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
    fn starts_on_current_editor_and_preserves_empty_auto_detect_value() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let callback = Arc::clone(&selected);
        let mut selector = EditorSelectorComponent::new(
            "nano",
            move |value| callback.lock().expect("selected").push(value),
            || {},
        );
        assert_eq!(selector.selected_command(), Some("nano"));
        selector.handle_input("\u{1b}[B");
        assert_eq!(selector.selected_command(), Some(""));
        selector.handle_input("\r");
        assert_eq!(*selected.lock().expect("selected"), [""]);

        let lines = selector.render(48);
        let plain = lines.iter().map(|line| strip_sgr(line)).collect::<Vec<_>>();
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Auto-detect ($VISUAL / $EDITOR)"))
        );
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
