use std::any::Any;

use crate::{
    sdk::types::ThinkingEffort,
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, wrap_text_with_ansi},
        },
        keys::{EditorKey, matches_editor_key},
        theme::{ColorToken, current_theme},
    },
};

type EffortCallback = dyn FnMut(ThinkingEffort) + Send;
type CancelCallback = dyn FnMut() + Send;

pub struct EffortSelectorOptions {
    pub title: Option<String>,
    pub efforts: Vec<ThinkingEffort>,
    pub current_value: ThinkingEffort,
    pub warning: Option<String>,
    on_select: Box<EffortCallback>,
    on_session_only_select: Option<Box<EffortCallback>>,
    on_cancel: Box<CancelCallback>,
}

impl EffortSelectorOptions {
    pub fn new<S, C>(
        efforts: Vec<ThinkingEffort>,
        current_value: ThinkingEffort,
        on_select: S,
        on_cancel: C,
    ) -> Self
    where
        S: FnMut(ThinkingEffort) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            title: None,
            efforts,
            current_value,
            warning: None,
            on_select: Box::new(on_select),
            on_session_only_select: None,
            on_cancel: Box::new(on_cancel),
        }
    }

    pub fn with_session_only_select<S>(mut self, callback: S) -> Self
    where
        S: FnMut(ThinkingEffort) + Send + 'static,
    {
        self.on_session_only_select = Some(Box::new(callback));
        self
    }
}

/// Horizontal thinking-effort selector.
///
/// Original: `effort-selector.ts`, `EffortSelectorComponent`.
pub struct EffortSelectorComponent {
    pub focused: bool,
    options: EffortSelectorOptions,
    active_index: usize,
}

impl EffortSelectorComponent {
    pub fn new(options: EffortSelectorOptions) -> Self {
        let active_index = options
            .efforts
            .iter()
            .position(|effort| effort == &options.current_value)
            .unwrap_or_default();
        Self {
            focused: false,
            options,
            active_index,
        }
    }

    pub fn selected_effort(&self) -> Option<&ThinkingEffort> {
        self.options.efforts.get(self.active_index)
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape) {
            (self.options.on_cancel)();
            return;
        }
        if matches_editor_key(data, EditorKey::Left) {
            self.active_index = self.active_index.saturating_sub(1);
            return;
        }
        if matches_editor_key(data, EditorKey::Right) {
            self.active_index = self
                .active_index
                .saturating_add(1)
                .min(self.options.efforts.len().saturating_sub(1));
            return;
        }
        if matches_editor_key(data, EditorKey::Alt('s')) {
            if let Some(effort) = self.selected_effort().cloned()
                && let Some(callback) = &mut self.options.on_session_only_select
            {
                callback(effort);
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Enter)
            && let Some(effort) = self.selected_effort().cloned()
        {
            (self.options.on_select)(effort);
        }
    }

    fn render_selector(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let mut hint_parts = vec!["←→ switch", "Enter select"];
        if self.options.on_session_only_select.is_some() {
            hint_parts.push("Alt+S session-only");
        }
        hint_parts.push("Esc cancel");
        let mut lines = vec![
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            current_theme().bold_fg(
                ColorToken::Primary,
                &format!(
                    " {}",
                    self.options
                        .title
                        .as_deref()
                        .unwrap_or("Select thinking effort")
                ),
            ),
            current_theme().fg(
                ColorToken::TextMuted,
                &format!(" {}", hint_parts.join(" · ")),
            ),
        ];
        if let Some(warning) = &self.options.warning {
            for line in wrap_text_with_ansi(warning, width.saturating_sub(1).max(1)) {
                lines.push(current_theme().fg(ColorToken::Warning, &format!(" {line}")));
            }
        }
        lines.push(String::new());
        let segments = self
            .options
            .efforts
            .iter()
            .enumerate()
            .map(|(index, effort)| {
                let label = effort_label(effort);
                if index == self.active_index {
                    current_theme().bold_fg(ColorToken::Primary, &format!("[ {label} ]"))
                } else {
                    current_theme().fg(ColorToken::Text, &format!("  {label}  "))
                }
            })
            .collect::<Vec<_>>();
        lines.push(format!("  {}", segments.join("  ")));
        lines.push(String::new());
        lines.push(current_theme().fg(ColorToken::Primary, &"─".repeat(width)));
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "", false))
            .collect()
    }
}

impl Component for EffortSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_selector(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Original: `model-selector.ts`, `effortLabel()`.
pub fn effort_label(effort: &ThinkingEffort) -> String {
    let value = effort.as_str();
    let Some(first) = value.chars().next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), &value[first.len_utf8()..])
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tui::components::render::visible_width;

    use super::*;

    #[test]
    fn switches_commits_and_supports_session_only_selection() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let selected = Arc::clone(&events);
        let session = Arc::clone(&events);
        let options = EffortSelectorOptions::new(
            ["off", "low", "high", "max"]
                .into_iter()
                .map(ThinkingEffort::from)
                .collect(),
            ThinkingEffort::from("low"),
            move |effort| {
                selected
                    .lock()
                    .expect("events")
                    .push(format!("select:{}", effort.as_str()));
            },
            || {},
        )
        .with_session_only_select(move |effort| {
            session
                .lock()
                .expect("events")
                .push(format!("session:{}", effort.as_str()));
        });
        let mut selector = EffortSelectorComponent::new(options);
        assert_eq!(
            selector.selected_effort().map(ThinkingEffort::as_str),
            Some("low")
        );
        selector.handle_input_event("\u{1b}[C");
        selector.handle_input_event("\r");
        selector.handle_input_event("\u{1b}s");
        assert_eq!(
            *events.lock().expect("events"),
            ["select:high", "session:high"]
        );
    }

    #[test]
    fn renders_custom_title_wrapped_warning_and_provider_efforts() {
        let mut options = EffortSelectorOptions::new(
            vec![ThinkingEffort::from("off"), ThinkingEffort::from("ultra")],
            ThinkingEffort::from("ultra"),
            |_| {},
            || {},
        );
        options.title = Some("Choose effort".to_owned());
        options.warning =
            Some("Changing effort during a conversation may invalidate cached context.".to_owned());
        let mut selector = EffortSelectorComponent::new(options);
        let lines = selector.render(36);
        let plain = lines.iter().map(|line| strip_sgr(line)).collect::<Vec<_>>();
        assert!(plain.iter().any(|line| line.contains("Choose effort")));
        assert!(plain.iter().any(|line| line.contains("[ Ultra ]")));
        assert!(
            plain
                .iter()
                .filter(|line| line.contains("cached") || line.contains("Changing"))
                .count()
                >= 2
        );
        assert!(lines.iter().all(|line| visible_width(line) <= 36));
    }

    #[test]
    fn labels_empty_ascii_and_unicode_efforts() {
        assert_eq!(effort_label(&ThinkingEffort::from("")), "");
        assert_eq!(effort_label(&ThinkingEffort::from("high")), "High");
        assert_eq!(effort_label(&ThinkingEffort::from("élan")), "Élan");
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
