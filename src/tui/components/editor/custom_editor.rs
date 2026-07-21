use std::{collections::HashMap, sync::Arc};

use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

use crate::tui::{
    components::render::visible_width,
    theme::{ColorToken, current_theme},
};

const CAPS_LOCK_BIT: u32 = 64;
const CTRL_BIT: u32 = 4;
const SHIFT_BIT: u32 = 1;
const EDITOR_LEFT_PADDING: usize = 4;
const CURSOR_BLOCK: &str = "\u{1b}[7m \u{1b}[0m";

type HistoryFilter = dyn Fn(&str) -> bool + Send + Sync;

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
}
