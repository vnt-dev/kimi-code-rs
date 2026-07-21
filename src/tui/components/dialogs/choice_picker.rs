use std::{any::Any, sync::Arc};

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
    utils::{
        printable_key::printable_char,
        searchable_list::{SearchableList, SearchableListView},
    },
};

const SELECT_POINTER: &str = "❯";
const CURRENT_MARK: &str = "← current";

type HintFormatter = dyn Fn(&str) -> String + Send + Sync;
type SelectCallback = dyn FnMut(String) + Send;
type CancelCallback = dyn FnMut() + Send;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceTone {
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeTone {
    #[default]
    Success,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
    pub tone: Option<ChoiceTone>,
    pub description: Option<String>,
    pub description_tone: Option<ColorToken>,
}

impl ChoiceOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            tone: None,
            description: None,
            description_tone: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_tone(mut self, tone: ChoiceTone) -> Self {
        self.tone = Some(tone);
        self
    }

    pub fn with_description_tone(mut self, tone: ColorToken) -> Self {
        self.description_tone = Some(tone);
        self
    }
}

pub struct ChoicePickerOptions {
    pub title: String,
    pub hint: Option<String>,
    pub format_hint: Option<Arc<HintFormatter>>,
    pub notice: Option<String>,
    pub notice_tone: NoticeTone,
    pub options: Vec<ChoiceOption>,
    pub current_value: Option<String>,
    pub searchable: bool,
    pub page_size: Option<isize>,
    on_select: Box<SelectCallback>,
    on_session_only_select: Option<Box<SelectCallback>>,
    on_cancel: Box<CancelCallback>,
}

impl ChoicePickerOptions {
    pub fn new<S, C>(
        title: impl Into<String>,
        options: Vec<ChoiceOption>,
        on_select: S,
        on_cancel: C,
    ) -> Self
    where
        S: FnMut(String) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            title: title.into(),
            hint: None,
            format_hint: None,
            notice: None,
            notice_tone: NoticeTone::Success,
            options,
            current_value: None,
            searchable: false,
            page_size: None,
            on_select: Box::new(on_select),
            on_session_only_select: None,
            on_cancel: Box::new(on_cancel),
        }
    }

    pub fn with_session_only_select<S>(mut self, callback: S) -> Self
    where
        S: FnMut(String) + Send + 'static,
    {
        self.on_session_only_select = Some(Box::new(callback));
        self
    }
}

/// Modal, optionally-searchable single-select list.
///
/// Original: `choice-picker.ts`, `ChoicePickerComponent`.
pub struct ChoicePickerComponent {
    pub focused: bool,
    options: ChoicePickerOptions,
    list: SearchableList<ChoiceOption>,
}

impl ChoicePickerComponent {
    pub fn new(options: ChoicePickerOptions) -> Self {
        let current_index = options
            .current_value
            .as_deref()
            .and_then(|current| {
                options
                    .options
                    .iter()
                    .position(|option| option.value == current)
            })
            .unwrap_or_default();
        let list = SearchableList::new(
            options.options.clone(),
            |option: &ChoiceOption| {
                format!(
                    "{} {}",
                    option.label,
                    option.description.as_deref().unwrap_or_default()
                )
            },
            options.page_size,
            Some(isize::try_from(current_index).unwrap_or(isize::MAX)),
            options.searchable,
        );
        Self {
            focused: false,
            options,
            list,
        }
    }

    pub fn selected(&self) -> Option<&ChoiceOption> {
        self.list.selected()
    }

    pub fn query(&self) -> &str {
        self.list.view().query
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape) {
            if !self.list.clear_query() {
                (self.options.on_cancel)();
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Alt('s')) {
            if let Some(value) = self.list.selected().map(|option| option.value.clone())
                && let Some(callback) = &mut self.options.on_session_only_select
            {
                callback(value);
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Left) {
            self.list.page_up();
            return;
        }
        if matches_editor_key(data, EditorKey::Right) {
            self.list.page_down();
            return;
        }
        let is_space = printable_char(data) == " ";
        if matches_editor_key(data, EditorKey::Enter) || is_space && !self.options.searchable {
            if let Some(value) = self.list.selected().map(|option| option.value.clone()) {
                (self.options.on_select)(value);
            }
            return;
        }
        self.list.handle_key(data);
    }

    fn render_picker(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let view = self.list.view();
        let mut navigation = vec!["↑↓ navigate"];
        if view.page.page_count > 1 {
            navigation.push("←→ page");
        }
        navigation.extend(["Enter select", "Esc cancel"]);
        let hint = self
            .options
            .hint
            .clone()
            .unwrap_or_else(|| navigation.join(" · "));
        let suffix = if self.options.searchable && view.query.is_empty() {
            current_theme().fg(ColorToken::TextMuted, "  (type to search)")
        } else {
            String::new()
        };
        let mut lines = vec![
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            format!(
                "{}{}",
                current_theme().bold_fg(ColorToken::Primary, &format!(" {}", self.options.title)),
                suffix
            ),
        ];
        for hint_line in hint.lines() {
            let line = format!(" {hint_line}");
            lines.push(self.options.format_hint.as_ref().map_or_else(
                || current_theme().fg(ColorToken::TextMuted, &line),
                |format| format(&line),
            ));
        }
        if let Some(notice) = &self.options.notice {
            let token = match self.options.notice_tone {
                NoticeTone::Success => ColorToken::Success,
                NoticeTone::Warning => ColorToken::Warning,
            };
            for notice_line in notice.lines() {
                for wrapped in wrap_description(notice_line, width.saturating_sub(1).max(1)) {
                    lines.push(current_theme().fg(token, &format!(" {wrapped}")));
                }
            }
        }
        lines.push(String::new());
        if self.options.searchable && !view.query.is_empty() {
            lines.push(format!(
                "{}{}",
                current_theme().fg(ColorToken::Primary, " Search: "),
                current_theme().fg(ColorToken::Text, view.query)
            ));
        }
        self.render_options(&view, width, &mut lines);
        lines.push(String::new());
        if view.page.page_count > 1 {
            lines.push(current_theme().fg(
                ColorToken::TextMuted,
                &format!(" Page {}/{}", view.page.page + 1, view.page.page_count),
            ));
        }
        lines.push(current_theme().fg(ColorToken::Primary, &"─".repeat(width)));
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "", false))
            .collect()
    }

    fn render_options(
        &self,
        view: &SearchableListView<'_, ChoiceOption>,
        width: usize,
        lines: &mut Vec<String>,
    ) {
        if view.items.is_empty() {
            lines.push(current_theme().fg(ColorToken::TextMuted, "   No matches"));
            return;
        }
        for index in view.page.start..view.page.end {
            let option = view.items[index];
            let selected = index == view.selected_index;
            let current = self.options.current_value.as_deref() == Some(option.value.as_str());
            let pointer = if selected { SELECT_POINTER } else { " " };
            let mut line = current_theme().fg(
                if selected {
                    ColorToken::Primary
                } else {
                    ColorToken::TextDim
                },
                &format!("  {pointer} "),
            );
            line.push_str(&style_option_label(option, selected));
            if current {
                line.push(' ');
                line.push_str(&current_theme().fg(ColorToken::Success, CURRENT_MARK));
            }
            lines.push(line);
            if let Some(description) = option
                .description
                .as_deref()
                .filter(|text| !text.is_empty())
            {
                let token = if selected {
                    option.description_tone.unwrap_or(ColorToken::TextMuted)
                } else {
                    ColorToken::TextMuted
                };
                for wrapped in wrap_description(description, width.saturating_sub(4).max(1)) {
                    lines.push(current_theme().fg(token, &format!("    {wrapped}")));
                }
            }
        }
    }
}

impl Component for ChoicePickerComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_picker(width)
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

fn style_option_label(option: &ChoiceOption, selected: bool) -> String {
    if option.tone == Some(ChoiceTone::Danger) {
        if selected {
            current_theme().bold_fg(ColorToken::Error, &option.label)
        } else {
            current_theme().fg(ColorToken::Error, &option.label)
        }
    } else if selected {
        current_theme().bold_fg(ColorToken::Primary, &option.label)
    } else {
        current_theme().fg(ColorToken::Text, &option.label)
    }
}

fn wrap_description(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
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
    lines
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn picker(events: Arc<Mutex<Vec<String>>>, searchable: bool) -> ChoicePickerComponent {
        let selected_events = Arc::clone(&events);
        let cancelled_events = Arc::clone(&events);
        let session_events = Arc::clone(&events);
        let mut options = ChoicePickerOptions::new(
            "Choose mode",
            vec![
                ChoiceOption::new("manual", "Manual").with_description("Ask before every command"),
                ChoiceOption::new("auto", "Auto").with_description("Proceed with safe commands"),
                ChoiceOption::new("yolo", "YOLO")
                    .with_tone(ChoiceTone::Danger)
                    .with_description("Proceed without asking")
                    .with_description_tone(ColorToken::Warning),
            ],
            move |value| selected_events.lock().expect("events").push(value),
            move || {
                cancelled_events
                    .lock()
                    .expect("events")
                    .push("cancel".to_owned())
            },
        )
        .with_session_only_select(move |value| {
            session_events
                .lock()
                .expect("events")
                .push(format!("session:{value}"));
        });
        options.searchable = searchable;
        options.current_value = Some("auto".to_owned());
        options.page_size = Some(2);
        ChoicePickerComponent::new(options)
    }

    #[test]
    fn renders_search_hint_current_mark_descriptions_and_paging() {
        let mut picker = picker(Arc::new(Mutex::new(Vec::new())), true);
        let lines = picker.render(42);
        let plain = lines.iter().map(|line| strip_sgr(line)).collect::<Vec<_>>();
        assert!(plain[1].contains("Choose mode  (type to search)"));
        assert!(plain.iter().any(|line| line.contains("← current")));
        assert!(plain.iter().any(|line| line.contains("Ask before every")));
        assert!(plain.iter().any(|line| line.contains("Page 1/2")));
        assert!(lines.iter().all(|line| visible_width(line) <= 42));
    }

    #[test]
    fn navigates_selects_and_supports_session_only_selection() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut picker = picker(Arc::clone(&events), false);
        picker.handle_input_event("\u{1b}[B");
        picker.handle_input_event("\r");
        picker.handle_input_event("\u{1b}s");
        assert_eq!(
            *events.lock().expect("events"),
            ["yolo".to_owned(), "session:yolo".to_owned()]
        );
    }

    #[test]
    fn searchable_space_filters_and_escape_clears_before_cancelling() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut picker = picker(Arc::clone(&events), true);
        for key in ["y", "o", "l", "o", " "] {
            picker.handle_input_event(key);
        }
        assert_eq!(picker.query(), "yolo ");
        assert_eq!(
            picker.selected().map(|option| option.value.as_str()),
            Some("yolo")
        );
        assert!(events.lock().expect("events").is_empty());
        picker.handle_input_event("\u{1b}");
        assert_eq!(picker.query(), "");
        assert!(events.lock().expect("events").is_empty());
        picker.handle_input_event("\u{1b}");
        assert_eq!(*events.lock().expect("events"), ["cancel"]);
    }

    #[test]
    fn non_searchable_space_selects_and_left_right_page() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut picker = picker(Arc::clone(&events), false);
        picker.handle_input_event("\u{1b}[C");
        assert_eq!(
            picker.selected().map(|option| option.value.as_str()),
            Some("yolo")
        );
        picker.handle_input_event(" ");
        picker.handle_input_event("\u{1b}[D");
        assert_eq!(
            picker.selected().map(|option| option.value.as_str()),
            Some("manual")
        );
        assert_eq!(*events.lock().expect("events"), ["yolo"]);
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
