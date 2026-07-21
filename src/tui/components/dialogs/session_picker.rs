use std::{
    any::Any,
    time::{SystemTime, UNIX_EPOCH},
};

use unicode_segmentation::UnicodeSegmentation;

use crate::{
    migration::format_session_label,
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, visible_width},
        },
        keys::{EditorKey, matches_editor_key},
        theme::{ColorToken, current_theme},
        utils::{searchable_list::SearchableList, session_picker_rows::SessionRow},
    },
};

const ELLIPSIS: &str = "…";
const CURRENT_MARK: &str = "← current";
const SELECT_POINTER: &str = "❯";

type SelectCallback = dyn FnMut(SessionRow) + Send;
type VoidCallback = dyn FnMut() + Send;
type ToggleScopeCallback = dyn FnMut(String) + Send;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionScope {
    #[default]
    CurrentDirectory,
    All,
}

pub struct SessionPickerOptions {
    pub sessions: Vec<SessionRow>,
    pub loading: bool,
    pub current_session_id: String,
    pub scope: SessionScope,
    pub initial_selected_session_id: Option<String>,
    pub page_size: usize,
    pub max_visible_sessions: usize,
    on_select: Box<SelectCallback>,
    on_cancel: Box<VoidCallback>,
    on_ctrl_c: Option<Box<VoidCallback>>,
    on_ctrl_d: Option<Box<VoidCallback>>,
    on_toggle_scope: Option<Box<ToggleScopeCallback>>,
}

impl SessionPickerOptions {
    pub fn new<S, C>(
        sessions: Vec<SessionRow>,
        current_session_id: impl Into<String>,
        on_select: S,
        on_cancel: C,
    ) -> Self
    where
        S: FnMut(SessionRow) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            sessions,
            loading: false,
            current_session_id: current_session_id.into(),
            scope: SessionScope::CurrentDirectory,
            initial_selected_session_id: None,
            page_size: 50,
            max_visible_sessions: 4,
            on_select: Box::new(on_select),
            on_cancel: Box::new(on_cancel),
            on_ctrl_c: None,
            on_ctrl_d: None,
            on_toggle_scope: None,
        }
    }

    pub fn with_toggle_scope<T>(mut self, callback: T) -> Self
    where
        T: FnMut(String) + Send + 'static,
    {
        self.on_toggle_scope = Some(Box::new(callback));
        self
    }
}

/// Searchable, progressively-loaded session chooser.
///
/// Original: `session-picker.ts`, `SessionPickerComponent`.
pub struct SessionPickerComponent {
    pub focused: bool,
    loading: bool,
    current_session_id: String,
    scope: SessionScope,
    max_visible_sessions: usize,
    page_size: usize,
    visible_count: usize,
    session_count: usize,
    list: SearchableList<SessionRow>,
    on_select: Box<SelectCallback>,
    on_cancel: Box<VoidCallback>,
    on_ctrl_c: Option<Box<VoidCallback>>,
    on_ctrl_d: Option<Box<VoidCallback>>,
    on_toggle_scope: Option<Box<ToggleScopeCallback>>,
}

impl SessionPickerComponent {
    pub fn new(options: SessionPickerOptions) -> Self {
        let page_size = options.page_size.max(1);
        let initial_index = options
            .initial_selected_session_id
            .as_deref()
            .and_then(|id| options.sessions.iter().position(|session| session.id == id))
            .unwrap_or_default();
        let visible_count = options
            .sessions
            .len()
            .min((initial_index / page_size + 1) * page_size);
        let session_count = options.sessions.len();
        let list = SearchableList::new(
            options.sessions,
            session_search_text,
            Some(isize::try_from(page_size).unwrap_or(isize::MAX)),
            Some(isize::try_from(initial_index).unwrap_or(isize::MAX)),
            true,
        );
        Self {
            focused: false,
            loading: options.loading,
            current_session_id: options.current_session_id,
            scope: options.scope,
            max_visible_sessions: options.max_visible_sessions.max(1),
            page_size,
            visible_count,
            session_count,
            list,
            on_select: options.on_select,
            on_cancel: options.on_cancel,
            on_ctrl_c: options.on_ctrl_c,
            on_ctrl_d: options.on_ctrl_d,
            on_toggle_scope: options.on_toggle_scope,
        }
    }

    pub fn query(&self) -> &str {
        self.list.view().query
    }
    pub fn selected_session_id(&self) -> Option<&str> {
        self.list.selected().map(|s| s.id.as_str())
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Ctrl('c')) {
            if let Some(callback) = &mut self.on_ctrl_c {
                callback();
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Ctrl('d')) {
            if let Some(callback) = &mut self.on_ctrl_d {
                callback();
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Ctrl('a')) {
            let id = self
                .list
                .selected()
                .map_or_else(|| self.current_session_id.clone(), |s| s.id.clone());
            if let Some(callback) = &mut self.on_toggle_scope {
                callback(id);
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Escape) {
            if self.list.clear_query() {
                self.visible_count = self.list.view().items.len().min(self.page_size);
            } else {
                (self.on_cancel)();
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Enter) {
            if let Some(session) = self.list.selected().cloned() {
                (self.on_select)(session);
            }
            return;
        }
        let previous_query = self.list.view().query.to_owned();
        if self.list.handle_key(data) {
            self.sync_visible_count(&previous_query);
        }
    }

    fn sync_visible_count(&mut self, previous_query: &str) {
        let view = self.list.view();
        if view.query != previous_query {
            self.visible_count = view.items.len().min(self.page_size);
            return;
        }
        let loaded = view.items.len().min(self.visible_count);
        if view.selected_index >= loaded.saturating_sub(1) && loaded < view.items.len() {
            self.visible_count = view.items.len().min(self.visible_count + self.page_size);
        }
    }

    fn render_picker(&self, width: usize) -> Vec<String> {
        let mut lines = self.render_lines(width);
        for line in &mut lines {
            *line = truncate_to_width(line, width, ELLIPSIS, false);
        }
        lines
    }

    fn render_lines(&self, width: usize) -> Vec<String> {
        let border = current_theme().fg(ColorToken::Primary, &"─".repeat(width));
        let title = if self.scope == SessionScope::All {
            "All sessions"
        } else {
            "Sessions"
        };
        let scope_hint = self.on_toggle_scope.as_ref().map(|_| {
            if self.scope == SessionScope::All {
                "Ctrl+A current cwd"
            } else {
                "Ctrl+A all"
            }
        });
        let mut lines = vec![border.clone()];
        if self.loading {
            lines.push(current_theme().bold_fg(ColorToken::Primary, title));
            lines.push(current_theme().fg(ColorToken::TextMuted, "Loading sessions..."));
            lines.push(border);
            return lines;
        }
        if self.session_count == 0 {
            let mut hints = Vec::new();
            if let Some(hint) = scope_hint {
                hints.push(hint);
            }
            hints.push("Esc cancel");
            lines.push(current_theme().bold_fg(ColorToken::Primary, title));
            lines.push(current_theme().fg(ColorToken::TextMuted, &hints.join(" · ")));
            lines.push(String::new());
            lines.push(current_theme().fg(ColorToken::TextMuted, "No sessions found."));
            lines.push(border);
            return lines;
        }
        let view = self.list.view();
        let suffix = if view.query.is_empty() {
            current_theme().fg(ColorToken::TextMuted, "  (type to search)")
        } else {
            String::new()
        };
        let mut hints = Vec::new();
        if !view.query.is_empty() {
            hints.push("Backspace clear");
        }
        hints.push("↑↓ navigate");
        if let Some(hint) = scope_hint {
            hints.push(hint);
        }
        hints.extend(["Enter select", "Esc cancel"]);
        lines.push(format!(
            "{}{}",
            current_theme().bold_fg(ColorToken::Primary, title),
            suffix
        ));
        lines.push(current_theme().fg(ColorToken::TextMuted, &hints.join(" · ")));
        lines.push(String::new());
        if !view.query.is_empty() {
            lines.push(format!(
                "{}{}",
                current_theme().fg(ColorToken::Primary, "Search: "),
                current_theme().fg(ColorToken::Text, view.query)
            ));
        }
        let loaded_count = view.items.len().min(self.visible_count);
        if loaded_count == 0 {
            lines.push(current_theme().fg(ColorToken::TextMuted, "No matches"));
            lines.push(border);
            return lines;
        }
        let start = view
            .selected_index
            .saturating_sub(self.max_visible_sessions / 2)
            .min(loaded_count.saturating_sub(self.max_visible_sessions));
        let end = (start + self.max_visible_sessions).min(loaded_count);
        for (visible_index, session) in view.items[start..end].iter().enumerate() {
            lines.extend(render_session_card(
                width,
                session,
                start + visible_index == view.selected_index,
                session.id == self.current_session_id,
            ));
            if start + visible_index + 1 < end {
                lines.push(String::new());
            }
        }
        if loaded_count > end - start || !view.query.is_empty() {
            lines.push(String::new());
            let total = if !view.query.is_empty() {
                format!("{loaded_count} loaded / {} matches", view.items.len())
            } else if loaded_count == self.session_count {
                format!("{loaded_count} sessions")
            } else {
                format!("{loaded_count} loaded / {} sessions", self.session_count)
            };
            lines.push(current_theme().fg(
                ColorToken::TextMuted,
                &format!("Showing {}-{end} of {total}", start + 1),
            ));
        }
        lines.push(border);
        lines
    }
}

impl Component for SessionPickerComponent {
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

fn render_session_card(
    width: usize,
    session: &SessionRow,
    selected: bool,
    current: bool,
) -> Vec<String> {
    let pointer = if selected { SELECT_POINTER } else { " " };
    let time = format_relative_time(session.updated_at, now_millis());
    let badge = if current { CURRENT_MARK } else { "" };
    let raw_title = session.title.as_deref().unwrap_or(&session.id).trim();
    let raw_title = if raw_title.is_empty() {
        &session.id
    } else {
        raw_title
    };
    let title = format_session_label(raw_title, session.metadata.as_ref());
    let trailing = [time.as_str(), badge]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let trailing_width = if trailing.is_empty() {
        0
    } else {
        visible_width(&format!("  {}", trailing.join("  ")))
    };
    let budget = width
        .saturating_sub(visible_width(pointer) + 1 + trailing_width)
        .max(8);
    let title = truncate_to_width(&single_line(&title), budget, ELLIPSIS, false);
    let mut header = current_theme().fg(
        if selected {
            ColorToken::Primary
        } else {
            ColorToken::TextDim
        },
        &format!("{pointer} "),
    );
    let styled_title = if selected {
        current_theme().bold_fg(ColorToken::Primary, &title)
    } else {
        current_theme().fg(ColorToken::Text, &title)
    };
    header.push_str(&styled_title);
    if !time.is_empty() {
        header.push_str(&format!(
            "  {}",
            current_theme().fg(ColorToken::TextDim, &time)
        ));
    }
    if current {
        header.push_str(&format!(
            "  {}",
            current_theme().fg(ColorToken::Success, badge)
        ));
    }
    let mut card = vec![header];
    let dir = home_alias(&session.work_dir);
    if 2 + visible_width(&session.id) + 3 + visible_width(&dir) <= width {
        card.push(format!(
            "  {}{}{}",
            current_theme().fg(ColorToken::TextMuted, &session.id),
            current_theme().fg(ColorToken::TextDim, "   "),
            current_theme().fg(ColorToken::TextMuted, &dir)
        ));
    } else {
        card.push(format!(
            "  {}",
            current_theme().fg(ColorToken::TextMuted, &session.id)
        ));
        card.push(format!(
            "  {}",
            current_theme().fg(
                ColorToken::TextMuted,
                &truncate_path_left(&dir, width.saturating_sub(2).max(8))
            )
        ));
    }
    if let Some(prompt) = session
        .last_prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let prompt = truncate_to_width(
            &single_line(prompt),
            width.saturating_sub(4).max(8),
            ELLIPSIS,
            false,
        );
        card.push(format!(
            "  {}",
            current_theme().fg(ColorToken::TextDim, &format!("› {prompt}"))
        ));
    }
    card
}

fn format_relative_time(timestamp: f64, now: f64) -> String {
    if !timestamp.is_finite() || timestamp <= 0.0 {
        return String::new();
    }
    let seconds = ((now - timestamp).max(0.0) / 1000.0).floor() as u64;
    if seconds < 60 {
        "just now".to_owned()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn session_search_text(session: &SessionRow) -> String {
    let title = session.title.as_deref().unwrap_or(&session.id).trim();
    single_line(if title.is_empty() { &session.id } else { title })
}
fn home_alias(path: &str) -> String {
    std::env::var("HOME")
        .ok()
        .filter(|home| !home.is_empty() && path.starts_with(home))
        .map_or_else(
            || path.to_owned(),
            |home| format!("~{}", &path[home.len()..]),
        )
}
fn truncate_path_left(path: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if visible_width(path) <= width {
        return path.to_owned();
    }
    if width == 1 {
        return ELLIPSIS.to_owned();
    }
    let mut tail = Vec::new();
    let mut used = 0;
    for grapheme in UnicodeSegmentation::graphemes(path, true).rev() {
        let size = visible_width(grapheme);
        if used + size > width - 1 {
            break;
        }
        used += size;
        tail.push(grapheme);
    }
    tail.reverse();
    format!("{ELLIPSIS}{}", tail.concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    fn row(id: &str, title: &str, updated_at: f64) -> SessionRow {
        SessionRow {
            id: id.to_owned(),
            title: Some(title.to_owned()),
            last_prompt: Some(" first\n prompt ".to_owned()),
            work_dir: format!("/projects/{id}"),
            updated_at,
            metadata: None,
        }
    }
    #[test]
    fn formats_time_paths_and_search_text() {
        assert_eq!(format_relative_time(1_000.0, 31_000.0), "just now");
        assert_eq!(format_relative_time(1_000.0, 121_000.0), "2m ago");
        assert_eq!(format_relative_time(f64::NAN, 1.0), "");
        assert_eq!(truncate_path_left("/very/long/project", 8), "…project");
        assert_eq!(session_search_text(&row("id", " a\n b ", 1.0)), "a b");
    }
    #[test]
    fn searches_clears_then_cancels_and_selects() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let selected = Arc::clone(&events);
        let cancelled = Arc::clone(&events);
        let mut picker = SessionPickerComponent::new(SessionPickerOptions::new(
            vec![row("one", "Alpha", 1.0), row("two", "Beta", 1.0)],
            "one",
            move |s| selected.lock().expect("events").push(s.id),
            move || cancelled.lock().expect("events").push("cancel".to_owned()),
        ));
        for c in "Beta".chars() {
            picker.handle_input_event(&c.to_string());
        }
        assert_eq!(picker.selected_session_id(), Some("two"));
        picker.handle_input_event("\r");
        picker.handle_input_event("\u{1b}");
        assert_eq!(picker.query(), "");
        picker.handle_input_event("\u{1b}");
        assert_eq!(*events.lock().expect("events"), ["two", "cancel"]);
    }
    #[test]
    fn toggles_scope_with_selected_id_and_progressively_loads() {
        let ids = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::clone(&ids);
        let sessions = (0..6)
            .map(|i| row(&format!("s{i}"), &format!("Session {i}"), 1.0))
            .collect();
        let mut options = SessionPickerOptions::new(sessions, "s0", |_| {}, || {})
            .with_toggle_scope(move |id| called.lock().expect("ids").push(id));
        options.page_size = 2;
        options.initial_selected_session_id = Some("s2".to_owned());
        let mut picker = SessionPickerComponent::new(options);
        assert_eq!(picker.visible_count, 4);
        picker.handle_input_event("\u{1b}[1;5A");
        picker.handle_input_event("\x01");
        assert_eq!(*ids.lock().expect("ids"), ["s2"]);
    }
    #[test]
    fn renders_loading_empty_and_session_cards_with_width_bound() {
        let mut loading = SessionPickerOptions::new(Vec::new(), "", |_| {}, || {});
        loading.loading = true;
        let mut picker = SessionPickerComponent::new(loading);
        assert!(
            picker
                .render(30)
                .iter()
                .any(|l| strip(l).contains("Loading"))
        );
        let mut picker = SessionPickerComponent::new(SessionPickerOptions::new(
            vec![row("current", "A title", now_millis())],
            "current",
            |_| {},
            || {},
        ));
        let lines = picker.render(32);
        let plain = lines.iter().map(|l| strip(l)).collect::<Vec<_>>();
        assert!(plain.iter().any(|l| l.contains("← current")));
        assert!(plain.iter().any(|l| l.contains("first prompt")));
        assert!(lines.iter().all(|l| visible_width(l) <= 32));
    }
    fn strip(text: &str) -> String {
        regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("regex")
            .replace_all(text, "")
            .into_owned()
    }
}
