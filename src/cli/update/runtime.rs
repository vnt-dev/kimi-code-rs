use std::{error::Error, fmt, path::PathBuf, process::Stdio};

use async_trait::async_trait;
use tokio::process::Command;

use super::{
    preflight::{
        ForegroundInstallerRuntime, SpawnUpdateExit, SpawnUpdateRequest, UpdateInstallError,
        UpdatePlatform,
    },
    source::{DetectInstallSourceDeps, InstallPlatform},
};
use crate::utils::shell_quote::{ShellPlatform, quote_shell_arg_for};

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemForegroundInstallerRuntime;

#[async_trait]
impl ForegroundInstallerRuntime for SystemForegroundInstallerRuntime {
    // Original:
    //   apps/kimi-code/src/cli/update/preflight.ts
    //   installUpdate() child-process boundary
    async fn spawn_and_wait(
        &self,
        request: SpawnUpdateRequest,
    ) -> Result<SpawnUpdateExit, UpdateInstallError> {
        let mut command = command_for_request(&request);
        if request.inherit_stdio {
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        } else {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        }
        let status = command.status().await.map_err(UpdateInstallError::new)?;
        Ok(SpawnUpdateExit {
            code: status.code(),
            signal: exit_signal_name(&status),
        })
    }
}

fn command_for_request(request: &SpawnUpdateRequest) -> Command {
    if !request.shell {
        let mut command = Command::new(&request.command);
        command.args(&request.arguments);
        return command;
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let command_line = std::iter::once(request.command.clone())
            .chain(
                request
                    .arguments
                    .iter()
                    .map(|argument| quote_shell_arg_for(argument, ShellPlatform::WindowsCmd)),
            )
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C"]);
        command.as_std_mut().raw_arg(command_line);
        command
    }
    #[cfg(not(windows))]
    {
        let command_line = std::iter::once(request.command.as_str())
            .chain(request.arguments.iter().map(String::as_str))
            .map(|argument| quote_shell_arg_for(argument, ShellPlatform::Posix))
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = Command::new("sh");
        command.args(["-c", &command_line]);
        command
    }
}

#[cfg(unix)]
fn exit_signal_name(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;

    status.signal().map(|signal| {
        match signal {
            1 => "SIGHUP",
            2 => "SIGINT",
            3 => "SIGQUIT",
            6 => "SIGABRT",
            9 => "SIGKILL",
            13 => "SIGPIPE",
            14 => "SIGALRM",
            15 => "SIGTERM",
            _ => return format!("SIG{signal}"),
        }
        .to_owned()
    })
}

#[cfg(not(unix))]
fn exit_signal_name(_: &std::process::ExitStatus) -> Option<String> {
    None
}

#[derive(Debug)]
pub struct InstallSourceRuntimeError(Box<dyn Error + Send + Sync>);

impl fmt::Display for InstallSourceRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for InstallSourceRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInstallSourceRuntime {
    package_root: PathBuf,
    native_install: bool,
    platform: InstallPlatform,
}

impl SystemInstallSourceRuntime {
    pub fn new(package_root: impl Into<PathBuf>, native_install: bool) -> Self {
        Self {
            package_root: package_root.into(),
            native_install,
            platform: if UpdatePlatform::current() == UpdatePlatform::Windows {
                InstallPlatform::Windows
            } else {
                InstallPlatform::Unix
            },
        }
    }

    pub fn with_platform(
        package_root: impl Into<PathBuf>,
        native_install: bool,
        platform: InstallPlatform,
    ) -> Self {
        Self {
            package_root: package_root.into(),
            native_install,
            platform,
        }
    }
}

#[async_trait]
impl DetectInstallSourceDeps for SystemInstallSourceRuntime {
    type Error = InstallSourceRuntimeError;

    fn package_root(&self) -> String {
        self.package_root.to_string_lossy().into_owned()
    }

    // Original:
    //   apps/kimi-code/src/cli/update/source.ts
    //   getGlobalPrefix default dependency
    async fn global_prefix(&self) -> Result<String, Self::Error> {
        let command = if self.platform == InstallPlatform::Windows {
            "npm.cmd"
        } else {
            "npm"
        };
        let output = Command::new(command)
            .args(["prefix", "-g"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|error| InstallSourceRuntimeError(Box::new(error)))?;
        if !output.status.success() {
            return Err(InstallSourceRuntimeError(Box::new(CommandFailed {
                command: format!("{command} prefix -g"),
                code: output.status.code(),
            })));
        }
        Ok(String::from_utf8(output.stdout)
            .map_err(|error| InstallSourceRuntimeError(Box::new(error)))?
            .trim()
            .to_owned())
    }

    fn detect_native(&self) -> bool {
        self.native_install
    }

    fn platform(&self) -> InstallPlatform {
        self.platform
    }
}

#[derive(Debug)]
struct CommandFailed {
    command: String,
    code: Option<i32>,
}

impl fmt::Display for CommandFailed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} exited with code {}",
            self.command,
            self.code
                .map_or_else(|| "null".to_owned(), |code| code.to_string())
        )
    }
}

impl Error for CommandFailed {}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit_request(code: i32, shell: bool) -> SpawnUpdateRequest {
        if cfg!(windows) {
            SpawnUpdateRequest {
                command: "powershell.exe".to_owned(),
                arguments: vec![
                    "-NoProfile".to_owned(),
                    "-Command".to_owned(),
                    format!("exit {code}"),
                ],
                inherit_stdio: false,
                shell,
            }
        } else {
            SpawnUpdateRequest {
                command: "sh".to_owned(),
                arguments: vec!["-c".to_owned(), format!("exit {code}")],
                inherit_stdio: false,
                shell,
            }
        }
    }

    #[tokio::test]
    async fn foreground_runtime_reports_real_exit_status_with_and_without_shell() {
        let runtime = SystemForegroundInstallerRuntime;
        for shell in [false, true] {
            let exit = runtime
                .spawn_and_wait(exit_request(7, shell))
                .await
                .expect("spawn command");
            assert_eq!(exit.code, Some(7), "shell={shell}");
            assert_eq!(exit.signal, None);
        }
    }

    #[tokio::test]
    async fn foreground_runtime_propagates_spawn_errors() {
        let error = SystemForegroundInstallerRuntime
            .spawn_and_wait(SpawnUpdateRequest {
                command: "definitely-not-a-kimi-command".to_owned(),
                arguments: Vec::new(),
                inherit_stdio: false,
                shell: false,
            })
            .await
            .expect_err("missing command");
        assert!(!error.to_string().is_empty());
    }

    #[tokio::test]
    async fn native_source_detection_short_circuits_npm_execution() {
        let runtime = SystemInstallSourceRuntime::with_platform(
            "/not/an/npm/package",
            true,
            InstallPlatform::Unix,
        );
        assert_eq!(
            super::super::source::detect_install_source(&runtime).await,
            super::super::types::InstallSource::Native
        );
    }
}
