use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub command: String,
    pub args: Vec<String>,
    pub shell: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenInAppId {
    Finder,
    Cursor,
    VsCode,
    ITerm,
    Terminal,
}

pub const OPEN_IN_APP_IDS: &[OpenInAppId] = &[
    OpenInAppId::Finder,
    OpenInAppId::Cursor,
    OpenInAppId::VsCode,
    OpenInAppId::ITerm,
    OpenInAppId::Terminal,
];

// Original: lib/fileLaunch.ts, openFileCommandFor().
pub fn open_file_command_for(
    absolute_path: &Path,
    line: Option<u64>,
    env: &HashMap<String, String>,
    platform: Platform,
) -> LaunchCommand {
    if let Some(editor) = resolve_editor_command(env) {
        let path = absolute_path.to_string_lossy();
        let target = if supports_line_target(editor) {
            line.map_or_else(|| path.to_string(), |line| format!("{path}:{line}"))
        } else {
            path.to_string()
        };
        return LaunchCommand {
            command: format!("{editor} {}", quote_shell_arg(&target, platform)),
            args: Vec::new(),
            shell: true,
        };
    }
    match platform {
        Platform::MacOs => command("open", [absolute_path.to_string_lossy().as_ref()]),
        Platform::Windows => command(
            "cmd",
            [
                "/c",
                "start",
                "\"\"",
                absolute_path.to_string_lossy().as_ref(),
            ],
        ),
        Platform::Linux => command("xdg-open", [absolute_path.to_string_lossy().as_ref()]),
    }
}

pub fn reveal_file_command_for(absolute_path: &Path, platform: Platform) -> LaunchCommand {
    let path = absolute_path.to_string_lossy();
    match platform {
        Platform::MacOs => command("open", ["-R", path.as_ref()]),
        Platform::Windows => command("explorer.exe", [format!("/select,{path}")]),
        Platform::Linux => command(
            "xdg-open",
            [absolute_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_string_lossy()
                .as_ref()],
        ),
    }
}

pub fn open_in_app_command_for(
    app_id: OpenInAppId,
    absolute_path: &Path,
    line: Option<u64>,
    is_directory: Option<bool>,
    platform: Platform,
    env: &HashMap<String, String>,
) -> LaunchCommand {
    match app_id {
        OpenInAppId::VsCode => open_in_vscode_like("code", absolute_path, line, platform),
        OpenInAppId::Cursor => open_in_vscode_like("cursor", absolute_path, line, platform),
        OpenInAppId::Finder => open_in_finder(absolute_path, is_directory, platform),
        OpenInAppId::ITerm => open_in_mac_app("iTerm", absolute_path, platform, env),
        OpenInAppId::Terminal => open_in_mac_app("Terminal", absolute_path, platform, env),
    }
}

pub async fn get_available_open_in_apps(
    platform: Platform,
    home_dir: Option<&Path>,
) -> Vec<OpenInAppId> {
    let mut available = Vec::new();
    for &app_id in OPEN_IN_APP_IDS {
        let is_available = match app_id {
            OpenInAppId::Finder | OpenInAppId::Terminal => platform == Platform::MacOs,
            OpenInAppId::ITerm => {
                platform == Platform::MacOs
                    && (Path::new("/Applications/iTerm.app").exists()
                        || home_dir
                            .is_some_and(|home| home.join("Applications/iTerm.app").exists()))
            }
            OpenInAppId::VsCode => command_exists("code", platform).await,
            OpenInAppId::Cursor => command_exists("cursor", platform).await,
        };
        if is_available {
            available.push(app_id);
        }
    }
    available
}

// Original: lib/fileLaunch.ts, launchDetached().
pub async fn launch_detached(launch: &LaunchCommand) -> io::Result<()> {
    let mut command = if launch.shell {
        match Platform::current() {
            Platform::Windows => {
                let mut command = Command::new("cmd");
                command.args(["/d", "/s", "/c", &launch.command]);
                command
            }
            Platform::Linux | Platform::MacOs => {
                let mut command = Command::new("sh");
                command.args(["-c", &launch.command]);
                command
            }
        }
    } else {
        let mut command = Command::new(&launch.command);
        command.args(&launch.args);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().map(|_| ())
}

fn resolve_editor_command(env: &HashMap<String, String>) -> Option<&str> {
    ["KIMI_CODE_EDITOR", "VISUAL", "EDITOR"]
        .into_iter()
        .find_map(|key| env.get(key).map(String::as_str).map(str::trim))
        .filter(|value| !value.is_empty())
}

fn supports_line_target(editor: &str) -> bool {
    let first = editor.split_whitespace().next().unwrap_or("");
    let file_name = first
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(first)
        .to_ascii_lowercase();
    let stem = file_name
        .strip_suffix(".cmd")
        .or_else(|| file_name.strip_suffix(".exe"))
        .unwrap_or(&file_name);
    matches!(stem, "code" | "cursor" | "windsurf")
}

fn quote_shell_arg(value: &str, platform: Platform) -> String {
    match platform {
        Platform::Windows => format!("\"{}\"", value.replace('"', "\\\"")),
        Platform::Linux | Platform::MacOs => {
            format!("'{}'", value.replace('\'', "'\\''"))
        }
    }
}

fn open_in_vscode_like(
    binary: &str,
    absolute_path: &Path,
    line: Option<u64>,
    platform: Platform,
) -> LaunchCommand {
    let path = absolute_path.to_string_lossy();
    let target = line.map_or_else(|| path.to_string(), |line| format!("{path}:{line}"));
    let flag = if line.is_some() { "-g " } else { "" };
    LaunchCommand {
        command: format!("{binary} {flag}{}", quote_shell_arg(&target, platform)),
        args: Vec::new(),
        shell: true,
    }
}

fn open_in_finder(
    absolute_path: &Path,
    is_directory: Option<bool>,
    platform: Platform,
) -> LaunchCommand {
    let path = absolute_path.to_string_lossy();
    match platform {
        Platform::MacOs if is_directory == Some(true) => command("open", [path.as_ref()]),
        Platform::MacOs => command("open", ["-R", path.as_ref()]),
        Platform::Windows if is_directory == Some(true) => command("explorer.exe", [path.as_ref()]),
        Platform::Windows => command("explorer.exe", [format!("/select,{path}")]),
        Platform::Linux => command(
            "xdg-open",
            [if is_directory == Some(true) {
                absolute_path
            } else {
                absolute_path.parent().unwrap_or_else(|| Path::new("."))
            }
            .to_string_lossy()
            .as_ref()],
        ),
    }
}

fn open_in_mac_app(
    app_name: &str,
    absolute_path: &Path,
    platform: Platform,
    env: &HashMap<String, String>,
) -> LaunchCommand {
    if platform == Platform::MacOs {
        command(
            "open",
            ["-a", app_name, absolute_path.to_string_lossy().as_ref()],
        )
    } else {
        open_file_command_for(absolute_path, None, env, platform)
    }
}

async fn command_exists(command: &str, platform: Platform) -> bool {
    let status = match platform {
        Platform::Windows => {
            let mut process = Command::new("cmd");
            process.args(["/c", "where", command]);
            process
        }
        Platform::Linux | Platform::MacOs => {
            let mut process = Command::new("sh");
            process.args([
                "-c",
                &format!("command -v {}", quote_shell_arg(command, platform)),
            ]);
            process
        }
    }
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .await;
    status.is_ok_and(|status| status.success())
}

fn command<I, S>(command: impl Into<String>, args: I) -> LaunchCommand
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    LaunchCommand {
        command: command.into(),
        args: args.into_iter().map(Into::into).collect(),
        shell: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("/tmp/a file.rs")
    }

    #[test]
    fn honors_editor_precedence_and_line_capability() {
        let env = HashMap::from([
            ("EDITOR".into(), "vim".into()),
            ("KIMI_CODE_EDITOR".into(), "code".into()),
        ]);
        assert_eq!(
            open_file_command_for(&path(), Some(12), &env, Platform::Linux),
            LaunchCommand {
                command: "code '/tmp/a file.rs:12'".into(),
                args: vec![],
                shell: true
            }
        );
        let env = HashMap::from([("EDITOR".into(), "vim".into())]);
        assert_eq!(
            open_file_command_for(&path(), Some(12), &env, Platform::Linux).command,
            "vim '/tmp/a file.rs'"
        );
    }

    #[test]
    fn builds_platform_default_and_reveal_commands() {
        assert_eq!(
            open_file_command_for(&path(), None, &HashMap::new(), Platform::Windows).args,
            ["/c", "start", "\"\"", "/tmp/a file.rs"]
        );
        assert_eq!(
            reveal_file_command_for(&path(), Platform::MacOs).args,
            ["-R", "/tmp/a file.rs"]
        );
        assert_eq!(
            reveal_file_command_for(&path(), Platform::Linux).args,
            ["/tmp"]
        );
    }

    #[test]
    fn quotes_shell_metacharacters_without_interpolation() {
        assert_eq!(
            quote_shell_arg("/tmp/it's here", Platform::Linux),
            "'/tmp/it'\\''s here'"
        );
        assert_eq!(
            quote_shell_arg("C:\\a\"b", Platform::Windows),
            "\"C:\\a\\\"b\""
        );
    }
}
