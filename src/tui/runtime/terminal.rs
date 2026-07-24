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
        queue!(self.stdout, MoveTo(0, 0), Clear(ClearType::All))?;
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                queue!(self.stdout, Print("\r\n"))?;
            }
            queue!(self.stdout, Print(line))?;
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
        KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => Some("\u{1b}[Z".to_owned()),
        KeyCode::Tab | KeyCode::BackTab => Some("\t".to_owned()),
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
    }

    #[test]
    fn ignores_key_release_events() {
        let mut event = key(KeyCode::Char('x'), KeyModifiers::NONE);
        event.kind = KeyEventKind::Release;
        assert_eq!(encode_key_event(event), None);
    }
}
