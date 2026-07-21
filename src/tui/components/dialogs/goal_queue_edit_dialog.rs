use std::{any::Any, sync::LazyLock};

use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

use crate::tui::{
    commands::goal::MAX_GOAL_OBJECTIVE_LENGTH,
    components::{
        Component, ComponentRole,
        core::CURSOR_MARKER,
        render::{truncate_to_width, visible_width},
    },
    goal_queue_store::UpcomingGoal,
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
    utils::printable_key::printable_char,
};

const MAX_EDIT_INPUT_LINES: usize = 8;
const BRACKET_PASTE_START: &str = "\u{1b}[200~";
const BRACKET_PASTE_END: &str = "\u{1b}[201~";
const SHIFT_ENTER_LEGACY: &str = "\u{1b}\r";
const SHIFT_ENTER_CSI: &str = "\u{1b}[13;2~";

static ANSI_CSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("ANSI CSI regex must compile"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalQueueEditResult {
    Save { goal_id: String, objective: String },
    Cancel { goal_id: String },
}

type DoneCallback = dyn FnMut(GoalQueueEditResult) + Send;

pub struct GoalQueueEditDialogOptions {
    pub goal: UpcomingGoal,
    on_done: Box<DoneCallback>,
}

impl GoalQueueEditDialogOptions {
    pub fn new<D>(goal: UpcomingGoal, on_done: D) -> Self
    where
        D: FnMut(GoalQueueEditResult) + Send + 'static,
    {
        Self {
            goal,
            on_done: Box::new(on_done),
        }
    }
}

/// Multiline editor for one queued goal.
///
/// Original: `goal-queue-manager.ts`, `GoalQueueEditDialogComponent`.
pub struct GoalQueueEditDialogComponent {
    pub focused: bool,
    goal_id: String,
    input: MultilineGoalInput,
    done: bool,
    error: Option<String>,
    on_done: Box<DoneCallback>,
}

impl GoalQueueEditDialogComponent {
    pub fn new(options: GoalQueueEditDialogOptions) -> Self {
        let mut input = MultilineGoalInput::default();
        input.set_value(&options.goal.objective);
        Self {
            focused: false,
            goal_id: options.goal.id,
            input,
            done: false,
            error: None,
            on_done: options.on_done,
        }
    }

    pub fn value(&self) -> String {
        self.input.value()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if self.done {
            return;
        }
        if matches_editor_key(data, EditorKey::Escape)
            || matches_editor_key(data, EditorKey::Ctrl('c'))
            || matches_editor_key(data, EditorKey::Ctrl('d'))
        {
            self.done = true;
            (self.on_done)(GoalQueueEditResult::Cancel {
                goal_id: self.goal_id.clone(),
            });
            return;
        }
        self.error = None;
        if self.input.handle_input(data) {
            self.submit();
        }
    }

    fn submit(&mut self) {
        let value = self.input.value();
        let objective = value.trim();
        if objective.is_empty() {
            self.error = Some("Goal objective cannot be empty.".to_owned());
            return;
        }
        if objective.encode_utf16().count() > MAX_GOAL_OBJECTIVE_LENGTH {
            self.error = Some(format!(
                "Goal objective cannot exceed {MAX_GOAL_OBJECTIVE_LENGTH} characters."
            ));
            return;
        }
        (self.on_done)(GoalQueueEditResult::Save {
            goal_id: self.goal_id.clone(),
            objective: objective.to_owned(),
        });
    }

    fn render_dialog(&mut self, width: usize) -> Vec<String> {
        self.input.focused = self.focused && !self.done;
        if width == 0 {
            return vec![String::new()];
        }
        let inner_width = width.saturating_sub(4).max(1);
        let title = truncate_to_width(
            &current_theme().bold_fg(ColorToken::TextStrong, "Edit upcoming goal"),
            inner_width,
            "…",
            false,
        );
        let subtitle = truncate_to_width(
            &current_theme().fg(
                if self.error.is_some() {
                    ColorToken::Warning
                } else {
                    ColorToken::TextDim
                },
                self.error
                    .as_deref()
                    .unwrap_or("Update the queued objective."),
            ),
            inner_width,
            "…",
            false,
        );
        let footer = truncate_to_width(
            &current_theme().fg(
                ColorToken::TextDim,
                "Enter submit · Shift-Enter/Ctrl-J newline · Esc cancel",
            ),
            inner_width,
            "…",
            false,
        );
        let mut content = vec![title, String::new(), subtitle, String::new()];
        content.extend(self.input.render(inner_width));
        content.extend([String::new(), footer]);
        if width < 4 {
            let mut lines = vec![String::new()];
            lines.extend(
                content
                    .into_iter()
                    .map(|line| truncate_to_width(&line, width, "…", false)),
            );
            return lines;
        }
        let border = |text: &str| current_theme().fg(ColorToken::Primary, text);
        let mut lines = vec![
            String::new(),
            border(&format!("╭{}╮", "─".repeat(width - 2))),
            format!("{}{}{}", border("│"), " ".repeat(width - 2), border("│")),
        ];
        for line in content {
            let padding = " ".repeat(inner_width.saturating_sub(visible_width(&line)));
            lines.push(format!("{}  {line}{padding}{}", border("│"), border("│")));
        }
        lines.extend([
            format!("{}{}{}", border("│"), " ".repeat(width - 2), border("│")),
            border(&format!("╰{}╯", "─".repeat(width - 2))),
            String::new(),
        ]);
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }
}

impl Component for GoalQueueEditDialogComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_dialog(width)
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

#[derive(Default)]
struct MultilineGoalInput {
    focused: bool,
    value: Vec<u16>,
    cursor: usize,
    paste_buffer: Option<String>,
}

impl MultilineGoalInput {
    fn value(&self) -> String {
        String::from_utf16_lossy(&self.value)
    }

    fn set_value(&mut self, value: &str) {
        self.value = normalize_newlines(value).encode_utf16().collect();
        self.cursor = self.value.len();
    }

    /// Returns true when Enter requests submission.
    fn handle_input(&mut self, data: &str) -> bool {
        if self.handle_bracketed_paste(data) {
            return false;
        }
        if is_newline_input(data) {
            self.insert("\n");
        } else if matches_editor_key(data, EditorKey::Enter) {
            return true;
        } else if matches_editor_key(data, EditorKey::Backspace) {
            let start = previous_grapheme_start(&self.value, self.cursor);
            self.value.drain(start..self.cursor);
            self.cursor = start;
        } else if matches_editor_key(data, EditorKey::Delete) {
            let end = next_grapheme_end(&self.value, self.cursor);
            self.value.drain(self.cursor..end);
        } else if matches_editor_key(data, EditorKey::Left) {
            self.cursor = previous_grapheme_start(&self.value, self.cursor);
        } else if matches_editor_key(data, EditorKey::Right) {
            self.cursor = next_grapheme_end(&self.value, self.cursor);
        } else if matches_editor_key(data, EditorKey::Up) {
            self.move_vertical(-1);
        } else if matches_editor_key(data, EditorKey::Down) {
            self.move_vertical(1);
        } else if matches_editor_key(data, EditorKey::Home)
            || matches_editor_key(data, EditorKey::Ctrl('a'))
        {
            self.cursor = self.current_line_start();
        } else if matches_editor_key(data, EditorKey::End)
            || matches_editor_key(data, EditorKey::Ctrl('e'))
        {
            self.cursor = self.current_line_end();
        } else {
            let decoded = printable_char(data);
            if is_printable_text(&decoded) {
                self.insert(&decoded);
            }
        }
        false
    }

    fn insert(&mut self, text: &str) {
        let units = normalize_newlines(text).encode_utf16().collect::<Vec<_>>();
        let length = units.len();
        self.value.splice(self.cursor..self.cursor, units);
        self.cursor += length;
    }

    fn move_vertical(&mut self, delta: isize) {
        let starts = line_starts(&self.value);
        let (line, column) = cursor_location(&starts, self.cursor);
        let Some(target) = line
            .checked_add_signed(delta)
            .filter(|line| *line < starts.len())
        else {
            return;
        };
        let start = starts[target];
        self.cursor = (start + column).min(line_end_for_start(&self.value, &starts, target));
    }

    fn current_line_start(&self) -> usize {
        self.value[..self.cursor]
            .iter()
            .rposition(|unit| *unit == u16::from(b'\n'))
            .map_or(0, |index| index + 1)
    }

    fn current_line_end(&self) -> usize {
        self.value[self.cursor..]
            .iter()
            .position(|unit| *unit == u16::from(b'\n'))
            .map_or(self.value.len(), |offset| self.cursor + offset)
    }

    fn handle_bracketed_paste(&mut self, data: &str) -> bool {
        if self.paste_buffer.is_some() {
            self.append_paste_chunk(data);
            return true;
        }
        let Some(start) = data.find(BRACKET_PASTE_START) else {
            return false;
        };
        self.paste_buffer = Some(String::new());
        let before = &data[..start];
        if is_printable_text(before) {
            self.insert(before);
        }
        self.append_paste_chunk(&data[start + BRACKET_PASTE_START.len()..]);
        true
    }

    fn append_paste_chunk(&mut self, data: &str) {
        let Some(buffer) = &mut self.paste_buffer else {
            return;
        };
        buffer.push_str(data);
        let Some(end) = buffer.find(BRACKET_PASTE_END) else {
            return;
        };
        let pasted = buffer[..end].to_owned();
        let remaining = buffer[end + BRACKET_PASTE_END.len()..].to_owned();
        self.paste_buffer = None;
        self.insert(&sanitize_pasted_text(&pasted));
        if !remaining.is_empty() {
            self.handle_input(&remaining);
        }
    }

    fn render(&self, width: usize) -> Vec<String> {
        let width = width.max(4);
        let starts = line_starts(&self.value);
        let (cursor_line, cursor_column) = cursor_location(&starts, self.cursor);
        let (start, end) = visible_line_range(starts.len(), cursor_line);
        let mut lines = Vec::new();
        if start > 0 {
            lines.push(pad_input_line(&format!("  … {start} previous"), width));
        }
        for line_index in start..end {
            let line_end = line_end_for_start(&self.value, &starts, line_index);
            let line = &self.value[starts[line_index]..line_end];
            let prefix = if line_index == 0 { "> " } else { "  " };
            lines.push(if line_index == cursor_line {
                render_cursor_line(line, cursor_column, prefix, width, self.focused)
            } else {
                render_text_line(line, prefix, width)
            });
        }
        let remaining = starts.len() - end;
        if remaining > 0 {
            lines.push(pad_input_line(&format!("  … {remaining} more"), width));
        }
        lines
    }
}

fn is_newline_input(data: &str) -> bool {
    data == "\n"
        || data == SHIFT_ENTER_LEGACY
        || data == SHIFT_ENTER_CSI
        || matches_editor_key(data, EditorKey::Ctrl('j'))
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn sanitize_pasted_text(text: &str) -> String {
    ANSI_CSI
        .replace_all(&normalize_newlines(text), "")
        .chars()
        .filter(|character| {
            *character == '\n'
                || (!character.is_control() && !matches!(u32::from(*character), 0x7f..=0x9f))
        })
        .collect()
}

fn is_printable_text(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|character| {
            !character.is_control() && !matches!(u32::from(character), 0x7f..=0x9f)
        })
}

fn line_starts(value: &[u16]) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        value
            .iter()
            .enumerate()
            .filter_map(|(index, unit)| (*unit == u16::from(b'\n')).then_some(index + 1)),
    );
    starts
}

fn cursor_location(starts: &[usize], cursor: usize) -> (usize, usize) {
    let line = starts
        .partition_point(|start| *start <= cursor)
        .saturating_sub(1);
    (line, cursor - starts[line])
}

fn line_end_for_start(value: &[u16], starts: &[usize], line: usize) -> usize {
    starts.get(line + 1).map_or(value.len(), |next| next - 1)
}

fn grapheme_boundaries(value: &[u16]) -> Vec<usize> {
    let text = String::from_utf16_lossy(value);
    let mut boundaries = UnicodeSegmentation::grapheme_indices(text.as_str(), true)
        .map(|(byte, _)| text[..byte].encode_utf16().count())
        .collect::<Vec<_>>();
    boundaries.push(value.len());
    boundaries
}

fn previous_grapheme_start(value: &[u16], cursor: usize) -> usize {
    grapheme_boundaries(value)
        .into_iter()
        .take_while(|boundary| *boundary < cursor)
        .last()
        .unwrap_or_default()
}

fn next_grapheme_end(value: &[u16], cursor: usize) -> usize {
    grapheme_boundaries(value)
        .into_iter()
        .find(|boundary| *boundary > cursor)
        .unwrap_or(value.len())
}

fn visible_line_range(total: usize, cursor: usize) -> (usize, usize) {
    if total <= MAX_EDIT_INPUT_LINES {
        return (0, total);
    }
    let start = cursor
        .saturating_sub(MAX_EDIT_INPUT_LINES / 2)
        .min(total - MAX_EDIT_INPUT_LINES);
    (start, start + MAX_EDIT_INPUT_LINES)
}

fn render_text_line(line: &[u16], prefix: &str, width: usize) -> String {
    let text = String::from_utf16_lossy(line);
    let shown = truncate_to_width(
        &text,
        width.saturating_sub(visible_width(prefix)).max(1),
        "…",
        false,
    );
    pad_input_line(&format!("{prefix}{shown}"), width)
}

fn render_cursor_line(
    line: &[u16],
    column: usize,
    prefix: &str,
    width: usize,
    focused: bool,
) -> String {
    let column = column.min(line.len());
    let cursor_end = next_grapheme_end(line, column);
    let before = String::from_utf16_lossy(&line[..column]);
    let cursor = if cursor_end == column {
        " ".to_owned()
    } else {
        String::from_utf16_lossy(&line[column..cursor_end])
    };
    let after = String::from_utf16_lossy(&line[cursor_end..]);
    let text_width = width.saturating_sub(visible_width(prefix)).max(1);
    let cursor_width = visible_width(&cursor).max(1);
    let before = take_end_by_width(&before, text_width.saturating_sub(cursor_width));
    let after = take_start_by_width(
        &after,
        text_width.saturating_sub(visible_width(&before) + cursor_width),
    );
    let marker = if focused { CURSOR_MARKER } else { "" };
    pad_input_line(
        &format!("{prefix}{before}{marker}\u{1b}[7m{cursor}\u{1b}[27m{after}"),
        width,
    )
}

fn take_start_by_width(text: &str, width: usize) -> String {
    let mut used = 0;
    UnicodeSegmentation::graphemes(text, true)
        .take_while(|grapheme| {
            let next = used + visible_width(grapheme);
            if next > width {
                false
            } else {
                used = next;
                true
            }
        })
        .collect()
}

fn take_end_by_width(text: &str, width: usize) -> String {
    let mut used = 0;
    let mut pieces = UnicodeSegmentation::graphemes(text, true)
        .rev()
        .take_while(|grapheme| {
            let next = used + visible_width(grapheme);
            if next > width {
                false
            } else {
                used = next;
                true
            }
        })
        .collect::<Vec<_>>();
    pieces.reverse();
    pieces.concat()
}

fn pad_input_line(line: &str, width: usize) -> String {
    format!(
        "{line}{}",
        " ".repeat(width.saturating_sub(visible_width(line)))
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn goal(objective: &str) -> UpcomingGoal {
        UpcomingGoal {
            id: "goal-1".to_owned(),
            objective: objective.to_owned(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn edits_unicode_by_grapheme_and_inserts_newlines() {
        let mut dialog = GoalQueueEditDialogComponent::new(GoalQueueEditDialogOptions::new(
            goal("A👨‍👩‍👧‍👦B"),
            |_| {},
        ));
        dialog.handle_input_event("\u{1b}[D");
        dialog.handle_input_event("\u{7f}");
        dialog.handle_input_event(SHIFT_ENTER_CSI);
        assert_eq!(dialog.value(), "A\nB");
    }

    #[test]
    fn sanitizes_chunked_bracketed_paste_and_submits_trimmed_value() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::clone(&events);
        let mut dialog = GoalQueueEditDialogComponent::new(GoalQueueEditDialogOptions::new(
            goal(""),
            move |result| called.lock().expect("events").push(result),
        ));
        dialog.handle_input_event("\u{1b}[200~hello\u{1b}[31m");
        dialog.handle_input_event("world\u{1b}[0m\nnext\u{1b}[201~");
        dialog.handle_input_event("\r");
        assert_eq!(dialog.value(), "helloworld\nnext");
        assert_eq!(
            *events.lock().expect("events"),
            [GoalQueueEditResult::Save {
                goal_id: "goal-1".to_owned(),
                objective: "helloworld\nnext".to_owned()
            }]
        );
    }

    #[test]
    fn validates_empty_and_utf16_length_then_allows_cancellation_once() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::clone(&events);
        let mut dialog = GoalQueueEditDialogComponent::new(GoalQueueEditDialogOptions::new(
            goal("   "),
            move |result| called.lock().expect("events").push(result),
        ));
        dialog.handle_input_event("\r");
        assert_eq!(dialog.error(), Some("Goal objective cannot be empty."));
        dialog.input.set_value(&"😀".repeat(2_001));
        dialog.handle_input_event("\r");
        assert!(dialog.error().is_some_and(|error| error.contains("4000")));
        dialog.handle_input_event("\u{1b}");
        dialog.handle_input_event("\u{1b}");
        assert_eq!(
            *events.lock().expect("events"),
            [GoalQueueEditResult::Cancel {
                goal_id: "goal-1".to_owned()
            }]
        );
    }

    #[test]
    fn renders_scrolling_input_cursor_and_narrow_dialog_within_width() {
        let objective = (1..=12)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut dialog = GoalQueueEditDialogComponent::new(GoalQueueEditDialogOptions::new(
            goal(&objective),
            |_| {},
        ));
        dialog.focused = true;
        let lines = dialog.render(42);
        assert!(lines.iter().any(|line| line.contains("previous")));
        assert!(lines.iter().any(|line| line.contains(CURSOR_MARKER)));
        assert!(lines.iter().all(|line| visible_width(line) <= 42));
        assert!(dialog.render(3).iter().all(|line| visible_width(line) <= 3));
    }
}
