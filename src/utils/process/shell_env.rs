use std::collections::HashMap;

use crate::sdk::types::ShellEnvironment;

// Original:
//   apps/kimi-code/src/utils/process/shell-env.ts
//   detectShellEnvironment()
pub fn detect_shell_environment() -> ShellEnvironment {
    detect_shell_environment_from(&std::env::vars().collect())
}

pub fn detect_shell_environment_from(environment: &HashMap<String, String>) -> ShellEnvironment {
    ShellEnvironment {
        term: nonempty(environment, "TERM"),
        term_program: nonempty(environment, "TERM_PROGRAM"),
        term_program_version: nonempty(environment, "TERM_PROGRAM_VERSION"),
        multiplexer: detect_multiplexer(environment),
        shell: nonempty(environment, "SHELL"),
    }
}

fn detect_multiplexer(environment: &HashMap<String, String>) -> Option<String> {
    if nonempty(environment, "TMUX").is_some() {
        Some("tmux".to_owned())
    } else if nonempty(environment, "STY").is_some() {
        Some("screen".to_owned())
    } else if nonempty(environment, "ZELLIJ").is_some() {
        Some("zellij".to_owned())
    } else {
        None
    }
}

fn nonempty(environment: &HashMap<String, String>, name: &str) -> Option<String> {
    environment
        .get(name)
        .filter(|value| !value.is_empty())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_terminal_and_shell_fields_without_trimming_values() {
        let environment = HashMap::from([
            ("TERM".to_owned(), "xterm-256color".to_owned()),
            ("TERM_PROGRAM".to_owned(), "WezTerm".to_owned()),
            ("TERM_PROGRAM_VERSION".to_owned(), "2026.1".to_owned()),
            ("SHELL".to_owned(), " /bin/zsh ".to_owned()),
        ]);
        assert_eq!(
            detect_shell_environment_from(&environment),
            ShellEnvironment {
                term: Some("xterm-256color".to_owned()),
                term_program: Some("WezTerm".to_owned()),
                term_program_version: Some("2026.1".to_owned()),
                multiplexer: None,
                shell: Some(" /bin/zsh ".to_owned()),
            }
        );
    }

    #[test]
    fn multiplexer_priority_is_tmux_then_screen_then_zellij() {
        let environment = HashMap::from([
            ("TMUX".to_owned(), "tmux-socket".to_owned()),
            ("STY".to_owned(), "screen-session".to_owned()),
            ("ZELLIJ".to_owned(), "1".to_owned()),
        ]);
        assert_eq!(
            detect_shell_environment_from(&environment)
                .multiplexer
                .as_deref(),
            Some("tmux")
        );
        let environment = HashMap::from([
            ("TMUX".to_owned(), String::new()),
            ("STY".to_owned(), "screen-session".to_owned()),
            ("ZELLIJ".to_owned(), "1".to_owned()),
        ]);
        assert_eq!(
            detect_shell_environment_from(&environment)
                .multiplexer
                .as_deref(),
            Some("screen")
        );
    }

    #[test]
    fn absent_and_empty_values_are_omitted() {
        let environment = HashMap::from([
            ("TERM".to_owned(), String::new()),
            ("ZELLIJ".to_owned(), String::new()),
        ]);
        assert_eq!(
            detect_shell_environment_from(&environment),
            ShellEnvironment::default()
        );
    }
}
