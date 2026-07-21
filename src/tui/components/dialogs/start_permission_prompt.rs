use std::{any::Any, sync::LazyLock};

use regex::Regex;

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
    utils::printable_key::printable_char,
};

const SELECT_POINTER: &str = "❯";

type SelectCallback<T> = dyn FnMut(T) + Send;
type CancelCallback = dyn FnMut() + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartPermissionOption<T> {
    pub value: T,
    pub label: String,
    pub description: String,
}

impl<T> StartPermissionOption<T> {
    pub fn new(value: T, label: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            description: description.into(),
        }
    }
}

pub struct StartPermissionPromptOptions<T> {
    pub title: String,
    pub notice_lines: Vec<String>,
    pub options: Vec<StartPermissionOption<T>>,
    on_select: Box<SelectCallback<T>>,
    on_cancel: Box<CancelCallback>,
}

impl<T> StartPermissionPromptOptions<T> {
    pub fn new<S, C>(
        title: impl Into<String>,
        notice_lines: Vec<String>,
        options: Vec<StartPermissionOption<T>>,
        on_select: S,
        on_cancel: C,
    ) -> Self
    where
        S: FnMut(T) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            title: title.into(),
            notice_lines,
            options,
            on_select: Box::new(on_select),
            on_cancel: Box::new(on_cancel),
        }
    }
}

/// Permission-mode confirmation shown before long-running work starts.
///
/// Original: `start-permission-prompt.ts`,
/// `StartPermissionPromptComponent`.
pub struct StartPermissionPromptComponent<T> {
    pub focused: bool,
    options: StartPermissionPromptOptions<T>,
    selected_index: usize,
}

impl<T> StartPermissionPromptComponent<T>
where
    T: Clone,
{
    pub fn new(options: StartPermissionPromptOptions<T>) -> Self {
        Self {
            focused: false,
            options,
            selected_index: 0,
        }
    }

    pub fn selected(&self) -> Option<&T> {
        self.options
            .options
            .get(self.selected_index)
            .map(|option| &option.value)
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape) {
            (self.options.on_cancel)();
            return;
        }
        if matches_editor_key(data, EditorKey::Up) {
            self.selected_index = self.selected_index.saturating_sub(1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.selected_index = self
                .selected_index
                .saturating_add(1)
                .min(self.options.options.len().saturating_sub(1));
            return;
        }
        if (matches_editor_key(data, EditorKey::Enter) || printable_char(data) == " ")
            && let Some(value) = self.selected().cloned()
        {
            (self.options.on_select)(value);
        }
    }

    fn render_prompt(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let rule = current_theme().fg(ColorToken::Primary, &"─".repeat(width));
        let mut lines = vec![
            rule.clone(),
            current_theme().bold_fg(ColorToken::Primary, &format!(" {}", self.options.title)),
            current_theme().fg(
                ColorToken::TextMuted,
                " ↑↓ navigate · Enter select · Esc cancel",
            ),
            String::new(),
        ];
        let notice_width = width.saturating_sub(2).max(20);
        for paragraph in &self.options.notice_lines {
            for line in wrap_plain(paragraph, notice_width) {
                lines.push(format!(
                    " {}",
                    style_mode_names(&line, ColorToken::TextMuted)
                ));
            }
            lines.push(String::new());
        }
        for (index, option) in self.options.options.iter().enumerate() {
            let selected = index == self.selected_index;
            let pointer = if selected { SELECT_POINTER } else { " " };
            lines.push(format!(
                "{}{}",
                current_theme().fg(
                    if selected {
                        ColorToken::Primary
                    } else {
                        ColorToken::TextDim
                    },
                    &format!("  {pointer} "),
                ),
                style_label(&option.label, selected)
            ));
            for line in wrap_plain(&option.description, width.saturating_sub(4).max(20)) {
                lines.push(format!(
                    "    {}",
                    style_mode_names(&line, ColorToken::TextMuted)
                ));
            }
            lines.push(String::new());
        }
        lines.push(rule);
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "", false))
            .collect()
    }
}

impl<T> Component for StartPermissionPromptComponent<T>
where
    T: Clone + Send + 'static,
{
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_prompt(width)
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

fn style_label(label: &str, selected: bool) -> String {
    if selected {
        current_theme().bold_fg(ColorToken::Primary, label)
    } else {
        style_mode_names(label, ColorToken::Text)
    }
}

fn style_mode_names(text: &str, base_token: ColorToken) -> String {
    static MODE_NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(?:Manual|Auto|YOLO)\b").expect("valid mode-name regex"));
    let mut result = String::new();
    let mut end = 0;
    for matched in MODE_NAME.find_iter(text) {
        result.push_str(&current_theme().fg(base_token, &text[end..matched.start()]));
        result.push_str(&current_theme().bold_fg(ColorToken::TextStrong, matched.as_str()));
        end = matched.end();
    }
    result.push_str(&current_theme().fg(base_token, &text[end..]));
    result
}

fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if visible_width(&candidate) <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(current);
        }
        current = if visible_width(word) <= width {
            word.to_owned()
        } else {
            truncate_to_width(word, width, "…", false)
        };
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Choice {
        Auto,
        Yolo,
        Manual,
    }

    fn prompt(events: Arc<Mutex<Vec<String>>>) -> StartPermissionPromptComponent<Choice> {
        let selected = Arc::clone(&events);
        let cancelled = Arc::clone(&events);
        StartPermissionPromptComponent::new(StartPermissionPromptOptions::new(
            "Start with approvals on?",
            vec![
                "Manual mode asks before risky actions.".to_owned(),
                "Auto mode skips questions.".to_owned(),
            ],
            vec![
                StartPermissionOption::new(
                    Choice::Auto,
                    "Switch to Auto",
                    "Best for unattended work.",
                ),
                StartPermissionOption::new(
                    Choice::Yolo,
                    "Switch to YOLO",
                    "Tools are approved automatically.",
                ),
                StartPermissionOption::new(Choice::Manual, "Start in Manual", "Keep approvals on."),
            ],
            move |choice| selected.lock().expect("events").push(format!("{choice:?}")),
            move || cancelled.lock().expect("events").push("Cancel".to_owned()),
        ))
    }

    #[test]
    fn navigates_clamps_selects_with_space_and_cancels() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut prompt = prompt(Arc::clone(&events));
        prompt.handle_input_event("\u{1b}[A");
        assert_eq!(prompt.selected(), Some(&Choice::Auto));
        prompt.handle_input_event("\u{1b}[B");
        prompt.handle_input_event(" ");
        prompt.handle_input_event("\u{1b}");
        assert_eq!(*events.lock().expect("events"), ["Yolo", "Cancel"]);
    }

    #[test]
    fn renders_notice_options_and_emphasized_mode_names_at_all_widths() {
        let mut prompt = prompt(Arc::new(Mutex::new(Vec::new())));
        let lines = prompt.render(44);
        let plain = lines.iter().map(|line| strip_sgr(line)).collect::<Vec<_>>();
        assert!(plain.iter().any(|line| line.contains("Manual mode asks")));
        assert!(plain.iter().any(|line| line.contains("Switch to Auto")));
        assert!(plain.iter().any(|line| line.contains("Keep approvals on.")));
        assert!(lines.iter().all(|line| visible_width(line) <= 44));
        assert!(lines.iter().any(|line| line.contains("\u{1b}[1mManual")));

        let narrow = prompt.render(8);
        assert!(narrow.iter().all(|line| visible_width(line) <= 8));
    }

    #[test]
    fn wraps_empty_and_overlong_plain_text_like_the_source() {
        assert_eq!(wrap_plain("", 10), [""]);
        assert_eq!(wrap_plain("one two three", 7), ["one two", "three"]);
        assert!(wrap_plain("abcdefghijkl", 5)[0].ends_with('…'));
    }

    fn strip_sgr(text: &str) -> String {
        let regex = Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
