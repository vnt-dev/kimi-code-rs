use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole, Input, InputAction,
        render::{truncate_to_width, visible_width},
    },
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
};

const FOOTER: &str = "Enter to submit  ·  Esc to cancel";
const EMPTY_HINT: &str = "API key cannot be empty.";
const INPUT_PREFIX: &str = "> ";
const IME_CURSOR_MARKER: &str = "\u{1b}_pi:c\u{7}";

type DoneCallback = dyn FnMut(ApiKeyInputResult) + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyInputResult {
    Ok { value: String },
    Cancel,
}

/// Protects terminal control sequences while masking each JavaScript UTF-16
/// input unit, matching `String.replaceAll(/./g, '•')` from the source.
///
/// Original: `api-key-input-dialog.ts`, `maskInputLine()`.
fn mask_input_line(raw: &str) -> String {
    let Some(remainder) = raw.strip_prefix(INPUT_PREFIX) else {
        return raw.to_owned();
    };
    let content_end = remainder.trim_end_matches(' ').len();
    let content = &remainder[..content_end];
    let padding = &remainder[content_end..];

    let mut masked = String::with_capacity(raw.len());
    masked.push_str(INPUT_PREFIX);
    let mut index = 0;
    while index < content.len() {
        if content[index..].starts_with(IME_CURSOR_MARKER) {
            masked.push_str(IME_CURSOR_MARKER);
            index += IME_CURSOR_MARKER.len();
            continue;
        }
        if let Some(end) = ansi_sgr_end(content, index) {
            masked.push_str(&content[index..end]);
            index = end;
            continue;
        }
        let character = content[index..].chars().next().expect("character boundary");
        masked.extend(std::iter::repeat_n('•', character.len_utf16()));
        index += character.len_utf8();
    }
    masked.push_str(padding);
    masked
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

/// Masked single-line API-key dialog.
///
/// Original: `src/tui/components/dialogs/api-key-input-dialog.ts`,
/// `ApiKeyInputDialogComponent`.
pub struct ApiKeyInputDialogComponent {
    pub focused: bool,
    input: Input,
    on_done: Box<DoneCallback>,
    title: String,
    subtitle_lines: Vec<String>,
    done: bool,
    empty_hinted: bool,
}

impl ApiKeyInputDialogComponent {
    pub fn new<F, S>(
        platform_name: impl AsRef<str>,
        subtitle_lines: impl IntoIterator<Item = S>,
        on_done: F,
    ) -> Self
    where
        F: FnMut(ApiKeyInputResult) + Send + 'static,
        S: Into<String>,
    {
        Self {
            focused: false,
            input: Input::new(),
            on_done: Box::new(on_done),
            title: format!("Enter API key for {}", platform_name.as_ref()),
            subtitle_lines: subtitle_lines.into_iter().map(Into::into).collect(),
            done: false,
            empty_hinted: false,
        }
    }

    /// Original: `ApiKeyInputDialogComponent.handleInput()`.
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
        if self.empty_hinted {
            self.empty_hinted = false;
        }
        if let Some(InputAction::Submit(value)) = self.input.handle_input_event(data) {
            self.submit(&value);
        }
    }

    /// Original: `ApiKeyInputDialogComponent.render()`.
    pub fn render_dialog(&mut self, width: usize) -> Vec<String> {
        self.input.focused = self.focused && !self.done;
        if width == 0 {
            return vec![String::new()];
        }

        let theme = current_theme();
        let inner_width = width.saturating_sub(4).max(1);
        let title = truncate_to_width(
            &theme.bold_fg(ColorToken::TextStrong, &self.title),
            inner_width,
            "…",
            false,
        );
        let subtitle_source: Vec<&str> = if self.empty_hinted {
            vec![EMPTY_HINT]
        } else {
            self.subtitle_lines.iter().map(String::as_str).collect()
        };
        let subtitles: Vec<String> = subtitle_source
            .into_iter()
            .map(|line| {
                truncate_to_width(
                    &theme.fg(ColorToken::TextDim, line),
                    inner_width,
                    "…",
                    false,
                )
            })
            .collect();
        let footer = truncate_to_width(
            &theme.fg(ColorToken::TextDim, FOOTER),
            inner_width,
            "…",
            false,
        );
        let raw_input = self.input.render_line(inner_width);
        let input = if self.input.value().is_empty() {
            raw_input
        } else {
            mask_input_line(&raw_input)
        };

        let mut content = vec![title, String::new()];
        content.extend(subtitles);
        content.extend([String::new(), input, String::new(), footer]);

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
            let right_padding = inner_width.saturating_sub(visible_width(&line));
            lines.push(format!(
                "{}  {line}{}{}",
                border("│"),
                " ".repeat(right_padding),
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

    /// Original: `ApiKeyInputDialogComponent.submit()`.
    fn submit(&mut self, value: &str) {
        if self.done {
            return;
        }
        let trimmed = value.trim();
        if trimmed.is_empty() {
            self.empty_hinted = true;
            return;
        }
        self.done = true;
        (self.on_done)(ApiKeyInputResult::Ok {
            value: trimmed.to_owned(),
        });
    }

    /// Original: `ApiKeyInputDialogComponent.cancel()`.
    fn cancel(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        (self.on_done)(ApiKeyInputResult::Cancel);
    }
}

impl Component for ApiKeyInputDialogComponent {
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
        self.input.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn recorder() -> (
        ApiKeyInputDialogComponent,
        Arc<Mutex<Vec<ApiKeyInputResult>>>,
    ) {
        let results = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&results);
        let component = ApiKeyInputDialogComponent::new(
            "Moonshot",
            ["Create a key in the console.", "It is stored locally."],
            move |result| recorded.lock().expect("API key results").push(result),
        );
        (component, results)
    }

    fn plain(lines: &[String]) -> String {
        let ansi = regex::Regex::new("\\x1b\\[[0-9;]*m").expect("ANSI regex");
        ansi.replace_all(&lines.join("\n"), "").into_owned()
    }

    #[test]
    fn masks_text_but_preserves_padding_cursor_and_utf16_width() {
        let raw = format!(
            "> a😀{}\u{1b}[7mZ\u{1b}[27m   ",
            crate::tui::components::core::CURSOR_MARKER
        );
        let masked = mask_input_line(&raw);
        assert_eq!(
            masked,
            format!(
                "> •••{}\u{1b}[7m•\u{1b}[27m   ",
                crate::tui::components::core::CURSOR_MARKER
            )
        );
        assert_eq!(mask_input_line("not an input"), "not an input");
    }

    #[test]
    fn trims_and_submits_secret_once_without_rendering_it() {
        let (mut dialog, results) = recorder();
        dialog.focused = true;
        dialog.handle_input_event("  sk-secret😀  ");
        let rendered = dialog.render_dialog(54);
        assert!(!rendered.join("").contains("sk-secret"));
        assert!(plain(&rendered).contains("•••••••••••"));
        dialog.handle_input_event("\r");
        dialog.handle_input_event("ignored");
        assert_eq!(
            *results.lock().expect("API key results"),
            [ApiKeyInputResult::Ok {
                value: "sk-secret😀".to_owned()
            }]
        );
        assert!(!dialog.render_dialog(54).join("").contains("sk-secret"));
    }

    #[test]
    fn empty_submit_replaces_subtitles_and_next_input_clears_hint() {
        let (mut dialog, results) = recorder();
        dialog.handle_input_event(" ");
        dialog.handle_input_event("\r");
        let empty = plain(&dialog.render_dialog(54));
        assert!(empty.contains(EMPTY_HINT));
        assert!(!empty.contains("Create a key"));
        assert!(results.lock().expect("API key results").is_empty());

        dialog.handle_input_event("x");
        let restored = plain(&dialog.render_dialog(54));
        assert!(restored.contains("Create a key"));
        assert!(!restored.contains(EMPTY_HINT));
    }

    #[test]
    fn all_cancel_keys_are_idempotent() {
        for key in ["\u{1b}", "\u{3}", "\u{4}"] {
            let (mut dialog, results) = recorder();
            dialog.handle_input_event(key);
            dialog.handle_input_event(key);
            assert_eq!(
                *results.lock().expect("API key results"),
                [ApiKeyInputResult::Cancel]
            );
        }
    }

    #[test]
    fn renders_box_focus_and_narrow_fallback_with_bounded_width() {
        let (mut dialog, _) = recorder();
        dialog.focused = true;
        let lines = dialog.render_dialog(54);
        let stripped = plain(&lines);
        assert!(stripped.contains("Enter API key for Moonshot"));
        assert!(stripped.contains("╭────────────────────────────────────────────────────╮"));
        assert!(lines.iter().all(|line| visible_width(line) <= 54));
        assert!(
            lines
                .join("")
                .contains(crate::tui::components::core::CURSOR_MARKER)
        );

        assert_eq!(dialog.render_dialog(0), [""]);
        assert!(
            dialog
                .render_dialog(3)
                .iter()
                .all(|line| visible_width(line) <= 3)
        );
    }
}
