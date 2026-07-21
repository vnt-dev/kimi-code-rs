use std::collections::HashSet;

const BEL: &str = "\u{7}";
const ESC: &str = "\u{1b}";
const ST: &str = "\\";
const MAX_TERMINAL_NOTIFICATION_MESSAGE_LENGTH: usize = 240;

pub trait Environment {
    fn value(&self, name: &str) -> Option<String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnvironment;

impl Environment for ProcessEnvironment {
    fn value(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

impl Environment for std::collections::HashMap<String, String> {
    fn value(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

pub trait TerminalWrite {
    fn write(&mut self, data: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalNotification {
    pub title: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildOptions {
    pub supports_osc9: bool,
    pub inside_tmux: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationCondition {
    Unfocused,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationSettings {
    pub enabled: bool,
    pub condition: NotificationCondition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalState {
    pub notification_keys: HashSet<String>,
    pub focused: bool,
    pub supports_osc9: bool,
    pub supports_progress: bool,
    pub inside_tmux: bool,
    pub progress_active: bool,
}

/// Original:
///   apps/kimi-code/src/tui/utils/terminal-state.ts
///   createTerminalState()
pub fn create_terminal_state(env: &dyn Environment) -> TerminalState {
    TerminalState {
        notification_keys: HashSet::new(),
        focused: true,
        supports_osc9: supports_osc9_notification(env),
        supports_progress: supports_terminal_progress(env),
        inside_tmux: is_inside_tmux(env),
        progress_active: false,
    }
}

/// Original:
///   apps/kimi-code/src/tui/utils/terminal-notification.ts
///   notifyTerminalOnce()
pub fn notify_terminal_once(
    terminal: &mut dyn TerminalWrite,
    settings: NotificationSettings,
    state: &mut TerminalState,
    key: &str,
    notification: &TerminalNotification,
) {
    if !settings.enabled || state.notification_keys.contains(key) {
        return;
    }
    state.notification_keys.insert(key.to_owned());
    if settings.condition == NotificationCondition::Unfocused && state.focused {
        return;
    }
    emit_terminal_notification(
        terminal,
        notification,
        BuildOptions {
            supports_osc9: state.supports_osc9,
            inside_tmux: state.inside_tmux,
        },
    );
}

pub fn emit_terminal_notification(
    terminal: &mut dyn TerminalWrite,
    notification: &TerminalNotification,
    options: BuildOptions,
) {
    for sequence in build_terminal_notification_sequences(notification, options) {
        terminal.write(&sequence);
    }
}

pub fn format_notification(notification: &TerminalNotification) -> String {
    let title = sanitize_notification_text(&notification.title);
    let body = sanitize_notification_text(notification.body.as_deref().unwrap_or_default());
    let message = match (title.is_empty(), body.is_empty()) {
        (false, false) => format!("{title}: {body}"),
        (false, true) => title,
        (true, false) => body,
        (true, true) => String::new(),
    };
    truncate_utf16(&message, MAX_TERMINAL_NOTIFICATION_MESSAGE_LENGTH)
}

/// Original:
///   apps/kimi-code/src/tui/utils/terminal-notification.ts
///   buildTerminalNotificationSequences()
pub fn build_terminal_notification_sequences(
    notification: &TerminalNotification,
    options: BuildOptions,
) -> Vec<String> {
    let message = format_notification(notification);
    if message.is_empty() {
        return Vec::new();
    }
    if !options.supports_osc9 {
        return vec![BEL.to_owned()];
    }
    let osc9 = format!("{ESC}]9;{message}{BEL}");
    if options.inside_tmux {
        let escaped = osc9.replace(ESC, &format!("{ESC}{ESC}"));
        return vec![format!("{ESC}Ptmux;{escaped}{ESC}{ST}")];
    }
    vec![osc9]
}

pub fn supports_osc9_notification(env: &dyn Environment) -> bool {
    let term_program = env.value("TERM_PROGRAM").unwrap_or_default();
    if matches!(
        term_program.as_str(),
        "iTerm.app" | "WezTerm" | "ghostty" | "WarpTerminal"
    ) {
        return true;
    }
    matches!(
        env.value("TERM").unwrap_or_default().as_str(),
        "xterm-kitty" | "xterm-ghostty"
    )
}

pub fn supports_terminal_progress(env: &dyn Environment) -> bool {
    if env
        .value("WT_SESSION")
        .is_some_and(|value| !value.is_empty())
    {
        return true;
    }
    if env.value("ConEmuANSI").as_deref() == Some("ON") {
        return true;
    }
    if matches!(
        env.value("TERM_PROGRAM").unwrap_or_default().as_str(),
        "ghostty" | "WezTerm"
    ) {
        return true;
    }
    env.value("TERM").as_deref() == Some("xterm-ghostty")
}

pub fn is_inside_tmux(env: &dyn Environment) -> bool {
    env.value("TMUX").is_some_and(|value| !value.is_empty())
}

fn sanitize_notification_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if is_control_character(character) {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_control_character(character: char) -> bool {
    let code = u32::from(character);
    (0x00..=0x1f).contains(&code) || (0x7f..=0x9f).contains(&code)
}

fn truncate_utf16(value: &str, maximum: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > maximum {
                return false;
            }
            units = next;
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct RecordingTerminal(Vec<String>);

    impl TerminalWrite for RecordingTerminal {
        fn write(&mut self, data: &str) {
            self.0.push(data.to_owned());
        }
    }

    fn notification() -> TerminalNotification {
        TerminalNotification {
            title: "Kimi Code".to_owned(),
            body: Some("Approval\nrequired".to_owned()),
        }
    }

    fn environment(values: &[(&str, &str)]) -> HashMap<String, String> {
        values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn builds_osc9_bel_and_tmux_sequences() {
        assert_eq!(
            build_terminal_notification_sequences(
                &notification(),
                BuildOptions {
                    supports_osc9: true,
                    inside_tmux: false,
                }
            ),
            [format!("{ESC}]9;Kimi Code: Approval required{BEL}")]
        );
        assert_eq!(
            build_terminal_notification_sequences(
                &notification(),
                BuildOptions {
                    supports_osc9: false,
                    inside_tmux: true,
                }
            ),
            [BEL]
        );
        assert_eq!(
            build_terminal_notification_sequences(
                &notification(),
                BuildOptions {
                    supports_osc9: true,
                    inside_tmux: true,
                }
            ),
            [format!(
                "{ESC}Ptmux;{ESC}{ESC}]9;Kimi Code: Approval required{BEL}{ESC}\\"
            )]
        );
    }

    #[test]
    fn sanitizes_falls_back_to_body_and_bounds_message() {
        assert_eq!(
            format_notification(&TerminalNotification {
                title: "".to_owned(),
                body: Some(" Question? ".to_owned()),
            }),
            "Question?"
        );
        let formatted = format_notification(&TerminalNotification {
            title: format!("{}\u{7}\n", "x".repeat(250)),
            body: None,
        });
        assert_eq!(formatted.encode_utf16().count(), 240);
        assert!(!formatted.contains('\u{7}'));
    }

    #[test]
    fn emits_nothing_for_an_empty_message() {
        let mut terminal = RecordingTerminal::default();
        emit_terminal_notification(
            &mut terminal,
            &TerminalNotification {
                title: String::new(),
                body: Some("\n\t".to_owned()),
            },
            BuildOptions {
                supports_osc9: true,
                inside_tmux: false,
            },
        );
        assert!(terminal.0.is_empty());
    }

    #[test]
    fn deduplicates_and_preserves_suppressed_keys() {
        let mut terminal = RecordingTerminal::default();
        let mut state = TerminalState {
            notification_keys: HashSet::new(),
            focused: true,
            supports_osc9: true,
            supports_progress: false,
            inside_tmux: false,
            progress_active: false,
        };
        let settings = NotificationSettings {
            enabled: true,
            condition: NotificationCondition::Unfocused,
        };

        notify_terminal_once(
            &mut terminal,
            settings,
            &mut state,
            "approval:1",
            &notification(),
        );
        state.focused = false;
        notify_terminal_once(
            &mut terminal,
            settings,
            &mut state,
            "approval:1",
            &notification(),
        );
        notify_terminal_once(
            &mut terminal,
            settings,
            &mut state,
            "approval:2",
            &notification(),
        );

        assert_eq!(terminal.0.len(), 1);
        assert!(state.notification_keys.contains("approval:1"));
    }

    #[test]
    fn disabled_notifications_do_not_consume_the_key_and_always_ignores_focus() {
        let mut terminal = RecordingTerminal::default();
        let mut state = create_terminal_state(&environment(&[("TERM", "xterm-kitty")]));
        notify_terminal_once(
            &mut terminal,
            NotificationSettings {
                enabled: false,
                condition: NotificationCondition::Always,
            },
            &mut state,
            "approval:1",
            &notification(),
        );
        assert!(!state.notification_keys.contains("approval:1"));

        notify_terminal_once(
            &mut terminal,
            NotificationSettings {
                enabled: true,
                condition: NotificationCondition::Always,
            },
            &mut state,
            "approval:1",
            &notification(),
        );
        assert_eq!(terminal.0.len(), 1);
    }

    #[test]
    fn detects_osc9_capabilities_conservatively() {
        for values in [
            vec![("TERM_PROGRAM", "iTerm.app")],
            vec![("TERM_PROGRAM", "WezTerm")],
            vec![("TERM_PROGRAM", "ghostty")],
            vec![("TERM_PROGRAM", "WarpTerminal")],
            vec![("TERM", "xterm-kitty")],
            vec![("TERM", "xterm-ghostty")],
        ] {
            assert!(supports_osc9_notification(&environment(&values)));
        }
        for values in [
            vec![("TERM_PROGRAM", "Apple_Terminal")],
            vec![("WT_SESSION", "abc")],
            vec![("ConEmuANSI", "ON")],
            vec![("TERM", "xterm-256color")],
        ] {
            assert!(!supports_osc9_notification(&environment(&values)));
        }
    }

    #[test]
    fn detects_progress_without_conflating_osc9_notifications() {
        for values in [
            vec![("WT_SESSION", "abc")],
            vec![("ConEmuANSI", "ON")],
            vec![("TERM_PROGRAM", "ghostty")],
            vec![("TERM_PROGRAM", "WezTerm")],
            vec![("TERM", "xterm-ghostty")],
        ] {
            assert!(supports_terminal_progress(&environment(&values)));
        }
        for values in [
            vec![("TERM_PROGRAM", "iTerm.app")],
            vec![("TERM_PROGRAM", "WarpTerminal")],
            vec![("TERM", "xterm-kitty")],
            vec![("ConEmuANSI", "OFF")],
            vec![("WT_SESSION", "")],
        ] {
            assert!(!supports_terminal_progress(&environment(&values)));
        }
    }

    #[test]
    fn detects_tmux_and_builds_initial_terminal_state() {
        let state = create_terminal_state(&environment(&[
            ("TMUX", "/tmp/tmux/default,1,0"),
            ("TERM_PROGRAM", "WezTerm"),
        ]));

        assert!(state.focused);
        assert!(state.inside_tmux);
        assert!(state.supports_osc9);
        assert!(state.supports_progress);
        assert!(!state.progress_active);
        assert!(state.notification_keys.is_empty());
    }
}
