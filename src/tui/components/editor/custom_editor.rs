use std::{
    any::Any,
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};

use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

use crate::tui::{
    components::{Component, ComponentRole, core::CURSOR_MARKER, render::visible_width},
    keys::{EditorKey, is_key_release, matches_editor_key},
    theme::{ColorToken, current_theme},
    utils::printable_key::{is_printable_char, printable_char},
};

use super::{
    file_mention_provider::{
        AutocompleteItem, AutocompleteSuggestions, FileMentionProvider, InputMode,
    },
    wrapping_select_list::{SelectItem, SelectListLayout, SelectListTheme, WrappingSelectList},
};

const CAPS_LOCK_BIT: u32 = 64;
const CTRL_BIT: u32 = 4;
const SHIFT_BIT: u32 = 1;
const EDITOR_LEFT_PADDING: usize = 4;
const CURSOR_BLOCK: &str = "\u{1b}[7m \u{1b}[0m";

type HistoryFilter = dyn Fn(&str) -> bool + Send + Sync;

pub struct AutocompleteRequest {
    request_id: u64,
    snapshot_lines: Vec<String>,
    snapshot_cursor_line: usize,
    snapshot_cursor_col: usize,
    force: bool,
    explicit_tab: bool,
    provider: Arc<FileMentionProvider>,
    cancelled: Arc<AtomicBool>,
}

pub struct AutocompleteResponse {
    request_id: u64,
    snapshot_lines: Vec<String>,
    snapshot_cursor_line: usize,
    snapshot_cursor_col: usize,
    force: bool,
    explicit_tab: bool,
    cancelled: Arc<AtomicBool>,
    suggestions: Option<AutocompleteSuggestions>,
}

impl AutocompleteRequest {
    pub async fn run(self) -> AutocompleteResponse {
        let suggestions = self
            .provider
            .get_suggestions(
                &self.snapshot_lines,
                self.snapshot_cursor_line,
                self.snapshot_cursor_col,
                self.force,
                Arc::clone(&self.cancelled),
            )
            .await;
        AutocompleteResponse {
            request_id: self.request_id,
            snapshot_lines: self.snapshot_lines,
            snapshot_cursor_line: self.snapshot_cursor_line,
            snapshot_cursor_col: self.snapshot_cursor_col,
            force: self.force,
            explicit_tab: self.explicit_tab,
            cancelled: self.cancelled,
            suggestions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutocompleteAccept {
    NotAccepted,
    Applied,
    Submitted(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditorBufferState {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

impl Default for EditorBufferState {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        }
    }
}

/// Editable text and history state underlying Kimi's custom editor.
///
/// Original infrastructure: `packages/pi-tui/src/components/editor.ts` plus
/// `custom-editor.ts`, `CustomEditor`.
pub struct CustomEditor {
    state: EditorBufferState,
    pastes: HashMap<u64, String>,
    paste_counter: u64,
    history: Vec<String>,
    history_index: isize,
    history_draft: Option<EditorBufferState>,
    history_filter: Option<Arc<HistoryFilter>>,
    undo_stack: Vec<EditorBufferState>,
    input_mode: InputMode,
    consuming_paste: bool,
    consume_buffer: String,
    bracket_paste_buffer: Option<String>,
    autocomplete_showing: bool,
    autocomplete_request_pending: bool,
    autocomplete_provider: Option<Arc<FileMentionProvider>>,
    autocomplete_list: Option<WrappingSelectList>,
    autocomplete_prefix: String,
    autocomplete_force: bool,
    autocomplete_cancel: Option<Arc<AtomicBool>>,
    autocomplete_request_id: u64,
    autocomplete_max_visible: usize,
    terminal_rows: usize,
    scroll_offset: usize,
    focused: bool,
    connected_above: bool,
    border_highlighted: bool,
    argument_hints: HashMap<String, String>,
    pending_actions: Vec<EditorAction>,
}

impl Default for CustomEditor {
    fn default() -> Self {
        Self {
            state: EditorBufferState::default(),
            pastes: HashMap::new(),
            paste_counter: 0,
            history: Vec::new(),
            history_index: -1,
            history_draft: None,
            history_filter: None,
            undo_stack: Vec::new(),
            input_mode: InputMode::Prompt,
            consuming_paste: false,
            consume_buffer: String::new(),
            bracket_paste_buffer: None,
            autocomplete_showing: false,
            autocomplete_request_pending: false,
            autocomplete_provider: None,
            autocomplete_list: None,
            autocomplete_prefix: String::new(),
            autocomplete_force: false,
            autocomplete_cancel: None,
            autocomplete_request_id: 0,
            autocomplete_max_visible: 5,
            terminal_rows: 24,
            scroll_offset: 0,
            focused: false,
            connected_above: false,
            border_highlighted: false,
            argument_hints: HashMap::new(),
            pending_actions: Vec::new(),
        }
    }
}

impl CustomEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> String {
        self.state.lines.join("\n")
    }

    pub fn expanded_text(&self) -> String {
        expand_paste_markers(&self.text(), &self.pastes)
    }

    pub fn lines(&self) -> Vec<String> {
        self.state.lines.clone()
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.state.cursor_line, self.state.cursor_col)
    }

    pub fn input_mode(&self) -> InputMode {
        self.input_mode
    }

    pub fn set_input_mode(&mut self, mode: InputMode) -> bool {
        if self.input_mode == mode {
            return false;
        }
        self.input_mode = mode;
        true
    }

    pub fn set_autocomplete_activity(&mut self, showing: bool, request_pending: bool) {
        self.autocomplete_showing = showing;
        self.autocomplete_request_pending = request_pending;
    }

    pub fn set_autocomplete_provider(&mut self, provider: Arc<FileMentionProvider>) {
        self.cancel_autocomplete();
        self.autocomplete_provider = Some(provider);
    }

    pub fn set_autocomplete_max_visible(&mut self, max_visible: usize) {
        self.autocomplete_max_visible = max_visible.clamp(3, 20);
    }

    /// Captures a request without holding `&mut CustomEditor` across I/O.
    /// The host may spawn `AutocompleteRequest::run()` and later feed its
    /// response to `finish_autocomplete_request()` on the TUI task.
    pub fn begin_autocomplete_request(
        &mut self,
        force: bool,
        explicit_tab: bool,
    ) -> Option<AutocompleteRequest> {
        let provider = Arc::clone(self.autocomplete_provider.as_ref()?);
        if let Some(cancelled) = self.autocomplete_cancel.take() {
            cancelled.store(true, AtomicOrdering::Relaxed);
        }
        self.autocomplete_request_id = self.autocomplete_request_id.wrapping_add(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.autocomplete_cancel = Some(Arc::clone(&cancelled));
        self.autocomplete_request_pending = true;
        Some(AutocompleteRequest {
            request_id: self.autocomplete_request_id,
            snapshot_lines: self.state.lines.clone(),
            snapshot_cursor_line: self.state.cursor_line,
            snapshot_cursor_col: self.state.cursor_col,
            force,
            explicit_tab,
            provider,
            cancelled,
        })
    }

    /// Applies a completed request only when its buffer/cursor snapshot is
    /// still current. Stale and cancelled results are ignored.
    pub fn finish_autocomplete_request(&mut self, response: AutocompleteResponse) -> bool {
        if response.cancelled.load(AtomicOrdering::Relaxed)
            || response.request_id != self.autocomplete_request_id
        {
            return false;
        }
        if response.snapshot_lines != self.state.lines
            || response.snapshot_cursor_line != self.state.cursor_line
            || response.snapshot_cursor_col != self.state.cursor_col
        {
            self.autocomplete_request_pending = false;
            self.autocomplete_cancel = None;
            return false;
        }
        self.autocomplete_request_pending = false;
        self.autocomplete_cancel = None;
        let Some(suggestions) = response.suggestions.filter(|value| !value.items.is_empty()) else {
            self.clear_autocomplete_ui();
            return true;
        };
        if response.force && response.explicit_tab && suggestions.items.len() == 1 {
            let item = suggestions.items[0].clone();
            self.apply_completion_item(&item, &suggestions.prefix);
            self.clear_autocomplete_ui();
            return true;
        }
        self.apply_autocomplete_suggestions(suggestions, response.force);
        true
    }

    pub fn cancel_autocomplete(&mut self) {
        if let Some(cancelled) = self.autocomplete_cancel.take() {
            cancelled.store(true, AtomicOrdering::Relaxed);
        }
        self.autocomplete_request_id = self.autocomplete_request_id.wrapping_add(1);
        self.autocomplete_request_pending = false;
        self.clear_autocomplete_ui();
    }

    pub fn move_autocomplete_up(&mut self) -> bool {
        let Some(list) = &mut self.autocomplete_list else {
            return false;
        };
        list.move_up();
        true
    }

    pub fn move_autocomplete_down(&mut self) -> bool {
        let Some(list) = &mut self.autocomplete_list else {
            return false;
        };
        list.move_down();
        true
    }

    pub fn accept_autocomplete(&mut self, submit_slash: bool) -> AutocompleteAccept {
        let Some(selected) = self
            .autocomplete_list
            .as_ref()
            .and_then(WrappingSelectList::selected)
            .cloned()
        else {
            return AutocompleteAccept::NotAccepted;
        };
        let label = selected
            .label
            .clone()
            .unwrap_or_else(|| selected.value.clone());
        let item = AutocompleteItem {
            value: selected.value,
            label,
            description: selected.description,
        };
        let prefix = self.autocomplete_prefix.clone();
        self.apply_completion_item(&item, &prefix);
        self.cancel_autocomplete();
        if submit_slash && prefix.starts_with('/') {
            AutocompleteAccept::Submitted(self.submit_value())
        } else {
            AutocompleteAccept::Applied
        }
    }

    pub fn set_terminal_rows(&mut self, rows: usize) {
        self.terminal_rows = rows.max(1);
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn set_connected_above(&mut self, connected: bool) {
        self.connected_above = connected;
    }

    pub fn set_border_highlighted(&mut self, highlighted: bool) {
        self.border_highlighted = highlighted;
    }

    pub fn set_argument_hints(&mut self, hints: HashMap<String, String>) {
        self.argument_hints = hints;
    }

    pub fn take_actions(&mut self) -> Vec<EditorAction> {
        std::mem::take(&mut self.pending_actions)
    }

    pub fn is_showing_autocomplete(&self) -> bool {
        self.autocomplete_showing
    }

    pub fn set_cursor(&mut self, line: usize, col: usize) {
        self.state.cursor_line = line.min(self.state.lines.len().saturating_sub(1));
        self.state.cursor_col = col.min(line_char_count(&self.state.lines[self.state.cursor_line]));
    }

    pub fn set_text(&mut self, text: &str) {
        let normalized = normalize_editor_text(text);
        self.exit_history_browsing();
        if self.text() != normalized {
            self.push_undo_snapshot();
        }
        self.set_text_internal(&normalized, CursorPlacement::End);
    }

    pub fn insert_text_at_cursor(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        self.exit_history_browsing();
        self.insert_text_internal(text);
    }

    /// Inserts terminal paste content, replacing large values with a stable
    /// marker exactly as pi-tui does.
    pub fn insert_paste(&mut self, pasted_text: &str) {
        self.push_undo_snapshot();
        self.exit_history_browsing();
        let decoded = decode_paste_control_sequences(pasted_text);
        let normalized = normalize_editor_text(&decoded);
        let mut filtered = normalized
            .chars()
            .filter(|character| *character == '\n' || u32::from(*character) >= 32)
            .collect::<String>();
        if filtered.starts_with(['/', '~', '.']) {
            let current = &self.state.lines[self.state.cursor_line];
            let before = char_at(current, self.state.cursor_col.saturating_sub(1));
            if self.state.cursor_col > 0 && before.is_some_and(is_word_character) {
                filtered.insert(0, ' ');
            }
        }
        let line_count = filtered.split('\n').count();
        let character_count = filtered.chars().count();
        if line_count > 10 || character_count > 1000 {
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.insert(paste_id, filtered);
            let marker = if line_count > 10 {
                format!("[paste #{paste_id} +{line_count} lines]")
            } else {
                format!("[paste #{paste_id} {character_count} chars]")
            };
            self.insert_text_internal(&marker);
        } else {
            self.insert_text_internal(&filtered);
        }
    }

    pub fn expand_paste_marker_at_cursor(&mut self) -> bool {
        let current_line = &self.state.lines[self.state.cursor_line];
        let cursor_byte = char_to_byte(current_line, self.state.cursor_col);
        for marker in paste_markers(current_line) {
            if cursor_byte < marker.start || cursor_byte > marker.end {
                continue;
            }
            let Some(content) = self.pastes.get(&marker.id) else {
                return false;
            };
            let text = self.text();
            let line_offset = self.state.lines[..self.state.cursor_line]
                .iter()
                .map(|line| line.len() + 1)
                .sum::<usize>();
            let start = line_offset + marker.start;
            let end = line_offset + marker.end;
            let replacement = format!("{}{}{}", &text[..start], content, &text[end..]);
            self.set_text(&replacement);
            return true;
        }
        false
    }

    pub fn delete_backward(&mut self) -> bool {
        self.exit_history_browsing();
        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();
            let line = &self.state.lines[self.state.cursor_line];
            let cursor_byte = char_to_byte(line, self.state.cursor_col);
            if let Some(marker) = paste_markers(line)
                .into_iter()
                .find(|marker| marker.end == cursor_byte && self.pastes.contains_key(&marker.id))
            {
                let next_cursor_col = byte_to_char(line, marker.start);
                self.state.lines[self.state.cursor_line]
                    .replace_range(marker.start..marker.end, "");
                self.state.cursor_col = next_cursor_col;
            } else {
                let before = &line[..cursor_byte];
                let grapheme = UnicodeSegmentation::graphemes(before, true)
                    .next_back()
                    .unwrap_or("");
                let start = cursor_byte.saturating_sub(grapheme.len());
                let removed_chars = grapheme.chars().count();
                self.state.lines[self.state.cursor_line].replace_range(start..cursor_byte, "");
                self.state.cursor_col = self.state.cursor_col.saturating_sub(removed_chars);
            }
            true
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();
            let current = self.state.lines.remove(self.state.cursor_line);
            self.state.cursor_line -= 1;
            self.state.cursor_col = line_char_count(&self.state.lines[self.state.cursor_line]);
            self.state.lines[self.state.cursor_line].push_str(&current);
            true
        } else {
            false
        }
    }

    pub fn delete_forward(&mut self) -> bool {
        self.exit_history_browsing();
        let line_length = line_char_count(&self.state.lines[self.state.cursor_line]);
        if self.state.cursor_col < line_length {
            self.push_undo_snapshot();
            let line = &self.state.lines[self.state.cursor_line];
            let start = char_to_byte(line, self.state.cursor_col);
            let grapheme = UnicodeSegmentation::graphemes(&line[start..], true)
                .next()
                .unwrap_or("");
            let end = start + grapheme.len();
            self.state.lines[self.state.cursor_line].replace_range(start..end, "");
            true
        } else if self.state.cursor_line + 1 < self.state.lines.len() {
            self.push_undo_snapshot();
            let next = self.state.lines.remove(self.state.cursor_line + 1);
            self.state.lines[self.state.cursor_line].push_str(&next);
            true
        } else {
            false
        }
    }

    pub fn move_left(&mut self) {
        if self.state.cursor_col > 0 {
            self.state.cursor_col -= 1;
        } else if self.state.cursor_line > 0 {
            self.state.cursor_line -= 1;
            self.state.cursor_col = line_char_count(&self.state.lines[self.state.cursor_line]);
        }
    }

    pub fn move_right(&mut self) {
        let line_length = line_char_count(&self.state.lines[self.state.cursor_line]);
        if self.state.cursor_col < line_length {
            self.state.cursor_col += 1;
        } else if self.state.cursor_line + 1 < self.state.lines.len() {
            self.state.cursor_line += 1;
            self.state.cursor_col = 0;
        }
    }

    pub fn move_home(&mut self) {
        self.state.cursor_col = 0;
    }

    pub fn move_end(&mut self) {
        self.state.cursor_col = line_char_count(&self.state.lines[self.state.cursor_line]);
    }

    pub fn add_to_history(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() || self.history.first().is_some_and(|entry| entry == trimmed) {
            return;
        }
        self.history.insert(0, trimmed.to_owned());
        self.history.truncate(100);
    }

    pub fn set_history_filter(&mut self, filter: Option<Arc<HistoryFilter>>) {
        self.history_filter = filter;
    }

    pub fn history_previous(&mut self) -> bool {
        self.navigate_history(-1)
    }

    pub fn history_next(&mut self) -> bool {
        self.navigate_history(1)
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        self.state = previous;
        self.exit_history_browsing();
        true
    }

    /// Applies one terminal input and returns host-level callbacks that must be
    /// dispatched in order.
    ///
    /// Original: `custom-editor.ts`, `CustomEditor.handleInput()`.
    pub fn handle_input_event(&mut self, data: &str) -> EditorInputOutcome {
        let normalized = normalize_caps_locked_ctrl(data);
        if is_key_release(&normalized) {
            return EditorInputOutcome::consumed();
        }
        let mut outcome = EditorInputOutcome::default();
        let is_escape = matches_editor_key(&normalized, EditorKey::Escape);
        if !is_escape {
            outcome.actions.push(EditorAction::NonEscapeInput);
        }

        if self.consuming_paste {
            self.consume_buffer.push_str(&normalized);
            if self.consume_buffer.contains(BRACKET_PASTE_END) {
                self.consuming_paste = false;
                self.consume_buffer.clear();
            }
            outcome.consumed = true;
            return outcome;
        }

        if let Some(result) = self.handle_bracketed_paste(&normalized) {
            outcome.actions.extend(result);
            outcome.consumed = true;
            return outcome;
        }

        if is_image_paste_key(&normalized) {
            if self.expand_paste_marker_at_cursor() {
                outcome.consumed = true;
                return outcome;
            }
            outcome.actions.push(EditorAction::PasteImage);
            outcome.consumed = true;
            return outcome;
        }

        if matches_editor_key(&normalized, EditorKey::Ctrl('d')) {
            if self.text().is_empty() {
                outcome.actions.push(EditorAction::CtrlD);
            } else {
                self.delete_forward();
            }
            outcome.consumed = true;
            return outcome;
        }
        for (key, action) in [
            (EditorKey::Ctrl('c'), EditorAction::CtrlC),
            (EditorKey::Ctrl('g'), EditorAction::OpenExternalEditor),
            (EditorKey::Ctrl('o'), EditorAction::ToggleToolExpand),
            (EditorKey::Ctrl('s'), EditorAction::CtrlS),
        ] {
            if matches_editor_key(&normalized, key) {
                outcome.actions.push(action);
                outcome.consumed = true;
                return outcome;
            }
        }
        if matches_editor_key(&normalized, EditorKey::Ctrl('b')) {
            outcome
                .actions
                .push(EditorAction::CtrlBWithCursorLeftFallback);
            outcome.consumed = true;
            return outcome;
        }
        if matches_editor_key(&normalized, EditorKey::Ctrl('t')) {
            outcome
                .actions
                .push(EditorAction::ToggleTodoWithDefaultFallback);
            outcome.consumed = true;
            return outcome;
        }
        if matches_editor_key(&normalized, EditorKey::ShiftTab) {
            outcome.actions.push(EditorAction::ShiftTab);
            outcome.consumed = true;
            return outcome;
        }
        if matches_editor_key(&normalized, EditorKey::CtrlMinus) {
            outcome.actions.push(EditorAction::UndoShortcut);
            self.undo();
            outcome.consumed = true;
            return outcome;
        }

        if self.input_mode == InputMode::Bash
            && self.text().is_empty()
            && (is_escape || matches_editor_key(&normalized, EditorKey::Backspace))
        {
            self.input_mode = InputMode::Prompt;
            outcome
                .actions
                .push(EditorAction::InputModeChanged(InputMode::Prompt));
            outcome.consumed = true;
            return outcome;
        }

        if self.autocomplete_showing && !self.text().is_empty() {
            if matches_editor_key(&normalized, EditorKey::Up) {
                self.move_autocomplete_up();
                outcome.consumed = true;
                return outcome;
            }
            if matches_editor_key(&normalized, EditorKey::Down) {
                self.move_autocomplete_down();
                outcome.consumed = true;
                return outcome;
            }
        }

        if matches_editor_key(&normalized, EditorKey::Up) && self.text().is_empty() {
            outcome
                .actions
                .push(EditorAction::UpArrowEmptyWithHistoryFallback);
            outcome.consumed = true;
            return outcome;
        }
        if matches_editor_key(&normalized, EditorKey::Down) && self.text().is_empty() {
            outcome
                .actions
                .push(EditorAction::DownArrowEmptyWithHistoryFallback);
            outcome.consumed = true;
            return outcome;
        }
        if is_escape {
            if self.autocomplete_showing || self.autocomplete_request_pending {
                self.cancel_autocomplete();
                outcome.actions.push(EditorAction::AutocompleteCancelled);
            } else {
                outcome.actions.push(EditorAction::Escape);
            }
            outcome.consumed = true;
            return outcome;
        }
        if matches_editor_key(&normalized, EditorKey::Tab) {
            if self.autocomplete_showing {
                self.accept_autocomplete(false);
            }
            outcome.consumed = true;
            return outcome;
        }

        if matches_editor_key(&normalized, EditorKey::Enter) && self.autocomplete_showing {
            match self.accept_autocomplete(true) {
                AutocompleteAccept::Submitted(text) => {
                    outcome.actions.push(EditorAction::Submit(text));
                }
                AutocompleteAccept::Applied | AutocompleteAccept::NotAccepted => {}
            }
            outcome.consumed = true;
            return outcome;
        }

        let printable = printable_char(&normalized);
        if self.input_mode == InputMode::Prompt && printable == "!" && self.text().is_empty() {
            self.input_mode = InputMode::Bash;
            outcome
                .actions
                .push(EditorAction::InputModeChanged(InputMode::Bash));
            outcome.consumed = true;
            return outcome;
        }

        let autocomplete_update_force =
            self.autocomplete_showing.then_some(self.autocomplete_force);
        let state_before_input = self.state.clone();
        if matches_editor_key(&normalized, EditorKey::Backspace) {
            self.delete_backward();
        } else if matches_editor_key(&normalized, EditorKey::Delete) {
            self.delete_forward();
        } else if matches_editor_key(&normalized, EditorKey::Left) {
            self.move_left();
        } else if matches_editor_key(&normalized, EditorKey::Right) {
            self.move_right();
        } else if matches_editor_key(&normalized, EditorKey::Home) {
            self.move_home();
        } else if matches_editor_key(&normalized, EditorKey::End) {
            self.move_end();
        } else if matches_editor_key(&normalized, EditorKey::Up) {
            self.move_up();
        } else if matches_editor_key(&normalized, EditorKey::Down) {
            self.move_down();
        } else if matches_editor_key(&normalized, EditorKey::Enter) {
            let submitted = self.submit_value();
            outcome.actions.push(EditorAction::Submit(submitted));
        } else if is_printable_char(&printable) {
            self.insert_text_at_cursor(&printable);
            if autocomplete_update_force.is_none() {
                self.append_autocomplete_request(&mut outcome.actions, &printable);
            }
        } else if normalized.chars().count() > 1 && !normalized.starts_with('\u{1b}') {
            let empty_prompt = self.input_mode == InputMode::Prompt && self.text().is_empty();
            self.insert_paste(&normalized);
            self.enter_bash_from_pasted_bang(empty_prompt, &mut outcome.actions);
        } else {
            return outcome;
        }
        if self.state != state_before_input
            && let Some(force) = autocomplete_update_force
            && !outcome
                .actions
                .iter()
                .any(|action| matches!(action, EditorAction::RequestAutocomplete { .. }))
        {
            outcome
                .actions
                .push(EditorAction::RequestAutocomplete { force });
        }
        outcome.consumed = true;
        outcome
    }

    pub fn apply_ctrl_b_fallback(&mut self) {
        self.move_left();
    }

    pub fn apply_up_arrow_history_fallback(&mut self) -> bool {
        self.history_previous()
    }

    pub fn apply_down_arrow_history_fallback(&mut self) -> bool {
        self.history_next()
    }

    pub fn paste_image_as_text(&mut self, text: &str) {
        self.insert_paste(text);
    }

    fn handle_bracketed_paste(&mut self, data: &str) -> Option<Vec<EditorAction>> {
        if let Some(buffer) = &mut self.bracket_paste_buffer {
            buffer.push_str(data);
            if let Some(end) = buffer.find(BRACKET_PASTE_END) {
                let content = buffer[..end].to_owned();
                self.bracket_paste_buffer = None;
                let empty_prompt = self.input_mode == InputMode::Prompt && self.text().is_empty();
                self.insert_paste(&content);
                let mut actions = Vec::new();
                self.enter_bash_from_pasted_bang(empty_prompt, &mut actions);
                return Some(actions);
            }
            return Some(Vec::new());
        }
        let start = data.find(BRACKET_PASTE_START)?;
        if self.expand_paste_marker_at_cursor() {
            if !data[start + BRACKET_PASTE_START.len()..].contains(BRACKET_PASTE_END) {
                self.consuming_paste = true;
            }
            return Some(Vec::new());
        }
        let content_start = start + BRACKET_PASTE_START.len();
        if let Some(relative_end) = data[content_start..].find(BRACKET_PASTE_END) {
            let content = &data[content_start..content_start + relative_end];
            let empty_prompt = self.input_mode == InputMode::Prompt && self.text().is_empty();
            self.insert_paste(content);
            let mut actions = Vec::new();
            self.enter_bash_from_pasted_bang(empty_prompt, &mut actions);
            Some(actions)
        } else {
            self.bracket_paste_buffer = Some(data[content_start..].to_owned());
            Some(Vec::new())
        }
    }

    fn enter_bash_from_pasted_bang(
        &mut self,
        empty_prompt_before_input: bool,
        actions: &mut Vec<EditorAction>,
    ) {
        if empty_prompt_before_input
            && self.input_mode == InputMode::Prompt
            && self.text().starts_with('!')
        {
            self.input_mode = InputMode::Bash;
            let text = self.text();
            self.set_text_internal(
                text.strip_prefix('!').unwrap_or(&text),
                CursorPlacement::End,
            );
            actions.push(EditorAction::InputModeChanged(InputMode::Bash));
        }
    }

    fn append_autocomplete_request(&self, actions: &mut Vec<EditorAction>, inserted: &str) {
        let current = &self.state.lines[self.state.cursor_line];
        let before = char_prefix_by_count(current, self.state.cursor_col);
        if inserted == "/" {
            if self.input_mode == InputMode::Bash {
                actions.push(EditorAction::RequestAutocomplete { force: true });
            } else if before == "/" || (before.starts_with('/') && before.contains(' ')) {
                actions.push(EditorAction::RequestAutocomplete { force: false });
            }
        } else if inserted == "@"
            || (before.starts_with('/')
                && inserted.chars().all(|character| {
                    character.is_ascii_alphanumeric() || ".-_".contains(character)
                }))
        {
            actions.push(EditorAction::RequestAutocomplete { force: false });
        }
    }

    fn submit_value(&mut self) -> String {
        let result = self.expanded_text().trim().to_owned();
        self.state = EditorBufferState::default();
        self.pastes.clear();
        self.paste_counter = 0;
        self.exit_history_browsing();
        self.undo_stack.clear();
        result
    }

    fn move_up(&mut self) {
        if self.state.cursor_line > 0 {
            self.state.cursor_line -= 1;
            self.state.cursor_col = self
                .state
                .cursor_col
                .min(line_char_count(&self.state.lines[self.state.cursor_line]));
        }
    }

    fn move_down(&mut self) {
        if self.state.cursor_line + 1 < self.state.lines.len() {
            self.state.cursor_line += 1;
            self.state.cursor_col = self
                .state
                .cursor_col
                .min(line_char_count(&self.state.lines[self.state.cursor_line]));
        }
    }

    fn navigate_history(&mut self, direction: isize) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let entering = self.history_index == -1;
        let mut new_index = self.history_index;
        let found = loop {
            new_index -= direction;
            if new_index == -1 {
                break true;
            }
            if new_index < -1
                || usize::try_from(new_index).map_or(true, |index| index >= self.history.len())
            {
                break false;
            }
            let entry = &self.history[usize::try_from(new_index).unwrap_or_default()];
            if self
                .history_filter
                .as_ref()
                .is_none_or(|filter| filter(entry))
            {
                break true;
            }
        };
        if !found {
            return false;
        }
        if entering && new_index >= 0 {
            self.push_undo_snapshot();
            self.history_draft = Some(self.state.clone());
        }
        self.history_index = new_index;
        if new_index == -1 {
            let draft = self.history_draft.take().unwrap_or_default();
            self.state = draft;
        } else {
            let entry = self.history[usize::try_from(new_index).unwrap_or_default()].clone();
            let placement = if direction == -1 {
                CursorPlacement::Start
            } else {
                CursorPlacement::End
            };
            self.set_text_internal(&entry, placement);
        }
        true
    }

    fn insert_text_internal(&mut self, text: &str) {
        let normalized = normalize_editor_text(text);
        let inserted = normalized.split('\n').collect::<Vec<_>>();
        let current = self.state.lines[self.state.cursor_line].clone();
        let cursor_byte = char_to_byte(&current, self.state.cursor_col);
        let before = &current[..cursor_byte];
        let after = &current[cursor_byte..];
        if inserted.len() == 1 {
            self.state.lines[self.state.cursor_line] = format!("{before}{normalized}{after}");
            self.state.cursor_col += normalized.chars().count();
            return;
        }
        let mut replacement = Vec::new();
        replacement.push(format!("{before}{}", inserted[0]));
        replacement.extend(
            inserted[1..inserted.len() - 1]
                .iter()
                .map(|line| (*line).to_owned()),
        );
        replacement.push(format!("{}{after}", inserted.last().copied().unwrap_or("")));
        let added_lines = replacement.len() - 1;
        self.state
            .lines
            .splice(self.state.cursor_line..=self.state.cursor_line, replacement);
        self.state.cursor_line += added_lines;
        self.state.cursor_col = inserted.last().map_or(0, |line| line.chars().count());
    }

    fn set_text_internal(&mut self, text: &str, placement: CursorPlacement) {
        self.state.lines = text.split('\n').map(str::to_owned).collect();
        if self.state.lines.is_empty() {
            self.state.lines.push(String::new());
        }
        match placement {
            CursorPlacement::Start => {
                self.state.cursor_line = 0;
                self.state.cursor_col = 0;
            }
            CursorPlacement::End => {
                self.state.cursor_line = self.state.lines.len() - 1;
                self.state.cursor_col = line_char_count(&self.state.lines[self.state.cursor_line]);
            }
        }
    }

    fn push_undo_snapshot(&mut self) {
        self.undo_stack.push(self.state.clone());
    }

    fn exit_history_browsing(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
    }

    /// Renders the pi-tui editor surface, then applies Kimi's prompt, slash
    /// highlighting, argument hint, and complete side border.
    ///
    /// Original:
    ///   packages/pi-tui/src/components/editor.ts, Editor.render()
    ///   apps/kimi-code/src/tui/components/editor/custom-editor.ts,
    ///   CustomEditor.render()
    pub fn render_editor(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let max_padding = width.saturating_sub(1) / 2;
        let padding_x = EDITOR_LEFT_PADDING.min(max_padding);
        let content_width = width.saturating_sub(padding_x.saturating_mul(2)).max(1);
        let layout_width = if padding_x == 0 {
            content_width.saturating_sub(1).max(1)
        } else {
            content_width
        };
        let layout_lines = layout_editor_text(&self.state, layout_width);
        let max_visible_lines = (self.terminal_rows.saturating_mul(3) / 10).max(5);
        let cursor_line_index = layout_lines
            .iter()
            .position(|line| line.has_cursor)
            .unwrap_or_default();

        if cursor_line_index < self.scroll_offset {
            self.scroll_offset = cursor_line_index;
        } else if cursor_line_index >= self.scroll_offset + max_visible_lines {
            self.scroll_offset = cursor_line_index - max_visible_lines + 1;
        }
        let max_scroll_offset = layout_lines.len().saturating_sub(max_visible_lines);
        self.scroll_offset = self.scroll_offset.min(max_scroll_offset);

        let visible_lines = &layout_lines
            [self.scroll_offset..(self.scroll_offset + max_visible_lines).min(layout_lines.len())];
        let border_token = if self.border_highlighted {
            ColorToken::BorderFocus
        } else {
            ColorToken::Border
        };
        let paint_border = |text: &str| current_theme().fg(border_token, text);
        let left_padding = " ".repeat(padding_x);
        let right_padding = left_padding.clone();
        let mut lines = Vec::with_capacity(visible_lines.len() + 2);

        if self.scroll_offset > 0 {
            lines.push(paint_border(&scroll_separator(
                width,
                '↑',
                self.scroll_offset,
            )));
        } else {
            lines.push(paint_border(&"─".repeat(width)));
        }

        for layout_line in visible_lines {
            let mut display_text = layout_line.text.clone();
            let mut line_visible_width = visible_width(&display_text);
            let mut cursor_in_padding = false;
            if layout_line.has_cursor {
                let cursor_col = layout_line.cursor_col.unwrap_or_default();
                let cursor_byte = char_to_byte(&display_text, cursor_col);
                let before = &display_text[..cursor_byte];
                let after = &display_text[cursor_byte..];
                let marker = if self.focused { CURSOR_MARKER } else { "" };
                if let Some(grapheme) = UnicodeSegmentation::graphemes(after, true).next() {
                    let rest = &after[grapheme.len()..];
                    display_text = format!("{before}{marker}\u{1b}[7m{grapheme}\u{1b}[0m{rest}");
                } else {
                    display_text = format!("{before}{marker}{CURSOR_BLOCK}");
                    line_visible_width += 1;
                    cursor_in_padding = line_visible_width > content_width && padding_x > 0;
                }
            }
            let padding = " ".repeat(content_width.saturating_sub(line_visible_width));
            let line_right_padding = if cursor_in_padding {
                right_padding.get(1..).unwrap_or_default()
            } else {
                &right_padding
            };
            lines.push(format!(
                "{left_padding}{display_text}{padding}{line_right_padding}"
            ));
        }

        let lines_below = layout_lines
            .len()
            .saturating_sub(self.scroll_offset + visible_lines.len());
        if lines_below > 0 {
            lines.push(paint_border(&scroll_separator(width, '↓', lines_below)));
        } else {
            lines.push(paint_border(&"─".repeat(width)));
        }

        if let Some(list) = &mut self.autocomplete_list {
            for line in list.render(content_width) {
                let padding = " ".repeat(content_width.saturating_sub(visible_width(&line)));
                lines.push(format!("{left_padding}{line}{padding}{right_padding}"));
            }
        }

        let first_content_index = 1;
        let is_bash = self.input_mode == InputMode::Bash;
        if !is_bash
            && self.text().trim_start().starts_with('/')
            && let Some(line) = lines.get(first_content_index)
            && let Some(highlighted) = highlight_first_slash_token(line)
        {
            lines[first_content_index] = highlighted;
        }
        if let Some(hint) = self.compute_argument_hint()
            && let Some(line) = lines.get(first_content_index)
        {
            lines[first_content_index] =
                inject_argument_hint(line, &hint, self.text().chars().count(), width);
        }
        if let Some(line) = lines.get(first_content_index) {
            let prompt_paint = |text: &str| paint_border(text);
            if let Some(with_prompt) = inject_prompt_symbol(
                line,
                if is_bash { "!" } else { ">" },
                is_bash.then_some(&prompt_paint as &dyn Fn(&str) -> String),
            ) {
                lines[first_content_index] = with_prompt;
            }
        }

        let label = is_bash.then(|| {
            format!(
                " {} ",
                current_theme().bold_fg(ColorToken::ShellMode, "! shell mode")
            )
        });
        wrap_with_side_borders(
            &lines,
            &paint_border,
            SideBorderOptions {
                connected_above: self.connected_above && !self.border_highlighted,
                label: label.as_deref(),
            },
        )
    }

    fn compute_argument_hint(&self) -> Option<String> {
        if self.input_mode == InputMode::Bash {
            return None;
        }
        let text = self.text();
        let captures = Regex::new(r"^/(\S+)( ?)$")
            .expect("valid argument hint regex")
            .captures(&text)?;
        let command = captures.get(1)?.as_str();
        let hint = self.argument_hints.get(command)?;
        if self.state.cursor_line != 0
            || self.state.cursor_col != line_char_count(&self.state.lines[0])
        {
            return None;
        }
        if captures
            .get(2)
            .is_some_and(|value| !value.as_str().is_empty())
        {
            Some(hint.clone())
        } else {
            Some(format!(" {hint}"))
        }
    }

    fn clear_autocomplete_ui(&mut self) {
        self.autocomplete_showing = false;
        self.autocomplete_list = None;
        self.autocomplete_prefix.clear();
    }

    fn apply_autocomplete_suggestions(
        &mut self,
        suggestions: AutocompleteSuggestions,
        force: bool,
    ) {
        let selected_index = best_autocomplete_match_index(&suggestions.items, &suggestions.prefix);
        let slash_menu = suggestions.prefix.starts_with('/');
        let items = suggestions
            .items
            .into_iter()
            .map(|item| SelectItem {
                value: item.value,
                label: Some(item.label),
                description: item.description,
            })
            .collect();
        let layout = if slash_menu {
            SelectListLayout {
                min_primary_column_width: Some(12),
                max_primary_column_width: Some(32),
            }
        } else {
            SelectListLayout::default()
        };
        let mut list = WrappingSelectList::new(
            items,
            self.autocomplete_max_visible,
            autocomplete_select_theme(),
            layout,
        );
        if let Some(index) = selected_index {
            list.set_selected_index(index);
        }
        self.autocomplete_prefix = suggestions.prefix;
        self.autocomplete_force = force;
        self.autocomplete_list = Some(list);
        self.autocomplete_showing = true;
    }

    fn apply_completion_item(&mut self, item: &AutocompleteItem, prefix: &str) -> bool {
        let Some(provider) = self.autocomplete_provider.as_ref() else {
            return false;
        };
        let edit = provider.apply_completion(
            &self.state.lines,
            self.state.cursor_line,
            self.state.cursor_col,
            item,
            prefix,
        );
        self.push_undo_snapshot();
        self.state.lines = edit.lines;
        self.state.cursor_line = edit
            .cursor_line
            .min(self.state.lines.len().saturating_sub(1));
        self.state.cursor_col = edit
            .cursor_col
            .min(line_char_count(&self.state.lines[self.state.cursor_line]));
        self.exit_history_browsing();
        true
    }
}

fn best_autocomplete_match_index(items: &[AutocompleteItem], prefix: &str) -> Option<usize> {
    if prefix.is_empty() {
        return None;
    }
    let mut first_prefix = None;
    for (index, item) in items.iter().enumerate() {
        if item.value == prefix {
            return Some(index);
        }
        if first_prefix.is_none() && item.value.starts_with(prefix) {
            first_prefix = Some(index);
        }
    }
    first_prefix
}

fn autocomplete_select_theme() -> SelectListTheme {
    SelectListTheme {
        selected_text: Arc::new(|text| current_theme().fg(ColorToken::Primary, text)),
        description: Arc::new(|text| current_theme().fg(ColorToken::TextMuted, text)),
        scroll_info: Arc::new(|text| current_theme().fg(ColorToken::TextMuted, text)),
        no_match: Arc::new(|text| current_theme().fg(ColorToken::TextMuted, text)),
    }
}

impl Component for CustomEditor {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_editor(width)
    }

    fn handle_input(&mut self, data: &str) {
        let outcome = self.handle_input_event(data);
        self.pending_actions.extend(outcome.actions);
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditorLayoutLine {
    text: String,
    has_cursor: bool,
    cursor_col: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextChunk {
    text: String,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy)]
struct GraphemeLayout<'a> {
    text: &'a str,
    start: usize,
    width: usize,
    whitespace: bool,
}

fn layout_editor_text(state: &EditorBufferState, width: usize) -> Vec<EditorLayoutLine> {
    if state.lines.is_empty() || state.lines.len() == 1 && state.lines[0].is_empty() {
        return vec![EditorLayoutLine {
            text: String::new(),
            has_cursor: true,
            cursor_col: Some(0),
        }];
    }

    let mut layout = Vec::new();
    for (line_index, line) in state.lines.iter().enumerate() {
        let is_current_line = line_index == state.cursor_line;
        let chunks = word_wrap_editor_line(line, width);
        for (chunk_index, chunk) in chunks.iter().enumerate() {
            let is_last_chunk = chunk_index + 1 == chunks.len();
            let cursor = state.cursor_col;
            let has_cursor = is_current_line
                && if is_last_chunk {
                    cursor >= chunk.start
                } else {
                    cursor >= chunk.start && cursor < chunk.end
                };
            let cursor_col = has_cursor.then(|| {
                cursor
                    .saturating_sub(chunk.start)
                    .min(chunk.text.chars().count())
            });
            layout.push(EditorLayoutLine {
                text: chunk.text.clone(),
                has_cursor,
                cursor_col,
            });
        }
    }
    layout
}

/// Word-aware counterpart of pi-tui's `wordWrapLine()` that records source
/// character ranges so a cursor can be mapped onto the correct visual row.
fn word_wrap_editor_line(line: &str, max_width: usize) -> Vec<TextChunk> {
    let max_width = max_width.max(1);
    let line_length = line.chars().count();
    if line.is_empty() || visible_width(line) <= max_width {
        return vec![TextChunk {
            text: line.to_owned(),
            start: 0,
            end: line_length,
        }];
    }

    let mut char_index = 0;
    let segments = UnicodeSegmentation::graphemes(line, true)
        .map(|grapheme| {
            let start = char_index;
            char_index += grapheme.chars().count();
            GraphemeLayout {
                text: grapheme,
                start,
                width: visible_width(grapheme),
                whitespace: grapheme.chars().all(char::is_whitespace),
            }
        })
        .collect::<Vec<_>>();
    let mut chunks = Vec::new();
    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    let mut wrap_opportunity: Option<(usize, usize)> = None;

    for (index, segment) in segments.iter().enumerate() {
        if current_width + segment.width > max_width {
            if let Some((wrap_index, wrap_width)) = wrap_opportunity
                && current_width.saturating_sub(wrap_width) + segment.width <= max_width
            {
                chunks.push(text_chunk(line, chunk_start, wrap_index));
                chunk_start = wrap_index;
                current_width = current_width.saturating_sub(wrap_width);
            } else if chunk_start < segment.start {
                chunks.push(text_chunk(line, chunk_start, segment.start));
                chunk_start = segment.start;
                current_width = 0;
            }
            wrap_opportunity = None;
        }

        if segment.width > max_width {
            current_width = segment.width;
            wrap_opportunity = None;
            continue;
        }

        current_width += segment.width;
        if let Some(next) = segments.get(index + 1) {
            let whitespace_boundary = segment.whitespace && !next.whitespace;
            let cjk_boundary = !segment.whitespace
                && !next.whitespace
                && (is_cjk_break(segment.text) || is_cjk_break(next.text));
            if whitespace_boundary || cjk_boundary {
                wrap_opportunity = Some((next.start, current_width));
            }
        }
    }
    chunks.push(text_chunk(line, chunk_start, line_length));
    chunks
}

fn text_chunk(line: &str, start: usize, end: usize) -> TextChunk {
    TextChunk {
        text: char_range(line, start, end).to_owned(),
        start,
        end,
    }
}

fn char_range(text: &str, start: usize, end: usize) -> &str {
    let start_byte = char_to_byte(text, start);
    let end_byte = char_to_byte(text, end);
    &text[start_byte..end_byte]
}

fn is_cjk_break(grapheme: &str) -> bool {
    grapheme.chars().any(|character| {
        matches!(
            character as u32,
            0x2e80..=0x2fff
                | 0x3040..=0x30ff
                | 0x31f0..=0x31ff
                | 0x3400..=0x4dbf
                | 0x4e00..=0x9fff
                | 0xac00..=0xd7af
                | 0xf900..=0xfaff
                | 0x20000..=0x2fa1f
        )
    })
}

fn scroll_separator(width: usize, arrow: char, count: usize) -> String {
    let indicator = format!("─── {arrow} {count} more ");
    let indicator_width = visible_width(&indicator);
    if indicator_width <= width {
        format!("{indicator}{}", "─".repeat(width - indicator_width))
    } else {
        truncate_plain_width(&indicator, width)
    }
}

fn truncate_plain_width(text: &str, width: usize) -> String {
    let mut output = String::new();
    let mut used = 0;
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let grapheme_width = visible_width(grapheme);
        if used + grapheme_width > width {
            break;
        }
        output.push_str(grapheme);
        used += grapheme_width;
    }
    output
}

const BRACKET_PASTE_START: &str = "\u{1b}[200~";
const BRACKET_PASTE_END: &str = "\u{1b}[201~";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorAction {
    NonEscapeInput,
    CtrlD,
    CtrlC,
    OpenExternalEditor,
    ToggleToolExpand,
    CtrlS,
    CtrlBWithCursorLeftFallback,
    ToggleTodoWithDefaultFallback,
    ShiftTab,
    UndoShortcut,
    UpArrowEmptyWithHistoryFallback,
    DownArrowEmptyWithHistoryFallback,
    Escape,
    PasteImage,
    InputModeChanged(InputMode),
    AutocompleteCancelled,
    RequestAutocomplete { force: bool },
    Submit(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EditorInputOutcome {
    pub consumed: bool,
    pub actions: Vec<EditorAction>,
}

impl EditorInputOutcome {
    fn consumed() -> Self {
        Self {
            consumed: true,
            actions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CursorPlacement {
    Start,
    End,
}

#[derive(Debug, Clone, Copy)]
struct PasteMarker {
    id: u64,
    start: usize,
    end: usize,
}

fn normalize_editor_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
}

fn decode_paste_control_sequences(text: &str) -> String {
    let regex = Regex::new(r"\x1b\[(\d+);5u").expect("valid paste control regex");
    regex
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let codepoint = captures[1].parse::<u8>().unwrap_or_default();
            if codepoint.is_ascii_alphabetic() {
                char::from(codepoint.to_ascii_lowercase() - b'a' + 1).to_string()
            } else {
                captures[0].to_owned()
            }
        })
        .into_owned()
}

fn expand_paste_markers(text: &str, pastes: &HashMap<u64, String>) -> String {
    let mut output = text.to_owned();
    for (id, content) in pastes {
        let regex = Regex::new(&format!(
            r"\[paste #{}(?: (?:\+\d+ lines|\d+ chars))?\]",
            regex::escape(&id.to_string())
        ))
        .expect("paste id produces a valid regex");
        output = regex.replace_all(&output, content.as_str()).into_owned();
    }
    output
}

fn paste_markers(text: &str) -> Vec<PasteMarker> {
    let regex = Regex::new(r"\[paste #(\d+)(?: (?:\+\d+ lines|\d+ chars))?\]")
        .expect("valid paste marker regex");
    regex
        .captures_iter(text)
        .filter_map(|captures| {
            let matched = captures.get(0)?;
            Some(PasteMarker {
                id: captures.get(1)?.as_str().parse().ok()?,
                start: matched.start(),
                end: matched.end(),
            })
        })
        .collect()
}

fn line_char_count(line: &str) -> usize {
    line.chars().count()
}

fn char_to_byte(text: &str, character_index: usize) -> usize {
    text.char_indices()
        .nth(character_index)
        .map_or(text.len(), |(index, _)| index)
}

fn byte_to_char(text: &str, byte_index: usize) -> usize {
    text[..byte_index.min(text.len())].chars().count()
}

fn char_at(text: &str, index: usize) -> Option<char> {
    text.chars().nth(index)
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn char_prefix_by_count(text: &str, count: usize) -> &str {
    let end = char_to_byte(text, count);
    &text[..end]
}

fn is_image_paste_key(data: &str) -> bool {
    #[cfg(windows)]
    {
        matches!(data, "\u{1b}v" | "\u{1b}[118;3u")
    }
    #[cfg(not(windows))]
    {
        matches_editor_key(data, EditorKey::Ctrl('v'))
    }
}

/// Normalizes Kitty CSI-u Ctrl+letter events reported with Caps Lock.
///
/// Original: `custom-editor.ts`, `normalizeCapsLockedCtrl()`.
pub fn normalize_caps_locked_ctrl(data: &str) -> String {
    let Some(body) = data
        .strip_prefix("\u{1b}[")
        .and_then(|value| value.strip_suffix('u'))
    else {
        return data.to_owned();
    };
    let Some((codepoint_text, modifier_and_tail)) = body.split_once(';') else {
        return data.to_owned();
    };
    if codepoint_text.is_empty()
        || !codepoint_text.bytes().all(|byte| byte.is_ascii_digit())
        || modifier_and_tail.contains(';')
    {
        return data.to_owned();
    }
    let modifier_end = modifier_and_tail
        .find(':')
        .unwrap_or(modifier_and_tail.len());
    let modifier_text = &modifier_and_tail[..modifier_end];
    let tail = &modifier_and_tail[modifier_end..];
    if modifier_text.is_empty()
        || !modifier_text.bytes().all(|byte| byte.is_ascii_digit())
        || !valid_kitty_tail(tail)
    {
        return data.to_owned();
    }
    let (Ok(codepoint), Ok(modifier_plus_one)) =
        (codepoint_text.parse::<u32>(), modifier_text.parse::<u32>())
    else {
        return data.to_owned();
    };
    let Some(modifier) = modifier_plus_one.checked_sub(1) else {
        return data.to_owned();
    };
    if modifier & CAPS_LOCK_BIT == 0
        || modifier & CTRL_BIT == 0
        || modifier & SHIFT_BIT != 0
        || !(65..=90).contains(&codepoint)
    {
        return data.to_owned();
    }
    let lowered_codepoint = codepoint + 32;
    let stripped_modifier = (modifier & !CAPS_LOCK_BIT) + 1;
    format!("\u{1b}[{lowered_codepoint};{stripped_modifier}{tail}u")
}

fn valid_kitty_tail(tail: &str) -> bool {
    tail.is_empty()
        || (tail.starts_with(':')
            && tail
                .split(':')
                .skip(1)
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
}

/// Highlights the leading slash command and the `/goal next manage` command
/// path while preserving existing ANSI escapes.
///
/// Original: `custom-editor.ts`, `highlightFirstSlashToken()`.
pub fn highlight_first_slash_token(line: &str) -> Option<String> {
    let visible = strip_sgr(line);
    let characters = visible.chars().collect::<Vec<_>>();
    let slash_index = characters.iter().position(|character| *character == '/')?;
    if characters[..slash_index]
        .iter()
        .any(|character| !matches!(character, ' ' | '\t'))
    {
        return None;
    }
    let mut end = slash_index + 1;
    while end < characters.len() && !is_token_space(characters[end]) {
        end += 1;
    }
    let visible_token = characters[slash_index..end].iter().collect::<String>();
    if visible_token[1..].contains('/') {
        return None;
    }
    let mut ranges = vec![(slash_index, end)];
    if visible_token == "/goal" {
        ranges.extend(goal_command_path_ranges(&characters, end));
    }
    Some(highlight_visible_ranges(line, &ranges))
}

fn goal_command_path_ranges(characters: &[char], command_end: usize) -> Vec<(usize, usize)> {
    let Some(next) = read_token_range(characters, command_end) else {
        return Vec::new();
    };
    if characters[next.0..next.1].iter().collect::<String>() != "next" {
        return Vec::new();
    }
    let mut ranges = vec![next];
    if let Some(manage) = read_token_range(characters, next.1)
        && characters[manage.0..manage.1].iter().collect::<String>() == "manage"
    {
        ranges.push(manage);
    }
    ranges
}

fn read_token_range(characters: &[char], start: usize) -> Option<(usize, usize)> {
    let mut token_start = start;
    while token_start < characters.len() && is_token_space(characters[token_start]) {
        token_start += 1;
    }
    if token_start >= characters.len() {
        return None;
    }
    let mut token_end = token_start;
    while token_end < characters.len() && !is_token_space(characters[token_end]) {
        token_end += 1;
    }
    Some((token_start, token_end))
}

fn is_token_space(character: char) -> bool {
    matches!(character, ' ' | '\t')
}

fn highlight_visible_ranges(line: &str, ranges: &[(usize, usize)]) -> String {
    let mut output = String::new();
    let mut raw_cursor = 0;
    for &(start, end) in ranges {
        let raw_start = map_visible_index_to_raw(line, start);
        let raw_end = map_visible_index_to_raw(line, end);
        output.push_str(&line[raw_cursor..raw_start]);
        output.push_str(&current_theme().bold_fg(ColorToken::Primary, &line[raw_start..raw_end]));
        raw_cursor = raw_end;
    }
    output.push_str(&line[raw_cursor..]);
    output
}

pub fn inject_argument_hint(
    line: &str,
    hint: &str,
    real_text_length: usize,
    width: usize,
) -> String {
    let cursor_index = line.find(CURSOR_BLOCK);
    let content_width = width
        .saturating_sub(EDITOR_LEFT_PADDING.saturating_mul(2))
        .max(1);
    let available = content_width
        .saturating_sub(real_text_length)
        .saturating_sub(usize::from(cursor_index.is_some()));
    let trimmed = truncate_hint(hint, available);
    if trimmed.is_empty() {
        return line.to_owned();
    }
    let insert_at = cursor_index.map_or_else(
        || map_visible_index_to_raw(line, EDITOR_LEFT_PADDING + real_text_length),
        |index| index + CURSOR_BLOCK.len(),
    );
    let trailing_width = visible_width(&line[insert_at..]);
    let remaining = trailing_width.saturating_sub(visible_width(&trimmed));
    format!(
        "{}{}{}",
        &line[..insert_at],
        current_theme().fg(ColorToken::TextDim, &trimmed),
        " ".repeat(remaining)
    )
}

fn truncate_hint(hint: &str, max_length: usize) -> String {
    if max_length == 0 {
        return String::new();
    }
    let characters = hint.chars().collect::<Vec<_>>();
    if characters.len() <= max_length {
        return hint.to_owned();
    }
    if max_length == 1 {
        return "…".to_owned();
    }
    format!(
        "{}…",
        characters[..max_length - 1].iter().collect::<String>()
    )
}

/// Overlays a prompt token in the editor's four-cell left padding.
pub fn inject_prompt_symbol(
    line: &str,
    symbol: &str,
    paint: Option<&dyn Fn(&str) -> String>,
) -> Option<String> {
    if line.len() < 4 || line.as_bytes()[..4].iter().any(|byte| *byte != b' ') {
        return None;
    }
    let rendered = paint.map_or_else(|| symbol.to_owned(), |paint| paint(symbol));
    Some(format!("  {rendered} {}", &line[4..]))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SideBorderOptions<'a> {
    pub connected_above: bool,
    pub label: Option<&'a str>,
}

/// Converts the base editor's horizontal separators and padded content into a
/// complete box while leaving inner ANSI styling intact.
pub fn wrap_with_side_borders(
    lines: &[String],
    paint: &dyn Fn(&str) -> String,
    options: SideBorderOptions<'_>,
) -> Vec<String> {
    let mut seen_top = false;
    lines
        .iter()
        .map(|line| {
            let plain = strip_sgr(line);
            if plain.starts_with('─') {
                let is_top = !seen_top;
                let (left_corner, right_corner) = if seen_top {
                    ('╰', '╯')
                } else if options.connected_above {
                    ('├', '┤')
                } else {
                    ('╭', '╮')
                };
                seen_top = true;
                let plain_chars = plain.chars().collect::<Vec<_>>();
                if plain_chars.len() == 1 {
                    return paint(&left_corner.to_string());
                }
                let middle = plain_chars[1..plain_chars.len() - 1]
                    .iter()
                    .collect::<String>();
                if is_top
                    && let Some(label) = options.label
                    && middle.chars().all(|character| character == '─')
                    && visible_width(label) <= middle.chars().count()
                {
                    return format!(
                        "{}{}{}{}",
                        paint(&left_corner.to_string()),
                        label,
                        paint(&"─".repeat(middle.chars().count() - visible_width(label))),
                        paint(&right_corner.to_string())
                    );
                }
                return paint(&format!("{left_corner}{middle}{right_corner}"));
            }
            if line.is_empty() {
                return String::new();
            }
            let first = line.chars().next().unwrap_or_default();
            let last = line.chars().next_back().unwrap_or_default();
            let first_end = first.len_utf8();
            let last_start = line.len() - last.len_utf8();
            let head = if first == ' ' {
                paint("│")
            } else {
                first.to_string()
            };
            if first_end >= line.len() {
                return head;
            }
            let tail = if last == ' ' {
                paint("│")
            } else {
                last.to_string()
            };
            format!("{head}{}{tail}", &line[first_end..last_start])
        })
        .collect()
}

fn strip_sgr(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = sgr_end(text, index) {
            index = end;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or_default();
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn map_visible_index_to_raw(line: &str, visible_index: usize) -> usize {
    let mut visible_count = 0;
    let mut index = 0;
    while index < line.len() && visible_count < visible_index {
        if let Some(end) = sgr_end(line, index) {
            index = end;
            continue;
        }
        let character = line[index..].chars().next().unwrap_or_default();
        index += character.len_utf8();
        visible_count += 1;
    }
    index
}

fn sgr_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&0x1b) || bytes.get(start + 1) != Some(&b'[') {
        return None;
    }
    let mut index = start + 2;
    while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b';') {
        index += 1;
    }
    (bytes.get(index) == Some(&b'm')).then_some(index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::editor::file_mention_provider::{
        SlashAutocompleteCommand, SlashCommandMetadata,
    };

    fn test_autocomplete_provider() -> Arc<FileMentionProvider> {
        let commands = ["help", "goal"]
            .into_iter()
            .map(|name| {
                SlashAutocompleteCommand::new(SlashCommandMetadata {
                    name: name.to_owned(),
                    aliases: Vec::new(),
                    description: Some(format!("{name} command")),
                    argument_hint: None,
                })
            })
            .collect();
        Arc::new(FileMentionProvider::new(
            commands,
            std::path::PathBuf::from("."),
            None,
            Vec::new(),
            Arc::new(|| InputMode::Prompt),
        ))
    }

    #[test]
    fn normalizes_caps_locked_ctrl_kitty_sequences() {
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;69u"), "\u{1b}[100;5u");
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[67;69u"), "\u{1b}[99;5u");
        assert_eq!(
            normalize_caps_locked_ctrl("\u{1b}[68;69:3:1u"),
            "\u{1b}[100;5:3:1u"
        );
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;65u"), "\u{1b}[68;65u");
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;5u"), "\u{1b}[68;5u");
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;70u"), "\u{1b}[68;70u");
        assert_eq!(normalize_caps_locked_ctrl("\u{1b}[68;71u"), "\u{1b}[100;7u");
        for value in [
            "\u{1b}[49;69u",
            "\u{1b}[91;69u",
            "H",
            "\u{4}",
            "\u{1b}[A",
            "",
        ] {
            assert_eq!(normalize_caps_locked_ctrl(value), value);
        }
    }

    #[test]
    fn highlights_leading_slash_and_goal_path_without_losing_ansi() {
        let output = highlight_first_slash_token("  /help rest").expect("slash should highlight");
        assert_eq!(strip_sgr(&output), "  /help rest");
        assert!(output.contains("/help"));

        let line = "/goal next manage Ship\u{1b}[7m \u{1b}[0m";
        let output = highlight_first_slash_token(line).expect("goal path should highlight");
        assert_eq!(strip_sgr(&output), strip_sgr(line));
        assert!(output.matches("38;2").count() >= 3);
        assert!(output.contains("\u{1b}[7m"));
        assert!(highlight_first_slash_token("hello /not-cmd").is_none());
        assert!(highlight_first_slash_token("/user/desktop foo").is_none());
        assert!(highlight_first_slash_token("hello world").is_none());
    }

    #[test]
    fn injects_prompt_symbol_and_optional_paint_in_padding() {
        assert_eq!(
            inject_prompt_symbol("    hello world", ">", None).as_deref(),
            Some("  > hello world")
        );
        assert_eq!(
            inject_prompt_symbol("    hello", "!", Some(&|text| format!("<{text}>"))).as_deref(),
            Some("  <!> hello")
        );
        assert!(inject_prompt_symbol("   ", ">", None).is_none());
        assert!(inject_prompt_symbol("  x hello", ">", None).is_none());
    }

    #[test]
    fn injects_and_truncates_argument_hint_without_changing_width() {
        let line = "    /goal \u{1b}[7m \u{1b}[0m                    ";
        let output = inject_argument_hint(line, "[status|cancel]", 6, 32);
        assert!(strip_sgr(&output).contains("/goal  [status|cancel]"));
        assert_eq!(visible_width(&output), visible_width(line));

        let short = inject_argument_hint("    /g      ", "long hint", 2, 12);
        assert!(strip_sgr(&short).contains('…'));
        assert_eq!(visible_width(&short), 12);
        assert_eq!(inject_argument_hint("    full", "hint", 4, 8), "    full");
    }

    #[test]
    fn wraps_horizontal_and_content_rows_with_box_borders() {
        let lines = vec!["─".repeat(10), "   hi     ".to_owned(), "─".repeat(10)];
        let output = wrap_with_side_borders(&lines, &str::to_owned, SideBorderOptions::default());
        assert_eq!(output[0], "╭────────╮");
        assert_eq!(output[1], "│  hi    │");
        assert_eq!(output[2], "╰────────╯");

        let connected = wrap_with_side_borders(
            &lines,
            &str::to_owned,
            SideBorderOptions {
                connected_above: true,
                label: None,
            },
        );
        assert!(connected[0].starts_with('├'));
        assert!(connected[0].ends_with('┤'));
    }

    #[test]
    fn preserves_inner_styles_outer_content_and_paints_edges() {
        let paint = |text: &str| format!("<{text}>");
        let lines = vec![
            "─".repeat(5),
            "  x  ".to_owned(),
            "  abc".to_owned(),
            "─".repeat(5),
            "   item1  ".to_owned(),
        ];
        let output = wrap_with_side_borders(&lines, &paint, SideBorderOptions::default());
        assert_eq!(output[0], "<╭───╮>");
        assert_eq!(output[1], "<│> x <│>");
        assert_eq!(output[2], "<│> abc");
        assert_eq!(output[4], "<│>  item1 <│>");
    }

    #[test]
    fn overlays_label_only_on_plain_top_border_when_it_fits() {
        let border = "─".repeat(30);
        let lines = vec![border.clone(), "   x   ".to_owned(), border];
        let output = wrap_with_side_borders(
            &lines,
            &str::to_owned,
            SideBorderOptions {
                connected_above: false,
                label: Some(" ! shell mode "),
            },
        );
        assert!(output[0].starts_with("╭ ! shell mode "));
        assert_eq!(output[0].chars().count(), 30);
        assert!(!output[2].contains("shell mode"));

        let scroll = vec!["─── ↑ 5 more ────".to_owned()];
        let output = wrap_with_side_borders(
            &scroll,
            &str::to_owned,
            SideBorderOptions {
                connected_above: false,
                label: Some(" ! shell mode "),
            },
        );
        assert!(output[0].contains("↑ 5 more"));
        assert!(!output[0].contains("shell mode"));
    }

    #[test]
    fn editor_normalizes_sets_and_inserts_multiline_unicode_text() {
        let mut editor = CustomEditor::new();
        editor.set_text("你好\r\nworld\t!");
        assert_eq!(editor.text(), "你好\nworld    !");
        assert_eq!(editor.cursor(), (1, 10));

        editor.set_cursor(0, 1);
        editor.insert_text_at_cursor("X\rY");
        assert_eq!(editor.text(), "你X\nY好\nworld    !");
        assert_eq!(editor.cursor(), (1, 1));
        assert!(editor.undo());
        assert_eq!(editor.text(), "你好\nworld    !");
    }

    #[test]
    fn editor_deletes_graphemes_and_joins_lines() {
        let mut editor = CustomEditor::new();
        editor.set_text("ÁB\nnext");
        editor.set_cursor(0, 2);
        assert!(editor.delete_backward());
        assert_eq!(editor.text(), "B\nnext");
        editor.set_cursor(1, 0);
        assert!(editor.delete_backward());
        assert_eq!(editor.text(), "Bnext");
        editor.set_cursor(0, 1);
        assert!(editor.delete_forward());
        assert_eq!(editor.text(), "Bext");
        assert!(editor.undo());
        assert_eq!(editor.text(), "Bnext");
    }

    #[test]
    fn editor_tracks_expands_and_atomically_deletes_large_pastes() {
        let content = (1..=11)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = CustomEditor::new();
        editor.insert_paste(&content);
        assert_eq!(editor.text(), "[paste #1 +11 lines]");
        assert_eq!(editor.expanded_text(), content);

        editor.set_cursor(0, editor.text().chars().count());
        assert!(editor.delete_backward());
        assert_eq!(editor.text(), "");
        assert!(editor.undo());
        assert_eq!(editor.text(), "[paste #1 +11 lines]");

        editor.set_cursor(0, 3);
        assert!(editor.expand_paste_marker_at_cursor());
        assert_eq!(editor.text(), content);
        assert_eq!(editor.cursor(), (10, "line 11".chars().count()));
    }

    #[test]
    fn editor_paste_normalizes_controls_paths_and_large_character_markers() {
        let mut editor = CustomEditor::new();
        editor.set_text("word");
        editor.insert_paste("/tmp\r\nfile\tname\u{1}");
        assert_eq!(editor.text(), "word /tmp\nfile    name");

        let mut large = CustomEditor::new();
        large.insert_paste(&"x".repeat(1001));
        assert_eq!(large.text(), "[paste #1 1001 chars]");
        assert_eq!(large.expanded_text(), "x".repeat(1001));

        let mut encoded = CustomEditor::new();
        encoded.insert_paste("a\u{1b}[106;5ub");
        assert_eq!(encoded.text(), "a\nb");
    }

    #[test]
    fn editor_history_deduplicates_filters_and_restores_draft() {
        let mut editor = CustomEditor::new();
        editor.add_to_history("");
        editor.add_to_history(" first ");
        editor.add_to_history("first");
        editor.add_to_history("!shell");
        editor.add_to_history("second");
        editor.set_text("draft");
        editor.set_cursor(0, 2);
        editor.set_history_filter(Some(Arc::new(|entry| !entry.starts_with('!'))));

        assert!(editor.history_previous());
        assert_eq!(editor.text(), "second");
        assert_eq!(editor.cursor(), (0, 0));
        assert!(editor.history_previous());
        assert_eq!(editor.text(), "first");
        assert!(!editor.history_previous());
        assert!(editor.history_next());
        assert_eq!(editor.text(), "second");
        assert!(editor.history_next());
        assert_eq!(editor.text(), "draft");
        assert_eq!(editor.cursor(), (0, 2));
    }

    #[test]
    fn editor_cursor_movement_crosses_line_boundaries() {
        let mut editor = CustomEditor::new();
        editor.set_text("ab\n你好");
        editor.set_cursor(1, 0);
        editor.move_left();
        assert_eq!(editor.cursor(), (0, 2));
        editor.move_right();
        assert_eq!(editor.cursor(), (1, 0));
        editor.move_end();
        assert_eq!(editor.cursor(), (1, 2));
        editor.move_home();
        assert_eq!(editor.cursor(), (1, 0));
    }

    #[test]
    fn input_dispatches_shortcuts_and_ignores_kitty_releases() {
        let mut editor = CustomEditor::new();
        let release = editor.handle_input_event("\u{1b}[99;5:3u");
        assert!(release.consumed);
        assert!(release.actions.is_empty());

        let ctrl_c = editor.handle_input_event("\u{3}");
        assert_eq!(
            ctrl_c.actions,
            [EditorAction::NonEscapeInput, EditorAction::CtrlC]
        );
        assert!(
            editor
                .handle_input_event("\u{7}")
                .actions
                .contains(&EditorAction::OpenExternalEditor)
        );
        assert!(
            editor
                .handle_input_event("\u{f}")
                .actions
                .contains(&EditorAction::ToggleToolExpand)
        );
        assert!(
            editor
                .handle_input_event("\u{13}")
                .actions
                .contains(&EditorAction::CtrlS)
        );

        editor.set_text("ab");
        let ctrl_b = editor.handle_input_event("\u{2}");
        assert!(
            ctrl_b
                .actions
                .contains(&EditorAction::CtrlBWithCursorLeftFallback)
        );
        editor.apply_ctrl_b_fallback();
        assert_eq!(editor.cursor(), (0, 1));
    }

    #[test]
    fn input_switches_bash_mode_for_typed_and_pasted_bang() {
        let mut editor = CustomEditor::new();
        let typed = editor.handle_input_event("!");
        assert_eq!(editor.input_mode(), InputMode::Bash);
        assert_eq!(editor.text(), "");
        assert!(
            typed
                .actions
                .contains(&EditorAction::InputModeChanged(InputMode::Bash))
        );
        let exited = editor.handle_input_event("\u{7f}");
        assert_eq!(editor.input_mode(), InputMode::Prompt);
        assert!(
            exited
                .actions
                .contains(&EditorAction::InputModeChanged(InputMode::Prompt))
        );

        let pasted = editor.handle_input_event("\u{1b}[200~!echo hi\u{1b}[201~");
        assert_eq!(editor.input_mode(), InputMode::Bash);
        assert_eq!(editor.text(), "echo hi");
        assert!(
            pasted
                .actions
                .contains(&EditorAction::InputModeChanged(InputMode::Bash))
        );

        editor.set_input_mode(InputMode::Prompt);
        editor.set_text("prefix ");
        editor.handle_input_event("\u{1b}[200~!literal\u{1b}[201~");
        assert_eq!(editor.input_mode(), InputMode::Prompt);
        assert_eq!(editor.text(), "prefix !literal");
    }

    #[test]
    fn input_handles_split_bracketed_paste_and_marker_expansion_tail() {
        let mut editor = CustomEditor::new();
        editor.handle_input_event("\u{1b}[200~multi");
        assert_eq!(editor.text(), "");
        editor.handle_input_event(" line\u{1b}[201~");
        assert_eq!(editor.text(), "multi line");

        let content = (0..11)
            .map(|index| format!("l{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        editor.set_text("");
        editor.insert_paste(&content);
        editor.set_cursor(0, 3);
        editor.handle_input_event("\u{1b}[200~ignored");
        assert_eq!(editor.text(), content);
        editor.handle_input_event("more chunks");
        editor.handle_input_event("tail\u{1b}[201~");
        assert_eq!(editor.text(), content);
    }

    #[test]
    fn input_submits_expanded_text_and_clears_buffer() {
        let mut editor = CustomEditor::new();
        let content = "x".repeat(1001);
        editor.insert_paste(&content);
        let outcome = editor.handle_input_event("\r");
        assert!(outcome.actions.contains(&EditorAction::NonEscapeInput));
        assert!(outcome.actions.contains(&EditorAction::Submit(content)));
        assert_eq!(editor.text(), "");
        assert_eq!(editor.expanded_text(), "");
    }

    #[test]
    fn input_escape_and_tab_prioritize_autocomplete_activity() {
        let mut editor = CustomEditor::new();
        editor.set_autocomplete_activity(true, false);
        let escape = editor.handle_input_event("\u{1b}");
        assert_eq!(escape.actions, [EditorAction::AutocompleteCancelled]);
        assert!(!editor.is_showing_autocomplete());

        let app_escape = editor.handle_input_event("\u{1b}");
        assert_eq!(app_escape.actions, [EditorAction::Escape]);
        assert_eq!(
            editor.handle_input_event("\t").actions,
            [EditorAction::NonEscapeInput]
        );
        editor.set_autocomplete_activity(true, false);
        assert_eq!(
            editor.handle_input_event("\t").actions,
            [EditorAction::NonEscapeInput]
        );
    }

    #[tokio::test]
    async fn async_autocomplete_request_renders_moves_and_applies_selection() {
        let mut editor = CustomEditor::new();
        editor.set_autocomplete_provider(test_autocomplete_provider());
        editor.set_text("/");
        let response = editor
            .begin_autocomplete_request(false, false)
            .expect("provider")
            .run()
            .await;
        assert!(editor.finish_autocomplete_request(response));
        assert!(editor.is_showing_autocomplete());

        let rendered = editor.render_editor(60);
        assert!(rendered.iter().any(|line| strip_sgr(line).contains("help")));
        assert!(rendered.iter().any(|line| strip_sgr(line).contains("goal")));
        assert!(rendered.iter().all(|line| visible_width(line) == 60));

        let down = editor.handle_input_event("\u{1b}[B");
        assert_eq!(down.actions, [EditorAction::NonEscapeInput]);
        let tab = editor.handle_input_event("\t");
        assert_eq!(tab.actions, [EditorAction::NonEscapeInput]);
        assert_eq!(editor.text(), "/goal ");
        assert!(!editor.is_showing_autocomplete());
    }

    #[tokio::test]
    async fn enter_accepts_slash_completion_and_submits_it() {
        let mut editor = CustomEditor::new();
        editor.set_autocomplete_provider(test_autocomplete_provider());
        editor.set_text("/h");
        let response = editor
            .begin_autocomplete_request(false, false)
            .expect("provider")
            .run()
            .await;
        assert!(editor.finish_autocomplete_request(response));

        let enter = editor.handle_input_event("\r");
        assert_eq!(
            enter.actions,
            [
                EditorAction::NonEscapeInput,
                EditorAction::Submit("/help".to_owned())
            ]
        );
        assert_eq!(editor.text(), "");
    }

    #[tokio::test]
    async fn stale_and_cancelled_autocomplete_results_do_not_replace_ui() {
        let mut editor = CustomEditor::new();
        editor.set_autocomplete_provider(test_autocomplete_provider());
        editor.set_text("/");
        let request = editor
            .begin_autocomplete_request(false, false)
            .expect("provider");
        editor.set_text("/changed");
        let response = request.run().await;
        assert!(!editor.finish_autocomplete_request(response));
        assert!(!editor.autocomplete_request_pending);
        assert!(!editor.is_showing_autocomplete());

        editor.set_text("/");
        let cancelled = editor
            .begin_autocomplete_request(false, false)
            .expect("provider");
        editor.cancel_autocomplete();
        assert!(!editor.finish_autocomplete_request(cancelled.run().await));
        assert!(!editor.is_showing_autocomplete());
    }

    #[test]
    fn open_autocomplete_refreshes_with_its_existing_request_mode() {
        let mut editor = CustomEditor::new();
        editor.set_autocomplete_provider(test_autocomplete_provider());
        editor.set_text("path");
        editor.apply_autocomplete_suggestions(
            AutocompleteSuggestions {
                prefix: "path".to_owned(),
                items: vec![AutocompleteItem::new("path-one", "path-one")],
            },
            true,
        );

        let typed = editor.handle_input_event("x");
        assert!(
            typed
                .actions
                .contains(&EditorAction::RequestAutocomplete { force: true })
        );
        let moved = editor.handle_input_event("\u{1b}[D");
        assert!(
            moved
                .actions
                .contains(&EditorAction::RequestAutocomplete { force: true })
        );
    }

    #[test]
    fn explicit_forced_single_result_applies_without_opening_a_list() {
        let mut editor = CustomEditor::new();
        editor.set_autocomplete_provider(test_autocomplete_provider());
        editor.set_text("fi");
        let request = editor
            .begin_autocomplete_request(true, true)
            .expect("provider");
        let response = AutocompleteResponse {
            request_id: request.request_id,
            snapshot_lines: request.snapshot_lines,
            snapshot_cursor_line: request.snapshot_cursor_line,
            snapshot_cursor_col: request.snapshot_cursor_col,
            force: true,
            explicit_tab: true,
            cancelled: request.cancelled,
            suggestions: Some(AutocompleteSuggestions {
                prefix: "fi".to_owned(),
                items: vec![AutocompleteItem::new("file.txt", "file.txt")],
            }),
        };
        assert!(editor.finish_autocomplete_request(response));
        assert_eq!(editor.text(), "file.txt");
        assert!(!editor.is_showing_autocomplete());
        assert!(editor.undo());
        assert_eq!(editor.text(), "fi");
    }

    #[test]
    fn input_requests_mode_specific_autocomplete() {
        let mut editor = CustomEditor::new();
        let slash = editor.handle_input_event("/");
        assert!(
            slash
                .actions
                .contains(&EditorAction::RequestAutocomplete { force: false })
        );
        editor.set_text("");
        editor.set_input_mode(InputMode::Bash);
        let slash = editor.handle_input_event("/");
        assert!(
            slash
                .actions
                .contains(&EditorAction::RequestAutocomplete { force: true })
        );

        editor.set_text("");
        editor.set_input_mode(InputMode::Prompt);
        let mention = editor.handle_input_event("@");
        assert!(
            mention
                .actions
                .contains(&EditorAction::RequestAutocomplete { force: false })
        );
    }

    #[test]
    fn empty_arrow_actions_allow_explicit_history_fallback() {
        let mut editor = CustomEditor::new();
        editor.add_to_history("previous");
        let up = editor.handle_input_event("\u{1b}[A");
        assert!(
            up.actions
                .contains(&EditorAction::UpArrowEmptyWithHistoryFallback)
        );
        assert_eq!(editor.text(), "");
        assert!(editor.apply_up_arrow_history_fallback());
        assert_eq!(editor.text(), "previous");
    }

    #[test]
    fn renders_prompt_cursor_and_hardware_marker_inside_full_border() {
        let mut editor = CustomEditor::new();
        editor.set_focused(true);
        let output = editor.render_editor(20);

        assert_eq!(output.len(), 3);
        assert!(strip_sgr(&output[0]).starts_with('╭'));
        assert!(strip_sgr(&output[0]).ends_with('╮'));
        assert!(strip_sgr(&output[1]).starts_with("│ > "));
        assert!(output[1].contains(CURSOR_MARKER));
        assert!(output[1].contains(CURSOR_BLOCK));
        assert!(strip_sgr(&output[2]).starts_with('╰'));
        assert!(strip_sgr(&output[2]).ends_with('╯'));
        assert!(output.iter().all(|line| visible_width(line) == 20));
    }

    #[test]
    fn renders_bash_badge_prompt_and_connected_border_states() {
        let mut editor = CustomEditor::new();
        editor.set_input_mode(InputMode::Bash);
        editor.set_connected_above(true);
        let output = editor.render_editor(32);
        assert!(strip_sgr(&output[0]).starts_with("├ ! shell mode "));
        assert!(strip_sgr(&output[0]).ends_with('┤'));
        assert!(strip_sgr(&output[1]).starts_with("│ ! "));

        editor.set_border_highlighted(true);
        let highlighted = editor.render_editor(32);
        assert!(strip_sgr(&highlighted[0]).starts_with("╭ ! shell mode "));
        assert!(highlighted[0].contains("38;2"));
    }

    #[test]
    fn renders_slash_highlight_and_argument_hint_only_at_command_end() {
        let mut editor = CustomEditor::new();
        editor.set_argument_hints(HashMap::from([(
            "goal".to_owned(),
            "[status|cancel]".to_owned(),
        )]));
        editor.set_text("/goal");
        let output = editor.render_editor(36);
        assert!(output[1].contains("38;2"));
        assert!(strip_sgr(&output[1]).contains("/goal  [status|cancel]"));

        editor.set_cursor(0, 2);
        let without_hint = editor.render_editor(36);
        assert!(!strip_sgr(&without_hint[1]).contains("status|cancel"));

        editor.set_input_mode(InputMode::Bash);
        let bash = editor.render_editor(36);
        assert!(!strip_sgr(&bash[1]).contains("status|cancel"));
    }

    #[test]
    fn wraps_cjk_and_scrolls_to_keep_cursor_visual_row_visible() {
        let mut editor = CustomEditor::new();
        editor.set_terminal_rows(10);
        editor.set_text("甲乙丙丁\none\ntwo\nthree\nfour\nfive\nsix");
        let output = editor.render_editor(12);

        assert_eq!(output.len(), 7);
        assert!(strip_sgr(&output[0]).contains('↑'));
        assert!(output.iter().all(|line| visible_width(line) == 12));
        assert!(output.iter().any(|line| strip_sgr(line).contains("six")));

        editor.set_cursor(0, 0);
        let top = editor.render_editor(12);
        assert!(!strip_sgr(&top[0]).contains('↑'));
        assert!(strip_sgr(top.last().expect("bottom border")).contains('↓'));
    }

    #[test]
    fn component_input_queues_host_actions_for_controller_dispatch() {
        let mut editor = CustomEditor::new();
        Component::handle_input(&mut editor, "\u{3}");
        assert_eq!(
            editor.take_actions(),
            [EditorAction::NonEscapeInput, EditorAction::CtrlC]
        );
        assert!(editor.take_actions().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_alt_v_requests_image_paste() {
        let mut editor = CustomEditor::new();
        let outcome = editor.handle_input_event("\u{1b}v");
        assert!(outcome.actions.contains(&EditorAction::PasteImage));
    }
}
