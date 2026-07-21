use std::{cell::RefCell, rc::Rc};

use crate::tui::theme::{colors::ResolvedTheme, terminal_background::find_osc11_background_theme};

use super::{
    terminal_focus::{InputListenerRegistry, TerminalInputResult},
    terminal_notification::TerminalWrite,
};

#[cfg(test)]
const ESC: &str = "\u{1b}";
#[cfg(test)]
const BEL: &str = "\u{7}";
pub const QUERY_TERMINAL_THEME: &str = "\u{1b}[?996n";
pub const TERMINAL_THEME_DARK: &str = "\u{1b}[?997;1n";
pub const TERMINAL_THEME_LIGHT: &str = "\u{1b}[?997;2n";
pub const ENABLE_TERMINAL_THEME_REPORTING: &str = "\u{1b}[?2031h";
pub const DISABLE_TERMINAL_THEME_REPORTING: &str = "\u{1b}[?2031l";
pub const OSC11_QUERY: &str = "\u{1b}]11;?\u{7}";
const OSC11_RESPONSE_PREFIX: &str = "\u{1b}]11;rgb:";
const OSC11_RESPONSE_PREFIX_NO_ESC: &str = "]11;rgb:";
const TERMINAL_THEME_INPUT_BUFFER_MAX_LENGTH: usize = 512;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalThemeInputState {
    pub osc11_buffer: String,
}

pub fn create_terminal_theme_input_state() -> TerminalThemeInputState {
    TerminalThemeInputState::default()
}

pub fn has_terminal_theme_report(data: &str) -> bool {
    data.contains(TERMINAL_THEME_DARK) || data.contains(TERMINAL_THEME_LIGHT)
}

/// Original:
///   apps/kimi-code/src/tui/utils/terminal-theme.ts
///   handleTerminalThemeInput()
pub fn handle_terminal_theme_input(
    data: &str,
    terminal: &mut dyn TerminalWrite,
    on_theme: &mut dyn FnMut(ResolvedTheme),
    input_state: &mut TerminalThemeInputState,
) -> Option<TerminalInputResult> {
    if !input_state.osc11_buffer.is_empty() {
        let candidate = format!("{}{data}", input_state.osc11_buffer);
        let stripped = strip_osc11_reports(&candidate, on_theme);
        if stripped != candidate {
            input_state.osc11_buffer.clear();
            return Some(result_from_remaining(stripped));
        }

        input_state.osc11_buffer =
            if candidate.encode_utf16().count() > TERMINAL_THEME_INPUT_BUFFER_MAX_LENGTH {
                String::new()
            } else {
                candidate
            };
        return Some(TerminalInputResult::Consume);
    }

    let mut remaining = strip_osc11_reports(data, on_theme);
    remaining = strip_terminal_theme_reports(&remaining, terminal);

    if let Some(partial_start) = find_partial_osc11_start(&remaining) {
        input_state.osc11_buffer = remaining[partial_start..].to_owned();
        return Some(result_from_remaining(remaining[..partial_start].to_owned()));
    }

    if remaining != data {
        return Some(result_from_remaining(remaining));
    }
    None
}

fn strip_osc11_reports(data: &str, on_theme: &mut dyn FnMut(ResolvedTheme)) -> String {
    let mut remaining = data.to_owned();
    while let Some((range, theme)) = find_osc11_background_theme(&remaining) {
        on_theme(theme);
        remaining.replace_range(range, "");
    }
    remaining
}

fn strip_terminal_theme_reports(data: &str, terminal: &mut dyn TerminalWrite) -> String {
    let mut remaining = data.to_owned();
    let mut stripped_report = false;
    for report in [TERMINAL_THEME_DARK, TERMINAL_THEME_LIGHT] {
        if remaining.contains(report) {
            remaining = remaining.replace(report, "");
            stripped_report = true;
        }
    }
    if stripped_report {
        terminal.write(OSC11_QUERY);
    }
    remaining
}

fn find_partial_osc11_start(data: &str) -> Option<usize> {
    if let Some(index) = data.find(OSC11_RESPONSE_PREFIX) {
        return Some(index);
    }
    if let Some(index) = data.find(OSC11_RESPONSE_PREFIX_NO_ESC) {
        return Some(index);
    }

    for (index, _) in data.char_indices() {
        let suffix = &data[index..];
        if OSC11_RESPONSE_PREFIX.starts_with(suffix) && suffix.encode_utf16().count() > 1 {
            return Some(index);
        }
        if OSC11_RESPONSE_PREFIX_NO_ESC.starts_with(suffix) && suffix.starts_with("]11;") {
            return Some(index);
        }
    }
    None
}

fn result_from_remaining(data: String) -> TerminalInputResult {
    if data.is_empty() {
        TerminalInputResult::Consume
    } else {
        TerminalInputResult::Data(data)
    }
}

pub struct TerminalThemeTracking<W: TerminalWrite> {
    terminal: Rc<RefCell<W>>,
    dispose_input_listener: Option<Box<dyn FnOnce()>>,
}

impl<W: TerminalWrite> TerminalThemeTracking<W> {
    pub fn dispose(mut self) {
        if let Some(dispose) = self.dispose_input_listener.take() {
            dispose();
        }
        self.terminal
            .borrow_mut()
            .write(DISABLE_TERMINAL_THEME_REPORTING);
    }
}

/// Original:
///   apps/kimi-code/src/tui/utils/terminal-theme.ts
///   installTerminalThemeTracking()
pub fn install_terminal_theme_tracking<W: TerminalWrite + 'static>(
    terminal: Rc<RefCell<W>>,
    ui: &mut dyn InputListenerRegistry,
    mut on_theme: impl FnMut(ResolvedTheme) + 'static,
) -> TerminalThemeTracking<W> {
    let input_state = Rc::new(RefCell::new(create_terminal_theme_input_state()));
    let listener_terminal = Rc::clone(&terminal);
    let listener_state = Rc::clone(&input_state);
    let dispose_input_listener = ui.add_input_listener(Box::new(move |data| {
        handle_terminal_theme_input(
            data,
            &mut *listener_terminal.borrow_mut(),
            &mut on_theme,
            &mut listener_state.borrow_mut(),
        )
    }));
    {
        let mut terminal = terminal.borrow_mut();
        terminal.write(ENABLE_TERMINAL_THEME_REPORTING);
        terminal.write(OSC11_QUERY);
        terminal.write(QUERY_TERMINAL_THEME);
    }

    TerminalThemeTracking {
        terminal,
        dispose_input_listener: Some(dispose_input_listener),
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, collections::VecDeque};

    use super::*;
    use crate::tui::utils::terminal_focus::TerminalInputListener;

    const DARK_OSC11_REPORT: &str = "\u{1b}]11;rgb:2828/2c2c/3434\u{7}";
    const LIGHT_OSC11_REPORT: &str = "\u{1b}]11;rgb:fafa/fbfb/fcfc\u{7}";

    #[derive(Default)]
    struct RecordingTerminal(Vec<String>);

    impl TerminalWrite for RecordingTerminal {
        fn write(&mut self, data: &str) {
            self.0.push(data.to_owned());
        }
    }

    #[derive(Default)]
    struct RecordingUi {
        listeners: VecDeque<TerminalInputListener>,
        dispose_count: Rc<Cell<usize>>,
    }

    impl InputListenerRegistry for RecordingUi {
        fn add_input_listener(&mut self, listener: TerminalInputListener) -> Box<dyn FnOnce()> {
            self.listeners.push_back(listener);
            let dispose_count = Rc::clone(&self.dispose_count);
            Box::new(move || dispose_count.set(dispose_count.get() + 1))
        }
    }

    #[test]
    fn recognizes_and_strips_private_theme_reports() {
        assert!(has_terminal_theme_report(TERMINAL_THEME_DARK));
        assert!(has_terminal_theme_report(&format!(
            "x{TERMINAL_THEME_LIGHT}y"
        )));
        assert!(!has_terminal_theme_report("x"));

        let mut terminal = RecordingTerminal::default();
        let mut themes = Vec::new();
        let mut state = create_terminal_theme_input_state();
        assert_eq!(
            handle_terminal_theme_input(
                &format!("a{TERMINAL_THEME_LIGHT}b"),
                &mut terminal,
                &mut |theme| themes.push(theme),
                &mut state,
            ),
            Some(TerminalInputResult::Data("ab".to_owned()))
        );
        assert_eq!(terminal.0, [OSC11_QUERY]);
        assert!(themes.is_empty());
    }

    #[test]
    fn consumes_complete_osc11_reports_and_keeps_coalesced_input() {
        let mut terminal = RecordingTerminal::default();
        let mut themes = Vec::new();
        let mut state = create_terminal_theme_input_state();

        assert_eq!(
            handle_terminal_theme_input(
                &format!("a{DARK_OSC11_REPORT}{LIGHT_OSC11_REPORT}b"),
                &mut terminal,
                &mut |theme| themes.push(theme),
                &mut state,
            ),
            Some(TerminalInputResult::Data("ab".to_owned()))
        );
        assert_eq!(themes, [ResolvedTheme::Dark, ResolvedTheme::Light]);
        assert!(terminal.0.is_empty());
    }

    #[test]
    fn accumulates_fragmented_reports_and_forwards_trailing_input() {
        let mut terminal = RecordingTerminal::default();
        let mut themes = Vec::new();
        let mut state = create_terminal_theme_input_state();

        assert_eq!(
            handle_terminal_theme_input(
                "\u{1b}]11;rgb:2828/2c2c/3",
                &mut terminal,
                &mut |theme| themes.push(theme),
                &mut state,
            ),
            Some(TerminalInputResult::Consume)
        );
        assert_eq!(
            handle_terminal_theme_input(
                "434\u{7}x",
                &mut terminal,
                &mut |theme| themes.push(theme),
                &mut state,
            ),
            Some(TerminalInputResult::Data("x".to_owned()))
        );
        assert_eq!(themes, [ResolvedTheme::Dark]);
        assert!(state.osc11_buffer.is_empty());
    }

    #[test]
    fn ignores_unrelated_short_prefixes_and_bounds_partial_buffer() {
        let mut terminal = RecordingTerminal::default();
        let mut state = create_terminal_theme_input_state();
        let mut on_theme = |_| {};
        assert_eq!(
            handle_terminal_theme_input("]", &mut terminal, &mut on_theme, &mut state),
            None
        );
        assert_eq!(
            handle_terminal_theme_input("\u{1b}]11;rgb:", &mut terminal, &mut on_theme, &mut state,),
            Some(TerminalInputResult::Consume)
        );
        assert_eq!(
            handle_terminal_theme_input(&"x".repeat(600), &mut terminal, &mut on_theme, &mut state,),
            Some(TerminalInputResult::Consume)
        );
        assert!(state.osc11_buffer.is_empty());
    }

    #[test]
    fn installs_queries_and_disposes_tracking() {
        let terminal = Rc::new(RefCell::new(RecordingTerminal::default()));
        let mut ui = RecordingUi::default();
        let dispose_count = Rc::clone(&ui.dispose_count);
        let themes = Rc::new(RefCell::new(Vec::new()));
        let listener_themes = Rc::clone(&themes);

        let tracking =
            install_terminal_theme_tracking(Rc::clone(&terminal), &mut ui, move |theme| {
                listener_themes.borrow_mut().push(theme)
            });
        assert_eq!(
            terminal.borrow().0,
            [
                ENABLE_TERMINAL_THEME_REPORTING,
                OSC11_QUERY,
                QUERY_TERMINAL_THEME
            ]
        );
        let mut listener = ui.listeners.pop_front();
        assert_eq!(
            listener
                .as_mut()
                .and_then(|listener| listener(DARK_OSC11_REPORT)),
            Some(TerminalInputResult::Consume)
        );
        assert_eq!(*themes.borrow(), [ResolvedTheme::Dark]);

        tracking.dispose();
        assert_eq!(dispose_count.get(), 1);
        assert_eq!(
            terminal.borrow().0.last().map(String::as_str),
            Some(DISABLE_TERMINAL_THEME_REPORTING)
        );
    }

    #[test]
    fn constants_match_terminal_protocol_bytes() {
        assert_eq!(OSC11_QUERY, format!("{ESC}]11;?{BEL}"));
    }
}
