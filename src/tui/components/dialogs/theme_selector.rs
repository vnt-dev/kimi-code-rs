use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole},
    theme::custom_theme_loader::list_custom_themes_sync,
};

use super::{ChoiceOption, ChoicePickerComponent, ChoicePickerOptions};

fn theme_options(custom_themes: impl IntoIterator<Item = String>) -> Vec<ChoiceOption> {
    let mut options = vec![
        ChoiceOption::new("auto", "Auto (match terminal)"),
        ChoiceOption::new("dark", "Dark"),
        ChoiceOption::new("light", "Light"),
    ];
    options.extend(
        custom_themes
            .into_iter()
            .map(|name| ChoiceOption::new(name.clone(), format!("Custom: {name}"))),
    );
    options
}

/// Built-in and custom theme selector.
///
/// Original: `theme-selector.ts`, `ThemeSelectorComponent`.
pub struct ThemeSelectorComponent {
    picker: ChoicePickerComponent,
}

impl ThemeSelectorComponent {
    pub fn new<S, C>(current_value: impl Into<String>, on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(String) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self::new_with_custom_themes(
            current_value,
            list_custom_themes_sync(),
            on_select,
            on_cancel,
        )
    }

    pub fn new_with_custom_themes<S, C>(
        current_value: impl Into<String>,
        custom_themes: Vec<String>,
        on_select: S,
        on_cancel: C,
    ) -> Self
    where
        S: FnMut(String) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        let mut options = ChoicePickerOptions::new(
            "Select theme",
            theme_options(custom_themes),
            on_select,
            on_cancel,
        );
        options.current_value = Some(current_value.into());
        Self {
            picker: ChoicePickerComponent::new(options),
        }
    }

    pub fn selected_theme(&self) -> Option<&str> {
        self.picker.selected().map(|option| option.value.as_str())
    }
}

impl Component for ThemeSelectorComponent {
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
    fn appends_custom_themes_and_returns_unmodified_theme_name() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let callback = Arc::clone(&selected);
        let mut selector = ThemeSelectorComponent::new_with_custom_themes(
            "ocean",
            vec!["ocean".to_owned(), "warm".to_owned()],
            move |value| callback.lock().expect("selected").push(value),
            || {},
        );
        assert_eq!(selector.selected_theme(), Some("ocean"));
        let plain = selector
            .render(48)
            .into_iter()
            .map(|line| strip_sgr(&line))
            .collect::<Vec<_>>();
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Auto (match terminal)"))
        );
        assert!(plain.iter().any(|line| line.contains("Custom: ocean")));
        assert!(plain.iter().any(|line| line.contains("Custom: warm")));

        selector.handle_input("\u{1b}[B");
        selector.handle_input("\r");
        assert_eq!(*selected.lock().expect("selected"), ["warm"]);
    }

    #[test]
    fn built_in_themes_keep_original_order() {
        let options = theme_options(Vec::new());
        assert_eq!(
            options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["auto", "dark", "light"]
        );
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
