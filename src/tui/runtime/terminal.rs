use std::io::{self, Stdout, Write};

use async_trait::async_trait;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange, Event,
        EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute, queue,
    style::Print,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode, size,
    },
};
use futures_util::StreamExt;

use crate::tui::components::{core::CURSOR_MARKER, render::visible_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Input(String),
    Resize { columns: u16, rows: u16 },
}

#[async_trait]
pub trait TerminalBackend: Send {
    fn enter(&mut self) -> io::Result<()>;
    fn leave(&mut self) -> io::Result<()>;
    fn size(&self) -> io::Result<(u16, u16)>;
    fn draw(&mut self, lines: &[String]) -> io::Result<()>;
    async fn next_event(&mut self) -> io::Result<Option<TerminalEvent>>;
}

pub struct ProcessTerminal {
    stdout: Stdout,
    events: EventStream,
    entered: bool,
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessTerminal {
    pub fn new() -> Self {
        Self {
            stdout: io::stdout(),
            events: EventStream::new(),
            entered: false,
        }
    }
}

#[async_trait]
impl TerminalBackend for ProcessTerminal {
    fn enter(&mut self) -> io::Result<()> {
        if self.entered {
            return Ok(());
        }
        enable_raw_mode()?;
        if let Err(error) = execute!(
            self.stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableFocusChange,
            Hide
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        self.entered = true;
        Ok(())
    }

    fn leave(&mut self) -> io::Result<()> {
        if !self.entered {
            return Ok(());
        }
        let terminal_result = execute!(
            self.stdout,
            Show,
            DisableFocusChange,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let raw_result = disable_raw_mode();
        self.entered = false;
        terminal_result.and(raw_result)
    }

    fn size(&self) -> io::Result<(u16, u16)> {
        size()
    }

    fn draw(&mut self, lines: &[String]) -> io::Result<()> {
        let (_, terminal_rows) = size()?;
        let (lines, cursor) = prepare_terminal_frame(lines, terminal_rows);
        queue!(self.stdout, MoveTo(0, 0), Clear(ClearType::All))?;
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                queue!(self.stdout, Print("\r\n"))?;
            }
            queue!(self.stdout, Print(line))?;
        }
        if let Some((column, row)) = cursor {
            queue!(self.stdout, MoveTo(column, row), Show)?;
        } else {
            queue!(self.stdout, Hide)?;
        }
        self.stdout.flush()
    }

    async fn next_event(&mut self) -> io::Result<Option<TerminalEvent>> {
        loop {
            let Some(event) = self.events.next().await else {
                return Ok(None);
            };
            let event = event?;
            match event {
                Event::Key(key) => {
                    if let Some(input) = encode_key_event(key) {
                        return Ok(Some(TerminalEvent::Input(input)));
                    }
                }
                Event::Paste(text) => {
                    return Ok(Some(TerminalEvent::Input(format!(
                        "\u{1b}[200~{text}\u{1b}[201~"
                    ))));
                }
                Event::Resize(columns, rows) => {
                    return Ok(Some(TerminalEvent::Resize { columns, rows }));
                }
                Event::FocusGained => {
                    return Ok(Some(TerminalEvent::Input("\u{1b}[I".to_owned())));
                }
                Event::FocusLost => {
                    return Ok(Some(TerminalEvent::Input("\u{1b}[O".to_owned())));
                }
                Event::Mouse(_) => {}
            }
        }
    }
}

// Original:
//   packages/pi-tui/src/tui.ts
//   TUI.extractCursorPosition()
//
// Rust adaptation:
//   The full-redraw terminal backend strips pi-tui's zero-width APC marker
//   before writing a frame, then uses its visual location for the hardware
//   cursor. Terminals that do not interpret the private APC sequence would
//   otherwise display its payload (`pi:c`) as ordinary text.
fn prepare_terminal_frame(
    lines: &[String],
    terminal_rows: u16,
) -> (Vec<String>, Option<(u16, u16)>) {
    let mut output = lines.to_vec();
    let viewport_top = output
        .len()
        .saturating_sub(usize::from(terminal_rows).max(1));

    for row in (viewport_top..output.len()).rev() {
        let Some(marker_index) = output[row].find(CURSOR_MARKER) else {
            continue;
        };
        let column = visible_width(&output[row][..marker_index]);
        output[row].replace_range(marker_index..marker_index + CURSOR_MARKER.len(), "");
        return (
            output,
            Some((
                u16::try_from(column).unwrap_or(u16::MAX),
                u16::try_from(row - viewport_top).unwrap_or(u16::MAX),
            )),
        );
    }

    (output, None)
}

impl Drop for ProcessTerminal {
    fn drop(&mut self) {
        if self.entered {
            let _ = self.leave();
        }
    }
}

fn encode_key_event(event: KeyEvent) -> Option<String> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let modifiers = event.modifiers;
    match event.code {
        KeyCode::Char(character) => encode_character(character, modifiers),
        KeyCode::Enter => Some("\r".to_owned()),
        KeyCode::Backspace => Some("\u{7f}".to_owned()),
        KeyCode::Delete => Some("\u{1b}[3~".to_owned()),
        KeyCode::Esc => Some("\u{1b}".to_owned()),
        KeyCode::BackTab => Some("\u{1b}[Z".to_owned()),
        KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => Some("\u{1b}[Z".to_owned()),
        KeyCode::Tab => Some("\t".to_owned()),
        KeyCode::Up => Some("\u{1b}[A".to_owned()),
        KeyCode::Down => Some("\u{1b}[B".to_owned()),
        KeyCode::Left => Some(modified_arrow('D', modifiers)),
        KeyCode::Right => Some(modified_arrow('C', modifiers)),
        KeyCode::Home => Some("\u{1b}[H".to_owned()),
        KeyCode::End => Some("\u{1b}[F".to_owned()),
        KeyCode::PageUp => Some("\u{1b}[5~".to_owned()),
        KeyCode::PageDown => Some("\u{1b}[6~".to_owned()),
        KeyCode::Insert
        | KeyCode::F(_)
        | KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => None,
    }
}

fn encode_character(character: char, modifiers: KeyModifiers) -> Option<String> {
    if modifiers.contains(KeyModifiers::CONTROL) && character.is_ascii_alphabetic() {
        return Some(String::from(char::from(
            (character.to_ascii_lowercase() as u8) & 0x1f,
        )));
    }
    let mut encoded = String::new();
    if modifiers.contains(KeyModifiers::ALT) {
        encoded.push('\u{1b}');
    }
    encoded.push(character);
    Some(encoded)
}

fn modified_arrow(direction: char, modifiers: KeyModifiers) -> String {
    let modifier = if modifiers.contains(KeyModifiers::CONTROL) {
        5
    } else if modifiers.contains(KeyModifiers::ALT) {
        3
    } else {
        return format!("\u{1b}[{direction}");
    };
    format!("\u{1b}[1;{modifier}{direction}")
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEventState;

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn encodes_editor_keys_to_the_existing_pi_tui_compatible_sequences() {
        assert_eq!(
            encode_key_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL)).as_deref(),
            Some("\u{3}")
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Left, KeyModifiers::CONTROL)).as_deref(),
            Some("\u{1b}[1;5D")
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Char('s'), KeyModifiers::ALT)).as_deref(),
            Some("\u{1b}s")
        );
        assert_eq!(
            encode_key_event(key(KeyCode::Enter, KeyModifiers::NONE)).as_deref(),
            Some("\r")
        );
        assert_eq!(
            encode_key_event(key(KeyCode::BackTab, KeyModifiers::SHIFT)).as_deref(),
            Some("\u{1b}[Z")
        );
    }

    #[test]
    fn ignores_key_release_events() {
        let mut event = key(KeyCode::Char('x'), KeyModifiers::NONE);
        event.kind = KeyEventKind::Release;
        assert_eq!(encode_key_event(event), None);
    }

    #[test]
    fn strips_pi_tui_cursor_marker_and_preserves_its_visual_position() {
        let lines = vec![
            "header".to_owned(),
            format!("\u{1b}[31m> 你好{CURSOR_MARKER}\u{1b}[0m "),
        ];

        let (prepared, cursor) = prepare_terminal_frame(&lines, 24);

        assert_eq!(cursor, Some((6, 1)));
        assert!(!prepared.join("\n").contains("pi:c"));
        assert!(!prepared.join("\n").contains(CURSOR_MARKER));
        assert!(prepared[1].contains("> 你好"));
    }

    #[test]
    fn cursor_row_is_relative_to_the_visible_viewport() {
        let lines = vec![
            "old".to_owned(),
            "visible".to_owned(),
            format!("> {CURSOR_MARKER}"),
        ];

        let (_, cursor) = prepare_terminal_frame(&lines, 2);

        assert_eq!(cursor, Some((2, 1)));
    }
}
