use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        editor::{CustomEditor, EditorAction, InputMode},
        render::truncate_to_width,
    },
    runtime::{TuiApp, TuiControl},
    theme::{ColorToken, current_theme},
};

const DEFAULT_PENDING_RESPONSE: &str =
    "Agent runtime is not connected yet. Your input was received.";

#[derive(Debug, Clone, PartialEq, Eq)]
enum TranscriptLine {
    User(String),
    Assistant(String),
    System(String),
}

// Original:
//   apps/kimi-code/src/tui/kimi-tui.ts
//   KimiTUI coordinator
//
// Rust adaptation:
//   This first interactive coordinator owns only layout, editor input, and
//   visible default responses. Session creation, v2 event routing, dialogs,
//   and command side effects remain explicit MIGRATION-TODO boundaries. The
//   defaults keep every accepted user action inside the live TUI instead of
//   panicking at an unimplemented backend call.
pub struct KimiTui {
    version: String,
    editor: CustomEditor,
    transcript: Vec<TranscriptLine>,
    status: Option<String>,
    terminal_rows: usize,
    startup_warning: Option<String>,
}

impl KimiTui {
    pub fn new(version: impl Into<String>, startup_warning: Option<String>) -> Self {
        let mut editor = CustomEditor::new();
        editor.set_focused(true);
        Self {
            version: version.into(),
            editor,
            transcript: Vec::new(),
            status: Some("Ready".to_owned()),
            terminal_rows: 24,
            startup_warning,
        }
    }

    pub fn editor_text(&self) -> String {
        self.editor.text()
    }

    fn handle_editor_action(&mut self, action: EditorAction) -> TuiControl {
        match action {
            EditorAction::Submit(text) => self.submit(text),
            EditorAction::CtrlC => {
                if self.editor.text().is_empty() {
                    TuiControl::Exit
                } else {
                    self.editor.set_text("");
                    self.status = Some("Input cleared. Press Ctrl+C again to exit.".to_owned());
                    TuiControl::Continue
                }
            }
            EditorAction::CtrlD => TuiControl::Exit,
            EditorAction::UpArrowEmptyWithHistoryFallback => {
                self.editor.apply_up_arrow_history_fallback();
                TuiControl::Continue
            }
            EditorAction::DownArrowEmptyWithHistoryFallback => {
                self.editor.apply_down_arrow_history_fallback();
                TuiControl::Continue
            }
            EditorAction::CtrlBWithCursorLeftFallback => {
                self.editor.apply_ctrl_b_fallback();
                TuiControl::Continue
            }
            EditorAction::Escape => {
                self.status = Some("No dialog is open.".to_owned());
                TuiControl::Continue
            }
            EditorAction::OpenExternalEditor => self.pending_action("External editor"),
            EditorAction::ToggleToolExpand => self.pending_action("Tool output expansion"),
            EditorAction::CtrlS => self.pending_action("Session picker"),
            EditorAction::ToggleTodoWithDefaultFallback => self.pending_action("Todo panel"),
            EditorAction::ShiftTab => self.pending_action("Permission mode switch"),
            EditorAction::UndoShortcut => {
                self.status = Some("Editor undo applied.".to_owned());
                TuiControl::Continue
            }
            EditorAction::PasteImage => self.pending_action("Clipboard image paste"),
            EditorAction::InputModeChanged(InputMode::Prompt) => {
                self.status = Some("Prompt mode".to_owned());
                TuiControl::Continue
            }
            EditorAction::InputModeChanged(InputMode::Bash) => {
                self.status = Some("Shell mode (execution not connected)".to_owned());
                TuiControl::Continue
            }
            EditorAction::AutocompleteCancelled => {
                self.status = Some("Autocomplete cancelled.".to_owned());
                TuiControl::Continue
            }
            EditorAction::RequestAutocomplete { .. } => {
                // MIGRATION-TODO:
                // Original: CustomEditor requests file and slash-command
                // completions from KimiTUI. The editor remains usable while
                // the v2-backed catalog is not yet composed.
                TuiControl::Continue
            }
            EditorAction::NonEscapeInput => TuiControl::Continue,
        }
    }

    fn pending_action(&mut self, action: &str) -> TuiControl {
        // MIGRATION-TODO:
        // Original: apps/kimi-code/src/tui/kimi-tui.ts dispatches this action
        // to a controller or session service.
        // Temporary behavior: keep the UI alive and show a deterministic
        // acknowledgement.
        self.status = Some(format!("{action} is not connected yet."));
        TuiControl::Continue
    }

    fn submit(&mut self, text: String) -> TuiControl {
        let text = text.trim().to_owned();
        if text.is_empty() {
            self.status = Some("Enter a message or /help.".to_owned());
            return TuiControl::Continue;
        }
        self.editor.add_to_history(&text);
        if let Some(command) = text.strip_prefix('/') {
            return self.handle_slash_command(command);
        }

        self.transcript.push(TranscriptLine::User(text));
        // MIGRATION-TODO:
        // Original: KimiTUI sends the prompt through KimiHarness.Session.
        // Completion condition: create/resume a v2 session, enqueue the turn,
        // and route DomainEvent values into transcript components.
        self.transcript.push(TranscriptLine::Assistant(
            DEFAULT_PENDING_RESPONSE.to_owned(),
        ));
        self.status = Some("Default response shown; v2 turn execution is pending.".to_owned());
        TuiControl::Continue
    }

    fn handle_slash_command(&mut self, command: &str) -> TuiControl {
        let (name, _) = command.split_once(' ').unwrap_or((command, ""));
        match name {
            "exit" | "quit" => TuiControl::Exit,
            "clear" => {
                self.transcript.clear();
                self.status = Some("Transcript cleared.".to_owned());
                TuiControl::Continue
            }
            "help" => {
                self.transcript.push(TranscriptLine::System(
                    "Available now: /help, /clear, /exit. Other commands return a migration notice."
                        .to_owned(),
                ));
                self.status = Some("Help".to_owned());
                TuiControl::Continue
            }
            "" => {
                self.status = Some("Enter a slash command.".to_owned());
                TuiControl::Continue
            }
            other => {
                // MIGRATION-TODO:
                // Original: commands/dispatch.ts and KimiTUI route the full
                // slash-command registry. Preserve an observable response
                // until each command's v2 service is connected.
                self.transcript.push(TranscriptLine::System(format!(
                    "/{other} is recognized by the migration shell but is not implemented yet."
                )));
                self.status = Some("Command acknowledged.".to_owned());
                TuiControl::Continue
            }
        }
    }

    fn render_transcript_line(line: &TranscriptLine) -> String {
        match line {
            TranscriptLine::User(text) => format!(
                "{} {text}",
                current_theme().bold_fg(ColorToken::RoleUser, "You:")
            ),
            TranscriptLine::Assistant(text) => format!(
                "{} {text}",
                current_theme().bold_fg(ColorToken::Primary, "Kimi:")
            ),
            TranscriptLine::System(text) => current_theme().fg(ColorToken::TextMuted, text),
        }
    }
}

impl Component for KimiTui {
    fn render(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let title = current_theme().bold_fg(ColorToken::Primary, "Kimi Code");
        let subtitle = current_theme().fg(
            ColorToken::TextMuted,
            &format!(
                "Rust interactive migration shell v{} · /help for commands",
                self.version
            ),
        );
        let mut lines = vec![
            truncate_to_width(&title, width, "…", false),
            truncate_to_width(&subtitle, width, "…", false),
            String::new(),
        ];
        if let Some(warning) = self.startup_warning.take() {
            self.transcript.push(TranscriptLine::System(warning));
        }

        let editor_lines = self.editor.render_editor(width);
        let reserved = editor_lines.len().saturating_add(5);
        let transcript_capacity = self.terminal_rows.saturating_sub(reserved).max(1);
        let transcript_start = self.transcript.len().saturating_sub(transcript_capacity);
        lines.extend(
            self.transcript[transcript_start..]
                .iter()
                .map(Self::render_transcript_line)
                .map(|line| truncate_to_width(&line, width, "…", false)),
        );
        lines.push(String::new());
        if let Some(status) = &self.status {
            let status = current_theme().fg(ColorToken::TextMuted, status);
            lines.push(truncate_to_width(&status, width, "…", false));
        }
        lines.extend(editor_lines);
        lines
    }

    fn handle_input(&mut self, data: &str) {
        let _ = self.handle_terminal_input(data);
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl TuiApp for KimiTui {
    fn handle_terminal_input(&mut self, data: &str) -> TuiControl {
        if data == "\u{1b}[I" || data == "\u{1b}[O" {
            return TuiControl::Continue;
        }
        let outcome = self.editor.handle_input_event(data);
        for action in outcome.actions {
            if self.handle_editor_action(action) == TuiControl::Exit {
                return TuiControl::Exit;
            }
        }
        TuiControl::Continue
    }

    fn handle_terminal_resize(&mut self, _columns: u16, rows: u16) {
        self.terminal_rows = usize::from(rows).max(1);
        self.editor.set_terminal_rows(self.terminal_rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::render::visible_width;

    #[test]
    fn accepts_text_and_returns_a_visible_default_response() {
        let mut tui = KimiTui::new("0.1.0", None);
        for input in ["h", "i", "\r"] {
            assert_eq!(tui.handle_terminal_input(input), TuiControl::Continue);
        }

        let rendered = tui.render(80).join("\n");
        assert!(rendered.contains("You:"));
        assert!(rendered.contains("hi"));
        assert!(rendered.contains(DEFAULT_PENDING_RESPONSE));
        assert_eq!(tui.editor_text(), "");
    }

    #[test]
    fn routes_builtin_and_pending_slash_commands_without_panicking() {
        let mut tui = KimiTui::new("0.1.0", None);
        for input in ["/", "h", "e", "l", "p", "\r"] {
            assert_eq!(tui.handle_terminal_input(input), TuiControl::Continue);
        }
        assert!(tui.render(80).join("\n").contains("Available now"));

        for input in ["/", "m", "o", "d", "e", "l", "\r"] {
            assert_eq!(tui.handle_terminal_input(input), TuiControl::Continue);
        }
        assert!(tui.render(80).join("\n").contains("/model is recognized"));
    }

    #[test]
    fn ctrl_c_clears_input_then_exits_and_render_respects_width() {
        let mut tui = KimiTui::new("0.1.0", Some("config warning".to_owned()));
        tui.handle_terminal_resize(24, 10);
        assert_eq!(tui.handle_terminal_input("x"), TuiControl::Continue);
        assert_eq!(tui.handle_terminal_input("\u{3}"), TuiControl::Continue);
        assert_eq!(tui.editor_text(), "");
        assert_eq!(tui.handle_terminal_input("\u{3}"), TuiControl::Exit);
        for line in tui.render(24) {
            assert!(
                visible_width(&line) <= 24,
                "line exceeds terminal width: {line:?}"
            );
        }
    }
}
