use std::any::Any;

use unicode_segmentation::UnicodeSegmentation;

use crate::tui::{
    components::{Component, core::CURSOR_MARKER, render::visible_width},
    keys::{EditorKey, decode_kitty_printable, is_key_release, matches_editor_key},
};

const BRACKETED_PASTE_START: &str = "\u{1b}[200~";
const BRACKETED_PASTE_END: &str = "\u{1b}[201~";
const PROMPT: &str = "> ";

type SubmitCallback = dyn FnMut(String) + Send;
type EscapeCallback = dyn FnMut() + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputState {
    value: String,
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    Submit(String),
    Escape,
}

#[derive(Debug, Default)]
struct KillRing {
    entries: Vec<String>,
}

impl KillRing {
    fn push(&mut self, text: String, prepend: bool, accumulate: bool) {
        if text.is_empty() {
            return;
        }
        if accumulate && !self.entries.is_empty() {
            let previous = self.entries.pop().unwrap_or_default();
            self.entries.push(if prepend {
                format!("{text}{previous}")
            } else {
                format!("{previous}{text}")
            });
        } else {
            self.entries.push(text);
        }
    }

    fn peek(&self) -> Option<&str> {
        self.entries.last().map(String::as_str)
    }

    fn rotate(&mut self) {
        if self.entries.len() > 1
            && let Some(last) = self.entries.pop()
        {
            self.entries.insert(0, last);
        }
    }
}

/// Single-line text input with horizontal scrolling.
///
/// Original: `packages/pi-tui/src/components/input.ts`, `Input`.
pub struct Input {
    value: String,
    cursor: usize,
    pub focused: bool,
    paste_buffer: String,
    is_in_paste: bool,
    kill_ring: KillRing,
    last_action: Option<LastAction>,
    undo_stack: Vec<InputState>,
    on_submit: Option<Box<SubmitCallback>>,
    on_escape: Option<Box<EscapeCallback>>,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    pub fn new() -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            focused: false,
            paste_buffer: String::new(),
            is_in_paste: false,
            kill_ring: KillRing::default(),
            last_action: None,
            undo_stack: Vec::new(),
            on_submit: None,
            on_escape: None,
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.cursor.min(self.value.len());
        while !self.value.is_char_boundary(self.cursor) {
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn set_on_submit<F>(&mut self, callback: F)
    where
        F: FnMut(String) + Send + 'static,
    {
        self.on_submit = Some(Box::new(callback));
    }

    pub fn set_on_escape<F>(&mut self, callback: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.on_escape = Some(Box::new(callback));
    }

    /// Original: `Input.handleInput()`.
    pub fn handle_input_event(&mut self, data: &str) -> Option<InputAction> {
        if is_key_release(data) {
            return None;
        }

        if let Some(start) = data.find(BRACKETED_PASTE_START) {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            let mut without_start = data.to_owned();
            without_start.replace_range(start..start + BRACKETED_PASTE_START.len(), "");
            return self.buffer_paste(&without_start);
        }
        if self.is_in_paste {
            return self.buffer_paste(data);
        }

        if matches_editor_key(data, EditorKey::Escape) {
            if let Some(callback) = &mut self.on_escape {
                callback();
            }
            return Some(InputAction::Escape);
        }
        if matches_editor_key(data, EditorKey::CtrlMinus) {
            self.undo();
            return None;
        }
        if matches_editor_key(data, EditorKey::Enter) {
            let value = self.value.clone();
            if let Some(callback) = &mut self.on_submit {
                callback(value.clone());
            }
            return Some(InputAction::Submit(value));
        }
        if matches_editor_key(data, EditorKey::Backspace) {
            self.handle_backspace();
            return None;
        }
        if matches_editor_key(data, EditorKey::Delete)
            || matches_editor_key(data, EditorKey::Ctrl('d'))
        {
            self.handle_forward_delete();
            return None;
        }
        if matches_editor_key(data, EditorKey::Ctrl('w'))
            || matches_editor_key(data, EditorKey::AltBackspace)
        {
            self.delete_word_backwards();
            return None;
        }
        if matches_editor_key(data, EditorKey::Alt('d'))
            || matches_editor_key(data, EditorKey::AltDelete)
        {
            self.delete_word_forward();
            return None;
        }
        if matches_editor_key(data, EditorKey::Ctrl('u')) {
            self.delete_to_line_start();
            return None;
        }
        if matches_editor_key(data, EditorKey::Ctrl('k')) {
            self.delete_to_line_end();
            return None;
        }
        if matches_editor_key(data, EditorKey::Ctrl('y')) {
            self.yank();
            return None;
        }
        if matches_editor_key(data, EditorKey::Alt('y')) {
            self.yank_pop();
            return None;
        }
        if matches_editor_key(data, EditorKey::Left)
            || matches_editor_key(data, EditorKey::Ctrl('b'))
        {
            self.last_action = None;
            self.move_grapheme_left();
            return None;
        }
        if matches_editor_key(data, EditorKey::Right)
            || matches_editor_key(data, EditorKey::Ctrl('f'))
        {
            self.last_action = None;
            self.move_grapheme_right();
            return None;
        }
        if matches_editor_key(data, EditorKey::Home)
            || matches_editor_key(data, EditorKey::Ctrl('a'))
        {
            self.last_action = None;
            self.cursor = 0;
            return None;
        }
        if matches_editor_key(data, EditorKey::End)
            || matches_editor_key(data, EditorKey::Ctrl('e'))
        {
            self.last_action = None;
            self.cursor = self.value.len();
            return None;
        }
        if matches_editor_key(data, EditorKey::WordLeft)
            || matches_editor_key(data, EditorKey::Alt('b'))
        {
            self.move_word_backwards();
            return None;
        }
        if matches_editor_key(data, EditorKey::WordRight)
            || matches_editor_key(data, EditorKey::Alt('f'))
        {
            self.move_word_forwards();
            return None;
        }
        if let Some(printable) = decode_kitty_printable(data) {
            self.insert_character(&printable);
            return None;
        }
        if !data.chars().any(is_rejected_control) {
            self.insert_character(data);
        }
        None
    }

    fn buffer_paste(&mut self, data: &str) -> Option<InputAction> {
        self.paste_buffer.push_str(data);
        let end = self.paste_buffer.find(BRACKETED_PASTE_END)?;
        let pasted = self.paste_buffer[..end].to_owned();
        let remaining = self.paste_buffer[end + BRACKETED_PASTE_END.len()..].to_owned();
        self.handle_paste(&pasted);
        self.is_in_paste = false;
        self.paste_buffer.clear();
        if remaining.is_empty() {
            None
        } else {
            self.handle_input_event(&remaining)
        }
    }

    fn insert_character(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if text.chars().all(char::is_whitespace) || self.last_action != Some(LastAction::TypeWord) {
            self.push_undo();
        }
        self.last_action = Some(LastAction::TypeWord);
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn handle_backspace(&mut self) {
        self.last_action = None;
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let start = previous_grapheme_boundary(&self.value, self.cursor);
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn handle_forward_delete(&mut self) {
        self.last_action = None;
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let end = next_grapheme_boundary(&self.value, self.cursor);
        self.value.replace_range(self.cursor..end, "");
    }

    fn delete_to_line_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let deleted = self.value[..self.cursor].to_owned();
        let accumulate = self.last_action == Some(LastAction::Kill);
        self.kill_ring.push(deleted, true, accumulate);
        self.last_action = Some(LastAction::Kill);
        self.value.replace_range(..self.cursor, "");
        self.cursor = 0;
    }

    fn delete_to_line_end(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let deleted = self.value[self.cursor..].to_owned();
        let accumulate = self.last_action == Some(LastAction::Kill);
        self.kill_ring.push(deleted, false, accumulate);
        self.last_action = Some(LastAction::Kill);
        self.value.truncate(self.cursor);
    }

    fn delete_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let accumulate = self.last_action == Some(LastAction::Kill);
        self.push_undo();
        let old_cursor = self.cursor;
        let delete_from = find_word_backward(&self.value, self.cursor);
        let deleted = self.value[delete_from..old_cursor].to_owned();
        self.kill_ring.push(deleted, true, accumulate);
        self.last_action = Some(LastAction::Kill);
        self.value.replace_range(delete_from..old_cursor, "");
        self.cursor = delete_from;
    }

    fn delete_word_forward(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        let accumulate = self.last_action == Some(LastAction::Kill);
        self.push_undo();
        let delete_to = find_word_forward(&self.value, self.cursor);
        let deleted = self.value[self.cursor..delete_to].to_owned();
        self.kill_ring.push(deleted, false, accumulate);
        self.last_action = Some(LastAction::Kill);
        self.value.replace_range(self.cursor..delete_to, "");
    }

    fn yank(&mut self) {
        let Some(text) = self.kill_ring.peek().map(str::to_owned) else {
            return;
        };
        self.push_undo();
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.last_action = Some(LastAction::Yank);
    }

    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) || self.kill_ring.entries.len() <= 1 {
            return;
        }
        self.push_undo();
        let previous = self.kill_ring.peek().unwrap_or_default().to_owned();
        let start = self.cursor.saturating_sub(previous.len());
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.kill_ring.rotate();
        let replacement = self.kill_ring.peek().unwrap_or_default().to_owned();
        self.value.insert_str(self.cursor, &replacement);
        self.cursor += replacement.len();
        self.last_action = Some(LastAction::Yank);
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(InputState {
            value: self.value.clone(),
            cursor: self.cursor,
        });
    }

    fn undo(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.value = snapshot.value;
        self.cursor = snapshot.cursor;
        self.last_action = None;
    }

    fn move_grapheme_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = previous_grapheme_boundary(&self.value, self.cursor);
        }
    }

    fn move_grapheme_right(&mut self) {
        if self.cursor < self.value.len() {
            self.cursor = next_grapheme_boundary(&self.value, self.cursor);
        }
    }

    fn move_word_backwards(&mut self) {
        self.last_action = None;
        self.cursor = find_word_backward(&self.value, self.cursor);
    }

    fn move_word_forwards(&mut self) {
        self.last_action = None;
        self.cursor = find_word_forward(&self.value, self.cursor);
    }

    fn handle_paste(&mut self, text: &str) {
        self.last_action = None;
        self.push_undo();
        let clean = text
            .replace("\r\n", "")
            .replace(['\r', '\n'], "")
            .replace('\t', "    ");
        self.value.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    /// Original: `Input.render()`.
    pub fn render_line(&self, width: usize) -> String {
        let available_width = width.saturating_sub(PROMPT.len());
        if available_width == 0 {
            return PROMPT.to_owned();
        }

        let total_width = visible_width(&self.value);
        let (visible_text, cursor_display) = if total_width < available_width {
            (self.value.clone(), self.cursor)
        } else {
            let scroll_width = if self.cursor == self.value.len() {
                available_width.saturating_sub(1)
            } else {
                available_width
            };
            if scroll_width == 0 {
                (String::new(), 0)
            } else {
                let cursor_col = visible_width(&self.value[..self.cursor]);
                let half_width = scroll_width / 2;
                let start_col = if cursor_col < half_width {
                    0
                } else if cursor_col > total_width.saturating_sub(half_width) {
                    total_width.saturating_sub(scroll_width)
                } else {
                    cursor_col.saturating_sub(half_width)
                };
                let visible = slice_by_column(&self.value, start_col, scroll_width);
                let before =
                    slice_by_column(&self.value, start_col, cursor_col.saturating_sub(start_col));
                (visible, before.len())
            }
        };

        let at_cursor =
            UnicodeSegmentation::graphemes(&visible_text[cursor_display..], true).next();
        let before = &visible_text[..cursor_display];
        let after = at_cursor.map_or("", |grapheme| {
            &visible_text[cursor_display + grapheme.len()..]
        });
        let at_cursor = at_cursor.unwrap_or(" ");
        let marker = if self.focused { CURSOR_MARKER } else { "" };
        let displayed = format!("{before}{marker}\u{1b}[7m{at_cursor}\u{1b}[27m{after}");
        let padding = " ".repeat(available_width.saturating_sub(visible_width(&displayed)));
        format!("{PROMPT}{displayed}{padding}")
    }
}

impl Component for Input {
    fn render(&mut self, width: usize) -> Vec<String> {
        vec![self.render_line(width)]
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }

    fn wants_key_release(&self) -> bool {
        true
    }

    fn invalidate(&mut self) {}

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn is_rejected_control(character: char) -> bool {
    let code = u32::from(character);
    code < 32 || code == 0x7f || (0x80..=0x9f).contains(&code)
}

fn previous_grapheme_boundary(text: &str, cursor: usize) -> usize {
    UnicodeSegmentation::grapheme_indices(&text[..cursor], true)
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_grapheme_boundary(text: &str, cursor: usize) -> usize {
    UnicodeSegmentation::graphemes(&text[cursor..], true)
        .next()
        .map_or(text.len(), |grapheme| cursor + grapheme.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Whitespace,
    Word,
    Punctuation,
}

fn word_class(grapheme: &str) -> WordClass {
    if grapheme.chars().all(char::is_whitespace) {
        WordClass::Whitespace
    } else if grapheme.chars().any(char::is_alphanumeric) || grapheme == "_" {
        WordClass::Word
    } else {
        WordClass::Punctuation
    }
}

/// Original: `packages/pi-tui/src/word-navigation.ts`, `findWordBackward()`.
fn find_word_backward(text: &str, cursor: usize) -> usize {
    let graphemes: Vec<(usize, &str)> =
        UnicodeSegmentation::grapheme_indices(&text[..cursor], true).collect();
    let mut index = graphemes.len();
    while index > 0 && word_class(graphemes[index - 1].1) == WordClass::Whitespace {
        index -= 1;
    }
    if index == 0 {
        return 0;
    }
    let class = word_class(graphemes[index - 1].1);
    while index > 0 && word_class(graphemes[index - 1].1) == class {
        index -= 1;
    }
    graphemes.get(index).map_or(0, |(offset, _)| *offset)
}

/// Original: `packages/pi-tui/src/word-navigation.ts`, `findWordForward()`.
fn find_word_forward(text: &str, cursor: usize) -> usize {
    let graphemes: Vec<(usize, &str)> =
        UnicodeSegmentation::grapheme_indices(&text[cursor..], true).collect();
    let mut index = 0;
    while index < graphemes.len() && word_class(graphemes[index].1) == WordClass::Whitespace {
        index += 1;
    }
    if index == graphemes.len() {
        return text.len();
    }
    let class = word_class(graphemes[index].1);
    while index < graphemes.len() && word_class(graphemes[index].1) == class {
        index += 1;
    }
    graphemes
        .get(index)
        .map_or(text.len(), |(offset, _)| cursor + offset)
}

fn slice_by_column(text: &str, start_column: usize, length: usize) -> String {
    if length == 0 {
        return String::new();
    }
    let end_column = start_column + length;
    let mut column = 0;
    let mut output = String::new();
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let width = visible_width(grapheme);
        if column >= start_column && column < end_column && column + width <= end_column {
            output.push_str(grapheme);
        }
        column += width;
        if column >= end_column {
            break;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn inserts_moves_and_deletes_complete_graphemes() {
        let mut input = Input::new();
        input.handle_input_event("A\u{301}猫");
        input.handle_input_event("\u{1b}[D");
        input.handle_input_event("\u{7f}");
        assert_eq!(input.value(), "猫");
        input.handle_input_event("\u{1b}[3~");
        assert_eq!(input.value(), "");
    }

    #[test]
    fn submits_escapes_and_ignores_key_releases() {
        let submissions = Arc::new(Mutex::new(Vec::new()));
        let escaped = Arc::new(Mutex::new(0));
        let mut input = Input::new();
        let recorded = Arc::clone(&submissions);
        input.set_on_submit(move |value| recorded.lock().expect("submissions").push(value));
        let recorded = Arc::clone(&escaped);
        input.set_on_escape(move || *recorded.lock().expect("escape count") += 1);
        input.handle_input_event("hello");
        assert_eq!(
            input.handle_input_event("\r"),
            Some(InputAction::Submit("hello".into()))
        );
        assert_eq!(
            input.handle_input_event("\u{1b}"),
            Some(InputAction::Escape)
        );
        input.handle_input_event("\u{1b}[120;1:3u");
        assert_eq!(input.value(), "hello");
        assert_eq!(*submissions.lock().expect("submissions"), ["hello"]);
        assert_eq!(*escaped.lock().expect("escape count"), 1);
    }

    #[test]
    fn buffers_and_normalizes_bracketed_paste_as_one_undo_unit() {
        let mut input = Input::new();
        input.handle_input_event("a\u{1b}[200~one\r\n");
        input.handle_input_event("two\tthree\u{1b}[201~z");
        assert_eq!(input.value(), "aonetwo    threez");
        input.handle_input_event("\u{1f}");
        assert_eq!(input.value(), "aonetwo    three");
        input.handle_input_event("\u{1f}");
        assert_eq!(input.value(), "");
    }

    #[test]
    fn coalesces_word_typing_but_not_whitespace_for_undo() {
        let mut input = Input::new();
        for character in ["a", "b", "c", " ", "d"] {
            input.handle_input_event(character);
        }
        input.handle_input_event("\u{1f}");
        assert_eq!(input.value(), "abc");
        input.handle_input_event("\u{1f}");
        assert_eq!(input.value(), "");
    }

    #[test]
    fn navigates_and_kills_word_and_line_ranges() {
        let mut input = Input::new();
        input.handle_input_event("foo.bar baz");
        input.handle_input_event("\u{1b}b");
        input.handle_input_event("\u{17}");
        assert_eq!(input.value(), "foo.baz");
        input.handle_input_event("\u{19}");
        assert_eq!(input.value(), "foo.bar baz");
        input.handle_input_event("\u{1}");
        input.handle_input_event("\u{1b}d");
        assert_eq!(input.value(), ".bar baz");
        input.handle_input_event("\u{5}");
        input.handle_input_event("\u{15}");
        assert_eq!(input.value(), "");
    }

    #[test]
    fn accumulates_consecutive_kills_and_supports_yank_pop() {
        let mut input = Input::new();
        input.handle_input_event("one two three");
        input.handle_input_event("\u{17}");
        input.handle_input_event("\u{17}");
        assert_eq!(input.value(), "one ");
        input.handle_input_event("\u{19}");
        assert_eq!(input.value(), "one two three");

        input.handle_input_event("\u{1}");
        input.handle_input_event("\u{1b}d");
        input.handle_input_event("\u{5}");
        input.handle_input_event("\u{19}");
        input.handle_input_event("\u{1b}y");
        assert_eq!(input.value(), " two threetwo three");
    }

    #[test]
    fn decodes_kitty_printable_and_modified_navigation() {
        let mut input = Input::new();
        input.handle_input_event("\u{1b}[97u");
        input.handle_input_event(" bc");
        input.handle_input_event("\u{1b}[57417;5u");
        input.handle_input_event("X");
        assert_eq!(input.value(), "a Xbc");
    }

    #[test]
    fn renders_cursor_marker_padding_and_horizontal_window() {
        let mut input = Input::new();
        input.focused = true;
        input.handle_input_event("ab猫defgh");
        let line = input.render_line(8);
        assert!(line.starts_with("> "));
        assert!(line.contains(CURSOR_MARKER));
        assert_eq!(visible_width(&line), 8);
        assert!(!line.contains("ab"));

        assert_eq!(Input::new().render_line(1), "> ");
    }

    #[test]
    fn word_navigation_preserves_ascii_punctuation_boundaries() {
        let text = "path/to/file";
        assert_eq!(find_word_backward(text, text.len()), 8);
        assert_eq!(find_word_backward(text, 8), 7);
        assert_eq!(find_word_backward(text, 7), 5);
        assert_eq!(find_word_forward(text, 0), 4);
        assert_eq!(find_word_forward(text, 4), 5);
        assert_eq!(find_word_forward(text, 5), 7);
        assert_eq!(find_word_forward("  hello  ", 0), 7);
    }
}
