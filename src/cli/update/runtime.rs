use std::{
    error::Error,
    fmt,
    io::{IsTerminal, Write},
    path::PathBuf,
    process::Stdio,
    time::SystemTime,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::process::Command;

use super::{
    cache::{UpdateCacheWriteError, write_update_cache},
    cdn::{CdnError, CdnFetch, CdnResponse, fetch_latest_from_cdn},
    preflight::{
        ForegroundInstallerRuntime, SpawnUpdateExit, SpawnUpdateRequest, UpdateInstallError,
        UpdatePlatform,
    },
    prompt::{InstallPromptRuntime, PromptKey},
    refresh::RefreshUpdateCacheDeps,
    source::{DetectInstallSourceDeps, InstallPlatform},
    types::{FetchLatestResult, UpdateCache},
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

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemInstallPromptRuntime;

#[async_trait]
impl InstallPromptRuntime for SystemInstallPromptRuntime {
    fn raw_mode(&self) -> bool {
        crossterm::terminal::is_raw_mode_enabled().unwrap_or(false)
    }

    fn can_set_raw_mode(&self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn set_raw_mode(&self, enabled: bool) {
        if enabled {
            let _ = crossterm::terminal::enable_raw_mode();
        } else {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }

    fn resume_input(&self) {
        // Rust's stdin handle has no paused state equivalent to Node streams.
    }

    async fn next_keypress(&self) -> PromptKey {
        tokio::task::spawn_blocking(|| {
            loop {
                match crossterm::event::read() {
                    Ok(crossterm::event::Event::Key(event)) => return prompt_key_for_event(event),
                    Ok(_) => {}
                    Err(_) => return PromptKey::CtrlC,
                }
            }
        })
        .await
        .unwrap_or(PromptKey::CtrlC)
    }

    fn color_enabled(&self) -> bool {
        std::io::stdout().is_terminal()
    }

    fn write_output(&self, text: &str) {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }
}

fn prompt_key_for_event(event: crossterm::event::KeyEvent) -> PromptKey {
    use crossterm::event::{KeyCode, KeyModifiers};

    match event.code {
        KeyCode::Up => PromptKey::Up,
        KeyCode::Down => PromptKey::Down,
        KeyCode::Enter => PromptKey::Enter,
        KeyCode::Esc => PromptKey::Escape,
        KeyCode::Char('c' | 'C') if event.modifiers.contains(KeyModifiers::CONTROL) => {
            PromptKey::CtrlC
        }
        _ => PromptKey::Other,
    }
}

#[derive(Debug, Clone)]
pub struct SystemCdnFetch {
    client: reqwest::Client,
}

impl Default for SystemCdnFetch {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl CdnFetch for SystemCdnFetch {
    type Error = reqwest::Error;

    // Original:
    //   apps/kimi-code/src/cli/update/cdn.ts
    //   fetchWithTimeout() fetch boundary
    async fn fetch(&self, url: &str) -> Result<CdnResponse, Self::Error> {
        let response = self.client.get(url).send().await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        Ok(CdnResponse { status, body })
    }
}

#[derive(Debug)]
pub enum UpdateRefreshRuntimeError {
    Cdn(CdnError<reqwest::Error>),
    Cache(UpdateCacheWriteError),
}

impl fmt::Display for UpdateRefreshRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cdn(error) => error.fmt(formatter),
            Self::Cache(error) => error.fmt(formatter),
        }
    }
}

impl Error for UpdateRefreshRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Cdn(error) => Some(error),
            Self::Cache(error) => Some(error),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SystemUpdateRefreshRuntime {
    fetcher: SystemCdnFetch,
}

#[async_trait]
impl RefreshUpdateCacheDeps for SystemUpdateRefreshRuntime {
    type Error = UpdateRefreshRuntimeError;

    // Original:
    //   apps/kimi-code/src/cli/update/refresh.ts
    //   refreshUpdateCache() default fetch dependency
    async fn fetch_latest(&self) -> Result<FetchLatestResult, Self::Error> {
        fetch_latest_from_cdn(&self.fetcher)
            .await
            .map_err(UpdateRefreshRuntimeError::Cdn)
    }

    async fn write_cache(&self, cache: &UpdateCache) -> Result<(), Self::Error> {
        write_update_cache(cache)
            .await
            .map_err(UpdateRefreshRuntimeError::Cache)
    }

    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(SystemTime::now())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

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

    #[tokio::test]
    async fn system_cdn_fetch_returns_status_and_body_without_reclassifying_http_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).expect("read");
            let body = "release unavailable";
            write!(
                stream,
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("response");
        });

        let response = SystemCdnFetch::default()
            .fetch(&format!("http://{address}/latest"))
            .await
            .expect("HTTP response");
        server.join().expect("server");

        assert_eq!(response.status, 503);
        assert_eq!(response.body, "release unavailable");
    }

    #[test]
    fn terminal_prompt_maps_navigation_confirmation_and_cancellation_keys() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        for (event, expected) in [
            (
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                PromptKey::Up,
            ),
            (
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                PromptKey::Down,
            ),
            (
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                PromptKey::Enter,
            ),
            (
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                PromptKey::Escape,
            ),
            (
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                PromptKey::CtrlC,
            ),
            (
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                PromptKey::Other,
            ),
        ] {
            assert_eq!(prompt_key_for_event(event), expected);
        }
    }
}
