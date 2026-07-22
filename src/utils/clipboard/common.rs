use std::{collections::HashMap, process::Stdio, time::Duration};

use tokio::{io::AsyncReadExt, process::Command};

pub const SUPPORTED_IMAGE_MIME_TYPES: &[&str] =
    &["image/png", "image/jpeg", "image/webp", "image/gif"];
pub const DEFAULT_LIST_TIMEOUT: Duration = Duration::from_secs(1);
pub const DEFAULT_MAX_BUFFER_BYTES: usize = 50 * 1_024 * 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub ok: bool,
}

impl CommandOutput {
    fn failed() -> Self {
        Self {
            stdout: Vec::new(),
            ok: false,
        }
    }
}

pub fn base_mime_type(raw: &str) -> String {
    raw.split(';').next().unwrap_or(raw).trim().to_lowercase()
}

pub fn is_supported_image_mime_type(mime: &str) -> bool {
    let base = base_mime_type(mime);
    SUPPORTED_IMAGE_MIME_TYPES.contains(&base.as_str())
}

pub fn parse_target_list(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Runs a clipboard helper without blocking the Tokio worker thread.
///
/// Original:
///   apps/kimi-code/src/utils/clipboard/clipboard-common.ts
///   runCommandAsync()
pub async fn run_command_async(
    command: &str,
    args: &[String],
    timeout: Option<Duration>,
    environment: Option<&HashMap<String, String>>,
) -> CommandOutput {
    let mut process = Command::new(command);
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(environment) = environment {
        process.env_clear().envs(environment);
    }
    let Ok(mut child) = process.spawn() else {
        return CommandOutput::failed();
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill().await;
        return CommandOutput::failed();
    };
    let operation = async {
        let mut limited = stdout.take((DEFAULT_MAX_BUFFER_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        let (read, status) = tokio::join!(limited.read_to_end(&mut bytes), child.wait());
        let read = read.ok()?;
        let status = status.ok()?;
        (read <= DEFAULT_MAX_BUFFER_BYTES && status.success()).then_some(bytes)
    };
    match tokio::time::timeout(timeout.unwrap_or(DEFAULT_LIST_TIMEOUT), operation).await {
        Ok(Some(stdout)) => CommandOutput { stdout, ok: true },
        Ok(None) => CommandOutput::failed(),
        Err(_) => {
            let _ = child.kill().await;
            CommandOutput::failed()
        }
    }
}

pub fn is_wayland_session(environment: &HashMap<String, String>) -> bool {
    environment
        .get("WAYLAND_DISPLAY")
        .is_some_and(|value| !value.is_empty())
        || environment.get("XDG_SESSION_TYPE").map(String::as_str) == Some("wayland")
}

pub fn is_wsl(environment: &HashMap<String, String>, proc_version: Option<&str>) -> bool {
    environment.contains_key("WSL_DISTRO_NAME")
        || environment.contains_key("WSLENV")
        || proc_version.is_some_and(|version| {
            let lower = version.to_lowercase();
            lower.contains("microsoft") || lower.contains("wsl")
        })
}

pub fn detect_wsl(environment: &HashMap<String, String>) -> bool {
    let proc_version = std::fs::read_to_string("/proc/version").ok();
    is_wsl(environment, proc_version.as_deref())
}

pub fn is_file_like_native_format(format: &str) -> bool {
    let lower = format.to_lowercase();
    let base = base_mime_type(format);
    lower.contains("file-url")
        || lower.contains("file url")
        || lower.contains("nsfilenames")
        || lower.contains("com.apple.finder")
        || base == "text/uri-list"
        || base == "public.url"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_mime_targets_and_file_like_formats() {
        assert_eq!(base_mime_type(" Image/PNG ; charset=binary"), "image/png");
        assert!(is_supported_image_mime_type("image/jpeg; q=1"));
        assert!(!is_supported_image_mime_type("image/bmp"));
        assert_eq!(
            parse_target_list(b" image/png \r\n\ntext/uri-list\n"),
            ["image/png", "text/uri-list"]
        );
        assert!(is_file_like_native_format("public.file-url"));
        assert!(is_file_like_native_format("text/uri-list; charset=utf-8"));
        assert!(!is_file_like_native_format("image/png"));
    }

    #[test]
    fn detects_wayland_and_wsl_from_the_same_sources_as_node() {
        assert!(is_wayland_session(&HashMap::from([(
            "WAYLAND_DISPLAY".to_owned(),
            "wayland-0".to_owned()
        )])));
        assert!(is_wayland_session(&HashMap::from([(
            "XDG_SESSION_TYPE".to_owned(),
            "wayland".to_owned()
        )])));
        assert!(is_wsl(&HashMap::new(), Some("Linux microsoft-standard")));
        assert!(is_wsl(
            &HashMap::from([("WSLENV".to_owned(), String::new())]),
            None
        ));
        assert!(!is_wsl(&HashMap::new(), Some("Linux generic")));
    }

    #[tokio::test]
    async fn command_runner_captures_success_and_converts_failures_to_empty_output() {
        let (command, success_args, failure_args) = if cfg!(windows) {
            (
                "cmd.exe",
                vec!["/D".to_owned(), "/C".to_owned(), "echo ok".to_owned()],
                vec!["/D".to_owned(), "/C".to_owned(), "exit 7".to_owned()],
            )
        } else {
            (
                "sh",
                vec!["-c".to_owned(), "printf ok".to_owned()],
                vec!["-c".to_owned(), "exit 7".to_owned()],
            )
        };
        let success = run_command_async(command, &success_args, None, None).await;
        assert!(success.ok);
        assert_eq!(String::from_utf8_lossy(&success.stdout).trim(), "ok");
        let failure = run_command_async(command, &failure_args, None, None).await;
        assert_eq!(failure, CommandOutput::failed());
        assert_eq!(
            run_command_async("definitely-not-a-kimi-command", &[], None, None).await,
            CommandOutput::failed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn command_runner_times_out_without_hanging() {
        let (command, args) = if cfg!(windows) {
            (
                "cmd.exe",
                vec![
                    "/D".to_owned(),
                    "/C".to_owned(),
                    "ping -n 10 127.0.0.1 >nul".to_owned(),
                ],
            )
        } else {
            ("sh", vec!["-c".to_owned(), "sleep 10".to_owned()])
        };
        let task = tokio::spawn(async move {
            run_command_async(command, &args, Some(Duration::from_millis(50)), None).await
        });
        tokio::time::advance(Duration::from_millis(51)).await;
        assert_eq!(task.await.expect("command task"), CommandOutput::failed());
    }
}
