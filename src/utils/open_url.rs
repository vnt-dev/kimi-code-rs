use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenUrlPlatform {
    MacOs,
    Windows,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenUrlCommand {
    pub program: String,
    pub arguments: Vec<String>,
}

impl OpenUrlPlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

// Original:
//   apps/kimi-code/src/utils/open-url.ts
//   openUrl()
pub fn open_url(url: &str) {
    let command = open_url_command(OpenUrlPlatform::current(), url);
    // The JavaScript implementation ignores execFile's callback error. Match
    // that best-effort contract, detach the child, and avoid buffered pipes
    // keeping it alive when the parent does not collect output.
    let _ = Command::new(command.program)
        .args(command.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub fn open_url_command(platform: OpenUrlPlatform, url: &str) -> OpenUrlCommand {
    match platform {
        OpenUrlPlatform::MacOs => OpenUrlCommand {
            program: "open".to_owned(),
            arguments: vec![url.to_owned()],
        },
        OpenUrlPlatform::Windows => OpenUrlCommand {
            program: "cmd".to_owned(),
            arguments: vec![
                "/c".to_owned(),
                "start".to_owned(),
                String::new(),
                url.to_owned(),
            ],
        },
        OpenUrlPlatform::Other => OpenUrlCommand {
            program: "xdg-open".to_owned(),
            arguments: vec![url.to_owned()],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_macos_open_command() {
        assert_eq!(
            open_url_command(OpenUrlPlatform::MacOs, "https://example.com/a?b=1"),
            OpenUrlCommand {
                program: "open".to_owned(),
                arguments: vec!["https://example.com/a?b=1".to_owned()],
            }
        );
    }

    #[test]
    fn builds_the_windows_cmd_start_command_with_an_empty_title() {
        assert_eq!(
            open_url_command(OpenUrlPlatform::Windows, "https://example.com/a b"),
            OpenUrlCommand {
                program: "cmd".to_owned(),
                arguments: vec![
                    "/c".to_owned(),
                    "start".to_owned(),
                    String::new(),
                    "https://example.com/a b".to_owned(),
                ],
            }
        );
    }

    #[test]
    fn builds_the_freedesktop_open_command_for_other_platforms() {
        assert_eq!(
            open_url_command(OpenUrlPlatform::Other, "https://example.com"),
            OpenUrlCommand {
                program: "xdg-open".to_owned(),
                arguments: vec!["https://example.com".to_owned()],
            }
        );
    }

    #[test]
    fn current_platform_matches_the_compilation_target() {
        let expected = if cfg!(target_os = "macos") {
            OpenUrlPlatform::MacOs
        } else if cfg!(target_os = "windows") {
            OpenUrlPlatform::Windows
        } else {
            OpenUrlPlatform::Other
        };
        assert_eq!(OpenUrlPlatform::current(), expected);
    }
}
