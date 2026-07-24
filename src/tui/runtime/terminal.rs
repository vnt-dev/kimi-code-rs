use std::io::{self, Stdout, Write};
#[cfg(windows)]
use std::{ffi::c_void, mem::MaybeUninit};

use async_trait::async_trait;
#[cfg(not(windows))]
use crossterm::event::{Event, EventStream};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange},
    execute, queue,
    style::Print,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode, size,
    },
};
#[cfg(not(windows))]
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
    #[cfg(not(windows))]
    events: EventStream,
    #[cfg(windows)]
    surrogate_buffer: Option<u16>,
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
            #[cfg(not(windows))]
            events: EventStream::new(),
            #[cfg(windows)]
            surrogate_buffer: None,
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
        #[cfg(windows)]
        {
            return self.next_windows_event().await;
        }
        #[cfg(not(windows))]
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

#[cfg(windows)]
impl ProcessTerminal {
    async fn next_windows_event(&mut self) -> io::Result<Option<TerminalEvent>> {
        loop {
            let record = tokio::task::spawn_blocking(read_windows_input_record)
                .await
                .map_err(io::Error::other)??;
            if let Some(event) = decode_windows_input_record(record, &mut self.surrogate_buffer)? {
                return Ok(Some(event));
            }
        }
    }
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct WindowsCoord {
    x: i16,
    y: i16,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct WindowsKeyEventRecord {
    key_down: i32,
    repeat_count: u16,
    virtual_key_code: u16,
    virtual_scan_code: u16,
    unicode_char: u16,
    control_key_state: u32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct WindowsBufferSizeRecord {
    size: WindowsCoord,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct WindowsFocusEventRecord {
    set_focus: i32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
union WindowsInputEvent {
    key: WindowsKeyEventRecord,
    buffer_size: WindowsBufferSizeRecord,
    focus: WindowsFocusEventRecord,
    storage: [u8; 16],
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct WindowsInputRecord {
    event_type: u16,
    event: WindowsInputEvent,
}

#[cfg(windows)]
const WINDOWS_KEY_EVENT: u16 = 0x0001;
#[cfg(windows)]
const WINDOWS_BUFFER_SIZE_EVENT: u16 = 0x0004;
#[cfg(windows)]
const WINDOWS_FOCUS_EVENT: u16 = 0x0010;
#[cfg(windows)]
const WINDOWS_STD_INPUT_HANDLE: i32 = -10;

#[cfg(windows)]
#[link(name = "Kernel32")]
unsafe extern "system" {
    fn GetStdHandle(standard_handle: i32) -> *mut c_void;
    fn ReadConsoleInputW(
        console_input: *mut c_void,
        buffer: *mut WindowsInputRecord,
        length: u32,
        events_read: *mut u32,
    ) -> i32;
}

#[cfg(windows)]
fn read_windows_input_record() -> io::Result<WindowsInputRecord> {
    // SAFETY:
    // GetStdHandle returns a process-owned console handle. We only pass that
    // handle to ReadConsoleInputW, with storage for exactly one INPUT_RECORD
    // and a valid output count pointer. The repr(C) definitions above mirror
    // the Win32 INPUT_RECORD layouts used by ReadConsoleInputW.
    unsafe {
        let handle = GetStdHandle(WINDOWS_STD_INPUT_HANDLE);
        if handle.is_null() || handle == (-1_isize as *mut c_void) {
            return Err(io::Error::last_os_error());
        }
        let mut record = MaybeUninit::<WindowsInputRecord>::uninit();
        let mut events_read = 0_u32;
        if ReadConsoleInputW(handle, record.as_mut_ptr(), 1, &mut events_read) == 0 {
            return Err(io::Error::last_os_error());
        }
        if events_read != 1 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "ReadConsoleInputW returned no terminal event",
            ));
        }
        Ok(record.assume_init())
    }
}

#[cfg(windows)]
fn decode_windows_input_record(
    record: WindowsInputRecord,
    surrogate_buffer: &mut Option<u16>,
) -> io::Result<Option<TerminalEvent>> {
    match record.event_type {
        WINDOWS_KEY_EVENT => {
            // SAFETY: event_type identifies the active INPUT_RECORD union arm.
            let key = unsafe { record.event.key };
            decode_windows_key_event(key, surrogate_buffer)
        }
        WINDOWS_BUFFER_SIZE_EVENT => {
            let (columns, rows) = size()?;
            Ok(Some(TerminalEvent::Resize { columns, rows }))
        }
        WINDOWS_FOCUS_EVENT => {
            // SAFETY: event_type identifies the active INPUT_RECORD union arm.
            let focus = unsafe { record.event.focus };
            let input = if focus.set_focus != 0 {
                "\u{1b}[I"
            } else {
                "\u{1b}[O"
            };
            Ok(Some(TerminalEvent::Input(input.to_owned())))
        }
        _ => Ok(None),
    }
}

#[cfg(windows)]
fn decode_windows_key_event(
    key: WindowsKeyEventRecord,
    surrogate_buffer: &mut Option<u16>,
) -> io::Result<Option<TerminalEvent>> {
    if key.key_down == 0 {
        return Ok(None);
    }

    let modifiers = windows_key_modifiers(key.control_key_state);
    let key_code = match key.virtual_key_code {
        0x08 => Some(KeyCode::Backspace),
        0x09 if modifiers.contains(KeyModifiers::SHIFT) => Some(KeyCode::BackTab),
        0x09 => Some(KeyCode::Tab),
        0x0d => Some(KeyCode::Enter),
        0x1b => Some(KeyCode::Esc),
        0x21 => Some(KeyCode::PageUp),
        0x22 => Some(KeyCode::PageDown),
        0x23 => Some(KeyCode::End),
        0x24 => Some(KeyCode::Home),
        0x25 => Some(KeyCode::Left),
        0x26 => Some(KeyCode::Up),
        0x27 => Some(KeyCode::Right),
        0x28 => Some(KeyCode::Down),
        0x2d => Some(KeyCode::Insert),
        0x2e => Some(KeyCode::Delete),
        _ => decode_windows_character(key, modifiers, surrogate_buffer)?,
    };
    let Some(key_code) = key_code else {
        return Ok(None);
    };
    Ok(encode_key_event(KeyEvent::new_with_kind(
        key_code,
        modifiers,
        KeyEventKind::Press,
    ))
    .map(TerminalEvent::Input))
}

#[cfg(windows)]
fn decode_windows_character(
    key: WindowsKeyEventRecord,
    modifiers: KeyModifiers,
    surrogate_buffer: &mut Option<u16>,
) -> io::Result<Option<KeyCode>> {
    match key.unicode_char {
        // Do not reconstruct unmodified ASCII from virtual-key codes here.
        // Those u_char == 0 records are IME composition keystrokes; Crossterm
        // reconstructing them is what appended strings such as "pi:c" to
        // committed Chinese input.
        0 if !modifiers.contains(KeyModifiers::CONTROL) => Ok(None),
        0 if (0x41..=0x5a).contains(&key.virtual_key_code) => {
            let character = char::from_u32(u32::from(key.virtual_key_code) + 0x20)
                .ok_or_else(|| io::Error::other("invalid virtual key code"))?;
            Ok(Some(KeyCode::Char(character)))
        }
        0 => Ok(None),
        high @ 0xd800..=0xdbff => {
            *surrogate_buffer = Some(high);
            Ok(None)
        }
        low @ 0xdc00..=0xdfff => {
            let Some(high) = surrogate_buffer.take() else {
                return Ok(None);
            };
            let mut decoded = char::decode_utf16([high, low]);
            Ok(decoded.next().and_then(Result::ok).map(KeyCode::Char))
        }
        scalar => {
            *surrogate_buffer = None;
            Ok(char::from_u32(u32::from(scalar)).map(KeyCode::Char))
        }
    }
}

#[cfg(windows)]
fn windows_key_modifiers(control_key_state: u32) -> KeyModifiers {
    let mut modifiers = KeyModifiers::NONE;
    if control_key_state & 0x0003 != 0 {
        modifiers.insert(KeyModifiers::ALT);
    }
    if control_key_state & 0x000c != 0 {
        modifiers.insert(KeyModifiers::CONTROL);
    }
    if control_key_state & 0x0010 != 0 {
        modifiers.insert(KeyModifiers::SHIFT);
    }
    modifiers
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

    #[cfg(windows)]
    #[test]
    fn windows_input_keeps_committed_cjk_and_drops_ime_composition_keys() {
        let mut surrogate = None;
        let composition = WindowsKeyEventRecord {
            key_down: 1,
            repeat_count: 1,
            virtual_key_code: u16::from(b'P'),
            virtual_scan_code: 0,
            unicode_char: 0,
            control_key_state: 0,
        };
        assert_eq!(
            decode_windows_key_event(composition, &mut surrogate).expect("composition"),
            None
        );

        let committed = WindowsKeyEventRecord {
            unicode_char: '你' as u16,
            ..composition
        };
        assert_eq!(
            decode_windows_key_event(committed, &mut surrogate).expect("committed"),
            Some(TerminalEvent::Input("你".to_owned()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_input_preserves_control_and_navigation_keys() {
        let mut surrogate = None;
        let ctrl_c = WindowsKeyEventRecord {
            key_down: 1,
            repeat_count: 1,
            virtual_key_code: u16::from(b'C'),
            virtual_scan_code: 0,
            unicode_char: 3,
            control_key_state: 0x0008,
        };
        assert_eq!(
            decode_windows_key_event(ctrl_c, &mut surrogate).expect("ctrl-c"),
            Some(TerminalEvent::Input("\u{3}".to_owned()))
        );

        let left = WindowsKeyEventRecord {
            virtual_key_code: 0x25,
            unicode_char: 0,
            control_key_state: 0,
            ..ctrl_c
        };
        assert_eq!(
            decode_windows_key_event(left, &mut surrogate).expect("left"),
            Some(TerminalEvent::Input("\u{1b}[D".to_owned()))
        );
    }
}
