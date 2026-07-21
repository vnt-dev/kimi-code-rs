use std::any::Any;

use crate::tui::components::{
    Component, ComponentRole,
    dialogs::choice_picker::{
        ChoiceOption, ChoicePickerComponent, ChoicePickerOptions, ChoiceTone, NoticeTone,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRemoveConfirmResult {
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginInstallTrustConfirmResult {
    Confirm,
    Cancel,
}

/// Original: `plugins-selector.ts`, `PluginRemoveConfirmComponent`.
pub struct PluginRemoveConfirmComponent {
    inner: ChoicePickerComponent,
}

impl PluginRemoveConfirmComponent {
    pub fn new<D>(id: impl Into<String>, display_name: impl Into<String>, on_done: D) -> Self
    where
        D: FnMut(PluginRemoveConfirmResult) + Send + 'static,
    {
        let id = id.into();
        let display_name = display_name.into();
        let callback = std::sync::Arc::new(std::sync::Mutex::new(on_done));
        let selected = std::sync::Arc::clone(&callback);
        let cancelled = std::sync::Arc::clone(&callback);
        let mut options = ChoicePickerOptions::new(
            format!("Remove {display_name} ({id})?"),
            vec![
                ChoiceOption::new("cancel", "Cancel")
                    .with_description("Keep this plugin installed."),
                ChoiceOption::new("remove", "Remove plugin")
                    .with_tone(ChoiceTone::Danger)
                    .with_description(
                        "Remove only the install record; plugin files are left in place.",
                    ),
            ],
            move |value| {
                invoke(
                    &selected,
                    if value == "remove" {
                        PluginRemoveConfirmResult::Confirm
                    } else {
                        PluginRemoveConfirmResult::Cancel
                    },
                );
            },
            move || invoke(&cancelled, PluginRemoveConfirmResult::Cancel),
        );
        options.hint = Some("↑↓ navigate · Enter/Space select · ← Esc cancel".to_owned());
        Self {
            inner: ChoicePickerComponent::new(options),
        }
    }
}

/// Original: `plugins-selector.ts`, `PluginInstallTrustConfirmComponent`.
pub struct PluginInstallTrustConfirmComponent {
    inner: ChoicePickerComponent,
}

impl PluginInstallTrustConfirmComponent {
    pub fn new<D>(label: impl Into<String>, on_done: D) -> Self
    where
        D: FnMut(PluginInstallTrustConfirmResult) + Send + 'static,
    {
        let callback = std::sync::Arc::new(std::sync::Mutex::new(on_done));
        let selected = std::sync::Arc::clone(&callback);
        let cancelled = std::sync::Arc::clone(&callback);
        let mut options = ChoicePickerOptions::new(
            format!("Install third-party plugin {}?", label.into()),
            vec![
                ChoiceOption::new("exit", "Exit").with_description("Cancel the installation."),
                ChoiceOption::new("trust", "Trust and install")
                    .with_tone(ChoiceTone::Danger)
                    .with_description("Install this third-party plugin anyway."),
            ],
            move |value| {
                invoke(
                    &selected,
                    if value == "trust" {
                        PluginInstallTrustConfirmResult::Confirm
                    } else {
                        PluginInstallTrustConfirmResult::Cancel
                    },
                );
            },
            move || invoke(&cancelled, PluginInstallTrustConfirmResult::Cancel),
        );
        options.hint = Some("↑↓ navigate · Enter/Space select · ← Esc cancel".to_owned());
        options.notice = Some(
            "⚠️ This is a third-party plugin that Kimi has not reviewed. It can bundle MCP servers, skills, or files that run code and access your workspace. Install it only if you trust the source."
                .to_owned(),
        );
        options.notice_tone = NoticeTone::Warning;
        Self {
            inner: ChoicePickerComponent::new(options),
        }
    }
}

fn invoke<T: Copy>(callback: &std::sync::Arc<std::sync::Mutex<impl FnMut(T)>>, value: T) {
    let mut callback = callback
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    callback(value);
}

macro_rules! delegate_component {
    ($type:ty) => {
        impl Component for $type {
            fn render(&mut self, width: usize) -> Vec<String> {
                self.inner.render(width)
            }
            fn handle_input(&mut self, data: &str) {
                self.inner.handle_input(data);
            }
            fn invalidate(&mut self) {
                self.inner.invalidate();
            }
            fn role(&self) -> ComponentRole {
                ComponentRole::Other
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
    };
}

delegate_component!(PluginRemoveConfirmComponent);
delegate_component!(PluginInstallTrustConfirmComponent);

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn remove_defaults_to_cancel_and_requires_navigation_to_confirm() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::clone(&events);
        let mut dialog = PluginRemoveConfirmComponent::new("plug", "Plugin", move |result| {
            called.lock().expect("events").push(result);
        });
        dialog.handle_input("\r");
        dialog.handle_input("\u{1b}[B");
        dialog.handle_input(" ");
        assert_eq!(
            *events.lock().expect("events"),
            [
                PluginRemoveConfirmResult::Cancel,
                PluginRemoveConfirmResult::Confirm
            ]
        );
    }

    #[test]
    fn trust_defaults_to_exit_renders_warning_and_escape_cancels() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::clone(&events);
        let mut dialog = PluginInstallTrustConfirmComponent::new("Example", move |result| {
            called.lock().expect("events").push(result);
        });
        let plain = dialog
            .render(50)
            .iter()
            .map(|line| strip(line))
            .collect::<Vec<_>>();
        let rendered = plain
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(rendered.contains("not reviewed"));
        dialog.handle_input("\u{1b}");
        assert_eq!(
            *events.lock().expect("events"),
            [PluginInstallTrustConfirmResult::Cancel]
        );
    }

    fn strip(text: &str) -> String {
        regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("regex")
            .replace_all(text, "")
            .into_owned()
    }
}
