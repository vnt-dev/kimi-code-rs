use std::{cell::Cell, rc::Rc};

use super::terminal_notification::TerminalWrite;

pub const TERMINAL_FOCUS_IN: &str = "\u{1b}[I";
pub const TERMINAL_FOCUS_OUT: &str = "\u{1b}[O";
pub const ENABLE_TERMINAL_FOCUS_REPORTING: &str = "\u{1b}[?1004h";
pub const DISABLE_TERMINAL_FOCUS_REPORTING: &str = "\u{1b}[?1004l";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInputResult {
    Consume,
    Data(String),
}

pub type TerminalInputListener = Box<dyn FnMut(&str) -> Option<TerminalInputResult>>;

pub trait InputListenerRegistry {
    fn add_input_listener(&mut self, listener: TerminalInputListener) -> Box<dyn FnOnce()>;
}

pub struct TerminalFocusTracking {
    focused: Rc<Cell<bool>>,
    dispose_input_listener: Option<Box<dyn FnOnce()>>,
}

impl TerminalFocusTracking {
    /// Completes the explicit cleanup sequence used by the original returned
    /// disposer. Cleanup needs a terminal because disabling focus reporting is
    /// an externally visible protocol write and must not be deferred to Drop.
    pub fn dispose(mut self, terminal: &mut dyn TerminalWrite) {
        if let Some(dispose) = self.dispose_input_listener.take() {
            dispose();
        }
        terminal.write(DISABLE_TERMINAL_FOCUS_REPORTING);
        self.focused.set(true);
    }
}

/// Original:
///   apps/kimi-code/src/tui/utils/terminal-focus.ts
///   installTerminalFocusTracking()
pub fn install_terminal_focus_tracking(
    focused: Rc<Cell<bool>>,
    terminal: &mut dyn TerminalWrite,
    ui: &mut dyn InputListenerRegistry,
) -> TerminalFocusTracking {
    focused.set(true);
    let listener_state = Rc::clone(&focused);
    let dispose_input_listener = ui.add_input_listener(Box::new(move |data| {
        handle_terminal_focus_input(&listener_state, data)
    }));
    terminal.write(ENABLE_TERMINAL_FOCUS_REPORTING);

    TerminalFocusTracking {
        focused,
        dispose_input_listener: Some(dispose_input_listener),
    }
}

/// Original:
///   apps/kimi-code/src/tui/utils/terminal-focus.ts
///   handleTerminalFocusInput()
pub fn handle_terminal_focus_input(
    focused: &Cell<bool>,
    data: &str,
) -> Option<TerminalInputResult> {
    match data {
        TERMINAL_FOCUS_IN => {
            focused.set(true);
            Some(TerminalInputResult::Consume)
        }
        TERMINAL_FOCUS_OUT => {
            focused.set(false);
            Some(TerminalInputResult::Consume)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    #[derive(Default)]
    struct RecordingTerminal(Vec<String>);

    impl TerminalWrite for RecordingTerminal {
        fn write(&mut self, data: &str) {
            self.0.push(data.to_owned());
        }
    }

    #[derive(Default)]
    struct RecordingUi {
        listener: Rc<RefCell<Option<TerminalInputListener>>>,
        dispose_count: Rc<Cell<usize>>,
    }

    impl InputListenerRegistry for RecordingUi {
        fn add_input_listener(&mut self, listener: TerminalInputListener) -> Box<dyn FnOnce()> {
            *self.listener.borrow_mut() = Some(listener);
            let dispose_count = Rc::clone(&self.dispose_count);
            Box::new(move || dispose_count.set(dispose_count.get() + 1))
        }
    }

    #[test]
    fn updates_focus_only_for_exact_reporting_sequences() {
        let focused = Cell::new(true);

        assert_eq!(
            handle_terminal_focus_input(&focused, TERMINAL_FOCUS_OUT),
            Some(TerminalInputResult::Consume)
        );
        assert!(!focused.get());
        assert_eq!(
            handle_terminal_focus_input(&focused, TERMINAL_FOCUS_IN),
            Some(TerminalInputResult::Consume)
        );
        assert!(focused.get());
        assert_eq!(handle_terminal_focus_input(&focused, "x"), None);
        assert_eq!(handle_terminal_focus_input(&focused, "\u{1b}[Ox"), None);
    }

    #[test]
    fn installs_listener_and_performs_explicit_cleanup() {
        let focused = Rc::new(Cell::new(false));
        let mut terminal = RecordingTerminal::default();
        let mut ui = RecordingUi::default();
        let listener = Rc::clone(&ui.listener);
        let dispose_count = Rc::clone(&ui.dispose_count);

        let tracking = install_terminal_focus_tracking(Rc::clone(&focused), &mut terminal, &mut ui);
        assert!(focused.get());
        assert_eq!(terminal.0, [ENABLE_TERMINAL_FOCUS_REPORTING]);

        let result = listener
            .borrow_mut()
            .as_mut()
            .and_then(|listener| listener(TERMINAL_FOCUS_OUT));
        assert_eq!(result, Some(TerminalInputResult::Consume));
        assert!(!focused.get());

        tracking.dispose(&mut terminal);
        assert_eq!(dispose_count.get(), 1);
        assert!(focused.get());
        assert_eq!(
            terminal.0,
            [
                ENABLE_TERMINAL_FOCUS_REPORTING,
                DISABLE_TERMINAL_FOCUS_REPORTING
            ]
        );
    }
}
