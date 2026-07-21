use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
    utils::searchable_list::SearchableList,
};

const MAX_VISIBLE_CHOICES: usize = 5;
const PREFERRED_SELECTED_OFFSET: usize = 2;
const SELECT_POINTER: &str = "❯";

type SelectCallback = dyn FnMut(UndoChoice) + Send;
type CancelCallback = dyn FnMut() + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoChoice {
    pub id: String,
    pub count: usize,
    pub input: String,
    pub label: String,
}

pub struct UndoSelectorOptions {
    pub choices: Vec<UndoChoice>,
    on_select: Box<SelectCallback>,
    on_cancel: Box<CancelCallback>,
}

impl UndoSelectorOptions {
    pub fn new<S, C>(choices: Vec<UndoChoice>, on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(UndoChoice) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            choices,
            on_select: Box::new(on_select),
            on_cancel: Box::new(on_cancel),
        }
    }
}

/// Selects the transcript boundary used by `/undo`.
///
/// Original: `undo-selector.ts`, `UndoSelectorComponent`.
pub struct UndoSelectorComponent {
    pub focused: bool,
    options: UndoSelectorOptions,
    list: SearchableList<UndoChoice>,
    submitted: bool,
}

impl UndoSelectorComponent {
    pub fn new(options: UndoSelectorOptions) -> Self {
        let initial_index = options.choices.len().saturating_sub(1);
        let list = SearchableList::new(
            options.choices.clone(),
            |choice: &UndoChoice| choice.label.clone(),
            None,
            Some(isize::try_from(initial_index).unwrap_or(isize::MAX)),
            false,
        );
        Self {
            focused: false,
            options,
            list,
            submitted: false,
        }
    }

    pub fn selected(&self) -> Option<&UndoChoice> {
        self.list.selected()
    }

    pub fn is_submitted(&self) -> bool {
        self.submitted
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if self.submitted {
            return;
        }
        if matches_editor_key(data, EditorKey::Escape) {
            (self.options.on_cancel)();
            return;
        }
        if self.list.handle_key(data) {
            return;
        }
        if matches_editor_key(data, EditorKey::Enter)
            && let Some(choice) = self.list.selected().cloned()
        {
            self.submitted = true;
            (self.options.on_select)(choice);
        }
    }

    fn render_selector(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let view = self.list.view();
        let mut lines = vec![
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            current_theme().bold_fg(ColorToken::Primary, " Select messages to undo"),
            current_theme().fg(
                ColorToken::TextMuted,
                " ↑↓ navigate · Enter select · Esc cancel",
            ),
            String::new(),
        ];
        if view.items.is_empty() {
            lines.push(current_theme().fg(ColorToken::TextMuted, "   No messages"));
        } else {
            let visible_count = MAX_VISIBLE_CHOICES.min(view.items.len());
            let max_start = view.items.len() - visible_count;
            let start = view
                .selected_index
                .saturating_sub(PREFERRED_SELECTED_OFFSET)
                .min(max_start);
            let end = start + visible_count;
            for index in start..end {
                let choice = view.items[index];
                lines.push(render_choice_line(
                    choice,
                    index == view.selected_index,
                    index > view.selected_index,
                    width,
                ));
            }
        }
        lines.push(String::new());
        lines.push(current_theme().fg(ColorToken::Primary, &"─".repeat(width)));
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "", false))
            .collect()
    }
}

impl Component for UndoSelectorComponent {
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

fn render_choice_line(
    choice: &UndoChoice,
    selected: bool,
    in_undo_range: bool,
    width: usize,
) -> String {
    let pointer = if selected { SELECT_POINTER } else { " " };
    let prefix = format!("  {pointer} ");
    let label_budget = width.saturating_sub(visible_width(&prefix)).max(8);
    let label = truncate_to_width(&choice.label, label_budget, "…", false);
    let token = if selected {
        ColorToken::Primary
    } else if in_undo_range {
        ColorToken::TextDim
    } else {
        ColorToken::Text
    };
    let mut line = current_theme().fg(
        if selected {
            ColorToken::Primary
        } else {
            ColorToken::TextDim
        },
        &prefix,
    );
    if selected {
        line.push_str(&current_theme().bold_fg(token, &label));
    } else {
        line.push_str(&current_theme().fg(token, &label));
    }
    line
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn choices(count: usize) -> Vec<UndoChoice> {
        (0..count)
            .map(|index| UndoChoice {
                id: format!("id-{index}"),
                count: index + 1,
                input: format!("input {index}"),
                label: format!("Message {index}"),
            })
            .collect()
    }

    #[test]
    fn starts_at_last_choice_and_submits_only_once() {
        let selected = Arc::new(Mutex::new(Vec::new()));
        let callback = Arc::clone(&selected);
        let mut selector = UndoSelectorComponent::new(UndoSelectorOptions::new(
            choices(4),
            move |choice| callback.lock().expect("selected").push(choice.id),
            || {},
        ));
        assert_eq!(
            selector.selected().map(|choice| choice.id.as_str()),
            Some("id-3")
        );
        selector.handle_input_event("\u{1b}[A");
        selector.handle_input_event("\r");
        selector.handle_input_event("\r");
        assert!(selector.is_submitted());
        assert_eq!(*selected.lock().expect("selected"), ["id-2"]);
    }

    #[test]
    fn centers_five_choice_window_and_truncates_narrow_labels() {
        let mut selector =
            UndoSelectorComponent::new(UndoSelectorOptions::new(choices(9), |_| {}, || {}));
        for _ in 0..4 {
            selector.handle_input_event("\u{1b}[A");
        }
        let lines = selector.render(16);
        let choice_lines = lines
            .iter()
            .map(|line| strip_sgr(line))
            .filter(|line| line.contains("Message"))
            .collect::<Vec<_>>();
        assert_eq!(choice_lines.len(), 5);
        assert!(choice_lines[2].contains("❯"));
        assert!(lines.iter().all(|line| visible_width(line) <= 16));
    }

    #[test]
    fn empty_selector_renders_message_and_escape_cancels() {
        let cancelled = Arc::new(Mutex::new(0));
        let callback = Arc::clone(&cancelled);
        let mut selector = UndoSelectorComponent::new(UndoSelectorOptions::new(
            Vec::new(),
            |_| {},
            move || *callback.lock().expect("cancelled") += 1,
        ));
        assert!(selector.selected().is_none());
        assert!(
            selector
                .render(40)
                .iter()
                .any(|line| strip_sgr(line).contains("No messages"))
        );
        selector.handle_input_event("\u{1b}");
        assert_eq!(*cancelled.lock().expect("cancelled"), 1);
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
