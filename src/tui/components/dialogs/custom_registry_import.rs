use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole, Input, InputAction,
        render::{truncate_to_width, visible_width},
    },
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
};

const TITLE: &str = "Import custom provider registry";
const SUBTITLE_DEFAULT: &str = "Paste an api.json URL and its Bearer token.";
const SUBTITLE_URL_EMPTY: &str = "Registry URL cannot be empty.";
const SUBTITLE_TOKEN_EMPTY: &str = "Bearer token cannot be empty.";
const FOOTER_NOT_LAST: &str = "Tab / ↑↓ to switch  ·  Enter for next field  ·  Esc to cancel";
const FOOTER_LAST: &str = "Tab / ↑↓ to switch  ·  Enter to submit  ·  Esc to cancel";
const INPUT_PREFIX: &str = "> ";
const IME_CURSOR_MARKER: &str = "\u{1b}_pi:c\u{7}";

type DoneCallback = dyn FnMut(CustomRegistryImportResult) + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRegistryImportValue {
    pub url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomRegistryImportResult {
    Ok(CustomRegistryImportValue),
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldId {
    Url,
    Token,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hint {
    None,
    UrlEmpty,
    TokenEmpty,
}

/// Two-field custom provider-registry credential dialog.
///
/// Original: `custom-registry-import.ts`,
/// `CustomRegistryImportDialogComponent`.
pub struct CustomRegistryImportDialogComponent {
    pub focused: bool,
    url_input: Input,
    token_input: Input,
    on_done: Box<DoneCallback>,
    active_field: FieldId,
    done: bool,
    hint: Hint,
}

impl CustomRegistryImportDialogComponent {
    pub fn new<F>(on_done: F, default_url: impl Into<String>) -> Self
    where
        F: FnMut(CustomRegistryImportResult) + Send + 'static,
    {
        let mut url_input = Input::new();
        let default_url = default_url.into();
        if !default_url.is_empty() {
            url_input.set_value(default_url);
        }
        Self {
            focused: false,
            url_input,
            token_input: Input::new(),
            on_done: Box::new(on_done),
            active_field: FieldId::Url,
            done: false,
            hint: Hint::None,
        }
    }

    pub fn without_default<F>(on_done: F) -> Self
    where
        F: FnMut(CustomRegistryImportResult) + Send + 'static,
    {
        Self::new(on_done, "")
    }

    /// Original: `CustomRegistryImportDialogComponent.handleInput()`.
    pub fn handle_input_event(&mut self, data: &str) {
        if self.done {
            return;
        }
        if matches_editor_key(data, EditorKey::Escape)
            || matches_editor_key(data, EditorKey::Ctrl('c'))
            || matches_editor_key(data, EditorKey::Ctrl('d'))
        {
            self.cancel();
            return;
        }
        if matches_editor_key(data, EditorKey::Tab) || matches_editor_key(data, EditorKey::ShiftTab)
        {
            self.toggle_field();
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.focus_field(FieldId::Token);
            return;
        }
        if matches_editor_key(data, EditorKey::Up) {
            self.focus_field(FieldId::Url);
            return;
        }
        if self.hint != Hint::None {
            self.hint = Hint::None;
        }
        match self.active_field {
            FieldId::Url => {
                if matches!(
                    self.url_input.handle_input_event(data),
                    Some(InputAction::Submit(_))
                ) {
                    self.focus_field(FieldId::Token);
                }
            }
            FieldId::Token => {
                if matches!(
                    self.token_input.handle_input_event(data),
                    Some(InputAction::Submit(_))
                ) {
                    self.handle_submit();
                }
            }
        }
    }

    /// Original: `CustomRegistryImportDialogComponent.render()`.
    pub fn render_dialog(&mut self, width: usize) -> Vec<String> {
        let active = self.focused && !self.done;
        self.url_input.focused = active && self.active_field == FieldId::Url;
        self.token_input.focused = active && self.active_field == FieldId::Token;
        if width == 0 {
            return vec![String::new()];
        }

        let theme = current_theme();
        let inner_width = width.saturating_sub(4).max(1);
        let subtitle = match self.hint {
            Hint::None => SUBTITLE_DEFAULT,
            Hint::UrlEmpty => SUBTITLE_URL_EMPTY,
            Hint::TokenEmpty => SUBTITLE_TOKEN_EMPTY,
        };
        let footer = if self.active_field == FieldId::Url {
            FOOTER_NOT_LAST
        } else {
            FOOTER_LAST
        };
        let active_label = |field, label: &str| {
            if self.active_field == field {
                theme.bold_fg(ColorToken::Accent, label)
            } else {
                theme.fg(ColorToken::TextDim, label)
            }
        };
        let fit = |text: String| truncate_to_width(&text, inner_width, "…", false);
        let content = vec![
            fit(theme.bold_fg(ColorToken::TextStrong, TITLE)),
            String::new(),
            fit(theme.fg(ColorToken::TextDim, subtitle)),
            String::new(),
            fit(active_label(FieldId::Url, "Registry URL")),
            self.url_input.render_line(inner_width),
            String::new(),
            fit(active_label(FieldId::Token, "Bearer token")),
            mask_input_line(&self.token_input.render_line(inner_width)),
            String::new(),
            fit(theme.fg(ColorToken::TextDim, footer)),
        ];

        if width < 4 {
            let mut lines = vec![String::new()];
            lines.extend(
                content
                    .into_iter()
                    .map(|line| truncate_to_width(&line, width, "…", false)),
            );
            return lines;
        }
        let border = |text: &str| theme.fg(ColorToken::Primary, text);
        let horizontal = "─".repeat(width - 2);
        let mut lines = vec![
            String::new(),
            border(&format!("╭{horizontal}╮")),
            format!("{}{}{}", border("│"), " ".repeat(width - 2), border("│")),
        ];
        for line in content {
            let padding = inner_width.saturating_sub(visible_width(&line));
            lines.push(format!(
                "{}  {line}{}{}",
                border("│"),
                " ".repeat(padding),
                border("│")
            ));
        }
        lines.push(format!(
            "{}{}{}",
            border("│"),
            " ".repeat(width - 2),
            border("│")
        ));
        lines.push(border(&format!("╰{horizontal}╯")));
        lines.push(String::new());
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }

    /// Original: `CustomRegistryImportDialogComponent.toggleField()`.
    fn toggle_field(&mut self) {
        self.focus_field(match self.active_field {
            FieldId::Url => FieldId::Token,
            FieldId::Token => FieldId::Url,
        });
    }

    /// Original: `CustomRegistryImportDialogComponent.focusField()`.
    fn focus_field(&mut self, field: FieldId) {
        self.hint = Hint::None;
        self.active_field = field;
    }

    /// Original: `CustomRegistryImportDialogComponent.handleSubmit()`.
    fn handle_submit(&mut self) {
        if self.done {
            return;
        }
        let url = self.url_input.value().trim().to_owned();
        let api_key = self.token_input.value().trim().to_owned();
        if url.is_empty() {
            self.hint = Hint::UrlEmpty;
            self.active_field = FieldId::Url;
            return;
        }
        if api_key.is_empty() {
            self.hint = Hint::TokenEmpty;
            self.active_field = FieldId::Token;
            return;
        }
        self.done = true;
        (self.on_done)(CustomRegistryImportResult::Ok(CustomRegistryImportValue {
            url,
            api_key,
        }));
    }

    /// Original: `CustomRegistryImportDialogComponent.cancel()`.
    fn cancel(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        (self.on_done)(CustomRegistryImportResult::Cancel);
    }
}

impl Component for CustomRegistryImportDialogComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_dialog(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }

    fn wants_key_release(&self) -> bool {
        true
    }

    fn invalidate(&mut self) {
        self.url_input.invalidate();
        self.token_input.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Original: `custom-registry-import.ts`, `maskInputLine()`.
fn mask_input_line(raw: &str) -> String {
    let Some(remainder) = raw.strip_prefix(INPUT_PREFIX) else {
        return raw.to_owned();
    };
    let content_end = remainder.trim_end_matches(' ').len();
    let content = &remainder[..content_end];
    let padding = &remainder[content_end..];
    let mut output = INPUT_PREFIX.to_owned();
    let mut index = 0;
    while index < content.len() {
        if content[index..].starts_with(IME_CURSOR_MARKER) {
            output.push_str(IME_CURSOR_MARKER);
            index += IME_CURSOR_MARKER.len();
            continue;
        }
        if let Some(end) = ansi_sgr_end(content, index) {
            output.push_str(&content[index..end]);
            index = end;
            continue;
        }
        let character = content[index..].chars().next().expect("character boundary");
        if character == ' ' {
            output.push(' ');
        } else {
            output.extend(std::iter::repeat_n('•', character.len_utf16()));
        }
        index += character.len_utf8();
    }
    output.push_str(padding);
    output
}

fn ansi_sgr_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start..start + 2)? != b"\x1b[" {
        return None;
    }
    let mut index = start + 2;
    while let Some(byte) = bytes.get(index) {
        if *byte == b'm' {
            return Some(index + 1);
        }
        if !byte.is_ascii_digit() && *byte != b';' {
            return None;
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn dialog() -> (
        CustomRegistryImportDialogComponent,
        Arc<Mutex<Vec<CustomRegistryImportResult>>>,
    ) {
        let results = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&results);
        let dialog = CustomRegistryImportDialogComponent::without_default(move |result| {
            recorded.lock().expect("registry results").push(result);
        });
        (dialog, results)
    }

    fn plain(lines: &[String]) -> String {
        let ansi = regex::Regex::new("\\x1b\\[[0-9;]*m").expect("ANSI regex");
        ansi.replace_all(&lines.join("\n"), "").into_owned()
    }

    #[test]
    fn url_enter_advances_and_token_enter_submits_trimmed_values_once() {
        let (mut dialog, results) = dialog();
        dialog.handle_input_event("  https://registry.test/api.json  ");
        dialog.handle_input_event("\r");
        dialog.handle_input_event("  secret token  ");
        dialog.handle_input_event("\r");
        dialog.handle_input_event("ignored");
        assert_eq!(
            *results.lock().expect("registry results"),
            [CustomRegistryImportResult::Ok(CustomRegistryImportValue {
                url: "https://registry.test/api.json".to_owned(),
                api_key: "secret token".to_owned()
            })]
        );
    }

    #[test]
    fn token_submit_validates_url_then_token_and_focuses_invalid_field() {
        let (mut dialog, results) = dialog();
        dialog.handle_input_event("\t");
        dialog.handle_input_event("\r");
        assert!(plain(&dialog.render_dialog(70)).contains(SUBTITLE_URL_EMPTY));
        dialog.handle_input_event("https://x");
        dialog.handle_input_event("\t");
        dialog.handle_input_event("\r");
        assert!(plain(&dialog.render_dialog(70)).contains(SUBTITLE_TOKEN_EMPTY));
        assert!(results.lock().expect("registry results").is_empty());
    }

    #[test]
    fn tab_shift_tab_and_arrows_switch_fields_and_clear_hints() {
        let (mut dialog, _) = dialog();
        dialog.handle_input_event("\t");
        assert!(plain(&dialog.render_dialog(70)).contains(FOOTER_LAST));
        dialog.handle_input_event("\u{1b}[Z");
        assert!(plain(&dialog.render_dialog(70)).contains(FOOTER_NOT_LAST));
        dialog.handle_input_event("\u{1b}[B");
        dialog.handle_input_event("\u{1b}[A");
        assert!(plain(&dialog.render_dialog(70)).contains(FOOTER_NOT_LAST));
    }

    #[test]
    fn masks_non_space_utf16_units_and_preserves_terminal_sequences() {
        let raw = format!(
            "> a b😀{}\u{1b}[7mZ\u{1b}[27m  ",
            crate::tui::components::core::CURSOR_MARKER
        );
        assert_eq!(
            mask_input_line(&raw),
            format!(
                "> • •••{}\u{1b}[7m•\u{1b}[27m  ",
                crate::tui::components::core::CURSOR_MARKER
            )
        );
    }

    #[test]
    fn cancel_keys_are_idempotent_and_render_is_bounded() {
        for key in ["\u{1b}", "\u{3}", "\u{4}"] {
            let (mut dialog, results) = dialog();
            dialog.handle_input_event(key);
            dialog.handle_input_event(key);
            assert_eq!(
                *results.lock().expect("registry results"),
                [CustomRegistryImportResult::Cancel]
            );
        }
        let (mut dialog, _) = dialog();
        dialog.focused = true;
        let lines = dialog.render_dialog(70);
        assert!(lines.iter().all(|line| visible_width(line) <= 70));
        assert!(
            lines
                .join("")
                .contains(crate::tui::components::core::CURSOR_MARKER)
        );
        assert_eq!(dialog.render_dialog(0), [""]);
    }

    #[test]
    fn default_url_preserves_source_set_value_cursor_behavior() {
        let mut dialog = CustomRegistryImportDialogComponent::new(|_| {}, "https://default/");
        dialog.handle_input_event("X");
        let rendered = plain(&dialog.render_dialog(70));
        assert!(rendered.contains("> Xhttps://default/"));
    }
}
