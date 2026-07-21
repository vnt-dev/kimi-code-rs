use std::{fmt, io::IsTerminal};

pub const STARTUP_ERROR_COLOR: &str = "#E85454";
const STARTUP_ERROR_RED: u8 = 0xE8;
const STARTUP_ERROR_GREEN: u8 = 0x54;
const STARTUP_ERROR_BLUE: u8 = 0x54;

/// The two branches accepted by the original `isKimiError` check.
#[derive(Clone, Copy)]
pub enum StartupFailure<'a> {
    Kimi { title: &'a str, message: &'a str },
    Other(&'a dyn fmt::Display),
}

pub type ErrorStyle<'a> = &'a dyn Fn(&str) -> String;

#[derive(Default)]
pub struct StartupErrorFormatOptions<'a> {
    pub error_style: Option<ErrorStyle<'a>>,
    pub operation: Option<&'a str>,
}

// Original:
//   apps/kimi-code/src/cli/startup-error.ts
//   formatStartupError()
//
// Rust adaptation:
//   The SDK error registry is represented at this boundary by the title and
//   message that the original formatter reads from it. The SDK migration can
//   construct this enum without coupling terminal formatting to the registry.
pub fn format_startup_error(
    error: StartupFailure<'_>,
    options: &StartupErrorFormatOptions<'_>,
) -> String {
    let apply_style = |text: &str| match options.error_style {
        Some(style) => style(text),
        None => default_error_style(text),
    };

    match error {
        StartupFailure::Other(error) => {
            let operation = options.operation.unwrap_or("start shell");
            apply_style(&format!("error: failed to {operation}: {error}")) + "\n"
        }
        StartupFailure::Kimi { title, message } => {
            let lines = [
                apply_style(&format!("error: {title}")),
                String::new(),
                apply_style("message:"),
                apply_style(message),
            ];
            lines.join("\n") + "\n"
        }
    }
}

fn default_error_style(text: &str) -> String {
    if std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        format!(
            "\u{1b}[38;2;{STARTUP_ERROR_RED};{STARTUP_ERROR_GREEN};{STARTUP_ERROR_BLUE}m{text}\u{1b}[39m"
        )
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red(text: &str) -> String {
        format!("\u{1b}[31m{text}\u{1b}[39m")
    }

    fn identity(text: &str) -> String {
        text.to_owned()
    }

    #[test]
    fn formats_kimi_errors_with_structured_fields() {
        let message = "Git Bash was not found on this Windows host. Checked: C:\\Program Files\\Git\\bin\\bash.exe.";
        assert_eq!(
            format_startup_error(
                StartupFailure::Kimi {
                    title: "Git Bash not found",
                    message,
                },
                &StartupErrorFormatOptions {
                    error_style: Some(&red),
                    operation: None,
                },
            ),
            [
                "\u{1b}[31merror: Git Bash not found\u{1b}[39m",
                "",
                "\u{1b}[31mmessage:\u{1b}[39m",
                "\u{1b}[31mGit Bash was not found on this Windows host. Checked: C:\\Program Files\\Git\\bin\\bash.exe.\u{1b}[39m",
                "",
            ]
            .join("\n")
        );
    }

    #[test]
    fn keeps_generic_errors_on_the_legacy_shell_path() {
        let error = "Provider not set";
        assert_eq!(
            format_startup_error(
                StartupFailure::Other(&error),
                &StartupErrorFormatOptions {
                    error_style: Some(&identity),
                    operation: None,
                },
            ),
            "error: failed to start shell: Provider not set\n"
        );
    }

    #[test]
    fn names_the_requested_generic_operation() {
        let error = "Provider not set";
        assert_eq!(
            format_startup_error(
                StartupFailure::Other(&error),
                &StartupErrorFormatOptions {
                    error_style: Some(&identity),
                    operation: Some("run prompt"),
                },
            ),
            "error: failed to run prompt: Provider not set\n"
        );
    }
}
