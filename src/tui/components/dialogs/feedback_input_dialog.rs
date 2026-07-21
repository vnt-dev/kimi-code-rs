use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole, Input, InputAction,
        render::{truncate_to_width, visible_width},
    },
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
};

const TITLE: &str = "Send feedback to Kimi Code";
const SUBTITLE_DEFAULT: &str = "Tell us what's working or what's not.";
const SUBTITLE_EMPTY: &str = "Feedback cannot be empty.";
const FOOTER: &str = "Enter to submit  ·  Esc to cancel";

type DoneCallback = dyn FnMut(FeedbackInputDialogResult) + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackInputDialogResult {
    Ok { value: String },
    Cancel,
}

/// Blue rounded dialog that collects the free-form feedback text.
///
/// Original: `src/tui/components/dialogs/feedback-input-dialog.ts`,
/// `FeedbackInputDialogComponent`.
pub struct FeedbackInputDialogComponent {
    pub focused: bool,
    input: Input,
    on_done: Box<DoneCallback>,
    done: bool,
    empty_hinted: bool,
}

impl FeedbackInputDialogComponent {
    pub fn new<F>(on_done: F) -> Self
    where
        F: FnMut(FeedbackInputDialogResult) + Send + 'static,
    {
        Self {
            focused: false,
            input: Input::new(),
            on_done: Box::new(on_done),
            done: false,
            empty_hinted: false,
        }
    }

    /// Original: `FeedbackInputDialogComponent.handleInput()`.
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

    /// Original: `FeedbackInputDialogComponent.render()`.
    pub fn render_dialog(&mut self, width: usize) -> Vec<String> {
        self.input.focused = self.focused && !self.done;
        if width == 0 {
            return vec![String::new()];
        }

        let theme = current_theme();
        let inner_width = width.saturating_sub(4).max(1);
        let title = truncate_to_width(
            &theme.bold_fg(ColorToken::TextStrong, TITLE),
            inner_width,
            "…",
            false,
        );
        let subtitle_text = if self.empty_hinted {
            SUBTITLE_EMPTY
        } else {
            SUBTITLE_DEFAULT
        };
        let subtitle = truncate_to_width(
            &theme.fg(ColorToken::TextDim, subtitle_text),
            inner_width,
            "…",
            false,
        );
        let footer = truncate_to_width(
            &theme.fg(ColorToken::TextDim, FOOTER),
            inner_width,
            "…",
            false,
        );
        let input = self.input.render_line(inner_width);
        let content = vec![
            title,
            String::new(),
            subtitle,
            String::new(),
            input,
            String::new(),
            footer,
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

    /// Original: `FeedbackInputDialogComponent.submit()`.
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
        (self.on_done)(FeedbackInputDialogResult::Ok {
            value: trimmed.to_owned(),
        });
    }

    /// Original: `FeedbackInputDialogComponent.cancel()`.
    fn cancel(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        (self.on_done)(FeedbackInputDialogResult::Cancel);
    }
}

impl Component for FeedbackInputDialogComponent {
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
        FeedbackInputDialogComponent,
        Arc<Mutex<Vec<FeedbackInputDialogResult>>>,
    ) {
        let results = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&results);
        let component = FeedbackInputDialogComponent::new(move |result| {
            recorded.lock().expect("feedback results").push(result);
        });
        (component, results)
    }

    fn plain(lines: &[String]) -> String {
        let ansi = regex::Regex::new("\\x1b\\[[0-9;]*m").expect("ANSI regex");
        ansi.replace_all(&lines.join("\n"), "").into_owned()
    }

    #[test]
    fn trims_and_submits_once() {
        let (mut dialog, results) = recorder();
        dialog.handle_input_event("  useful feedback  ");
        dialog.handle_input_event("\r");
        dialog.handle_input_event("ignored");
        dialog.handle_input_event("\r");
        assert_eq!(
            *results.lock().expect("feedback results"),
            [FeedbackInputDialogResult::Ok {
                value: "useful feedback".to_owned()
            }]
        );
    }

    #[test]
    fn empty_submit_shows_hint_until_the_next_input() {
        let (mut dialog, results) = recorder();
        dialog.handle_input_event("   ");
        dialog.handle_input_event("\r");
        assert!(plain(&dialog.render_dialog(48)).contains(SUBTITLE_EMPTY));
        assert!(results.lock().expect("feedback results").is_empty());

        dialog.handle_input_event("x");
        assert!(plain(&dialog.render_dialog(48)).contains(SUBTITLE_DEFAULT));
    }

    #[test]
    fn escape_ctrl_c_and_ctrl_d_cancel_once() {
        for key in ["\u{1b}", "\u{3}", "\u{4}"] {
            let (mut dialog, results) = recorder();
            dialog.handle_input_event(key);
            dialog.handle_input_event(key);
            assert_eq!(
                *results.lock().expect("feedback results"),
                [FeedbackInputDialogResult::Cancel]
            );
        }
    }

    #[test]
    fn renders_focus_cursor_full_box_and_narrow_fallback() {
        let (mut dialog, _) = recorder();
        dialog.focused = true;
        let lines = dialog.render_dialog(42);
        let stripped = plain(&lines);
        assert!(stripped.contains(TITLE));
        assert!(stripped.contains("╭────────────────────────────────────────╮"));
        assert!(lines.iter().all(|line| visible_width(line) <= 42));
        assert!(
            lines
                .join("")
                .contains(crate::tui::components::core::CURSOR_MARKER)
        );

        assert_eq!(dialog.render_dialog(0), [""]);
        let narrow = dialog.render_dialog(3);
        assert_eq!(narrow.len(), 8);
        assert!(narrow.iter().all(|line| visible_width(line) <= 3));
    }
}
