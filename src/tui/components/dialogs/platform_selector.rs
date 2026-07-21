use std::any::Any;

use crate::{
    oauth::open_platform::OPEN_PLATFORMS,
    tui::components::{Component, ComponentRole},
};

use super::{ChoiceOption, ChoicePickerComponent, ChoicePickerOptions};

/// Platform choice used by the authentication flow.
///
/// Original: `platform-selector.ts`, `PlatformSelectorComponent`.
pub struct PlatformSelectorComponent {
    picker: ChoicePickerComponent,
}

impl PlatformSelectorComponent {
    pub fn new<S, C>(on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(String) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        let mut options = vec![ChoiceOption::new("kimi-code", "Kimi Code (OAuth)")];
        options.extend(
            OPEN_PLATFORMS
                .iter()
                .map(|platform| ChoiceOption::new(platform.id, platform.name)),
        );
        Self {
            picker: ChoicePickerComponent::new(ChoicePickerOptions::new(
                "Select a platform",
                options,
                on_select,
                on_cancel,
            )),
        }
    }

    pub fn selected_platform_id(&self) -> Option<&str> {
        self.picker.selected().map(|option| option.value.as_str())
    }
}

impl Component for PlatformSelectorComponent {
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

    use crate::tui::components::render::visible_width;

    use super::*;

    #[test]
    fn lists_managed_then_open_platforms_and_returns_platform_id() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let selected_callback = Arc::clone(&selected);
        let mut selector = PlatformSelectorComponent::new(
            move |value| selected_callback.lock().expect("selected").push(value),
            || {},
        );

        assert_eq!(selector.selected_platform_id(), Some("kimi-code"));
        let lines = selector.render(72);
        let plain = lines.iter().map(|line| strip_sgr(line)).collect::<Vec<_>>();
        assert!(plain.iter().any(|line| line.contains("Kimi Code (OAuth)")));
        for platform in OPEN_PLATFORMS {
            assert!(plain.iter().any(|line| line.contains(platform.name)));
        }
        assert!(lines.iter().all(|line| visible_width(line) <= 72));

        selector.handle_input("\u{1b}[B");
        selector.handle_input("\r");
        assert_eq!(*selected.lock().expect("selected"), [OPEN_PLATFORMS[0].id]);
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
