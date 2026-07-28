//! Tokio-backed host child-process service.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/hostProcessService.ts`.

use std::{
    collections::HashMap,
    error::Error,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
};

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::{
    io::{empty, sink},
    process::Command,
    sync::{Mutex, watch},
};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2Options, ErrorCause},
        exec_env::buffered_readable::BufferedReadable,
    },
    os::interface::host_process::{
        HOST_PROCESS_SERVICE_ID, HostProcess, HostProcessError, HostProcessOptions,
        HostProcessService, HostProcessServiceHandle, OS_PROCESS_KILL_FAILED,
        OS_PROCESS_SPAWN_FAILED, ProcessReader, ProcessShell, ProcessSignal, ProcessWriter,
        SharedProcessReader, SharedProcessWriter,
    },
};

const RUNNING: i32 = i32::MIN;

pub struct LocalHostProcess {
    pid: i64,
    exit_code: Arc<AtomicI32>,
    exit: watch::Receiver<Option<Result<i32, HostProcessError>>>,
    stdin: SharedProcessWriter,
    stdout: SharedProcessReader,
    stderr: SharedProcessReader,
    ignored_stderr: Option<SharedProcessReader>,
    disposed: AtomicBool,
}

#[async_trait]
impl HostProcess for LocalHostProcess {
    fn pid(&self) -> i64 {
        self.pid
    }

    fn exit_code(&self) -> Option<i32> {
        match self.exit_code.load(Ordering::Acquire) {
            RUNNING => None,
            code => Some(code),
        }
    }

    fn stdin(&self) -> SharedProcessWriter {
        Arc::clone(&self.stdin)
    }
    fn stdout(&self) -> SharedProcessReader {
        Arc::clone(&self.stdout)
    }
    fn stderr(&self) -> SharedProcessReader {
        Arc::clone(&self.stderr)
    }

    async fn wait(&self) -> Result<i32, HostProcessError> {
        let mut exit = self.exit.clone();
        loop {
            if let Some(result) = exit.borrow().clone() {
                return result;
            }
            if exit.changed().await.is_err() {
                return Ok(self.exit_code().unwrap_or(-1));
            }
        }
    }

    async fn kill(&self, signal: Option<ProcessSignal>) -> Result<(), HostProcessError> {
        if self.pid <= 0 || self.exit_code().is_some() {
            return Ok(());
        }
        let result = kill_process_tree(self.pid, signal.unwrap_or(ProcessSignal::Terminate)).await;
        if result.is_err() && self.exit_code().is_some() {
            Ok(())
        } else {
            result
        }
    }

    fn dispose(&self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut input) = self.stdin.try_lock() {
            *input = Box::new(sink());
        }
        if let Ok(mut output) = self.stdout.try_lock() {
            *output = Box::new(empty());
        }
        if !Arc::ptr_eq(&self.stderr, &self.stdout)
            && let Ok(mut error) = self.stderr.try_lock()
        {
            *error = Box::new(empty());
        }
        if let Some(ignored) = &self.ignored_stderr
            && let Ok(mut error) = ignored.try_lock()
        {
            *error = Box::new(empty());
        }
    }
}

#[derive(Default)]
pub struct LocalHostProcessService {
    base_environment: Option<Arc<HashMap<String, String>>>,
}

impl LocalHostProcessService {
    pub fn with_environment(environment: Arc<HashMap<String, String>>) -> Self {
        Self {
            base_environment: Some(environment),
        }
    }
}

/// Original: `hostProcessService.ts`, App-scope eager registration.
pub fn register_local_host_process_service() {
    register_scoped_service(
        LifecycleScope::App,
        HOST_PROCESS_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn HostProcessService> = Arc::new(LocalHostProcessService::default());
            Ok(HostProcessServiceHandle(service))
        }),
        InstantiationType::Eager,
        "hostProcess",
    );
}

#[async_trait]
impl HostProcessService for LocalHostProcessService {
    async fn spawn(
        &self,
        command: &str,
        args: &[String],
        options: HostProcessOptions,
    ) -> Result<Arc<dyn HostProcess>, HostProcessError> {
        let mut process = build_command(command, args, &options, self.base_environment.as_deref());
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        if options.detached.unwrap_or(true) {
            process.process_group(0);
        }
        let mut child = process
            .spawn()
            .map_err(|error| spawn_error(error, command, args, &options))?;
        let pid = child.id().map(i64::from).unwrap_or(-1);
        let stdin = child.stdin.take().ok_or_else(|| {
            simple_spawn_error("Process must be created with stdin/stdout pipes.")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            simple_spawn_error("Process must be created with stdin/stdout pipes.")
        })?;
        let raw_stderr = child.stderr.take().ok_or_else(|| {
            simple_spawn_error(
                "Process must be created with stderr pipe unless mergeStderr is set.",
            )
        })?;
        let stdin: SharedProcessWriter = Arc::new(Mutex::new(Box::new(stdin) as ProcessWriter));
        let stdout: SharedProcessReader = Arc::new(Mutex::new(Box::new(BufferedReadable::new(
            stdout,
        )) as ProcessReader));
        let (stderr, ignored_stderr): (SharedProcessReader, Option<SharedProcessReader>) =
            if options.merge_stderr.unwrap_or(false) {
                (
                    Arc::clone(&stdout),
                    Some(Arc::new(Mutex::new(
                        Box::new(BufferedReadable::new(raw_stderr)) as ProcessReader,
                    ))),
                )
            } else {
                (
                    Arc::new(Mutex::new(
                        Box::new(BufferedReadable::new(raw_stderr)) as ProcessReader
                    )),
                    None,
                )
            };
        let exit_code = Arc::new(AtomicI32::new(RUNNING));
        let (exit_tx, exit) = watch::channel(None);
        let exit_code_task = Arc::clone(&exit_code);
        tokio::spawn(async move {
            let result = child
                .wait()
                .await
                .map(|status| status.code().unwrap_or(-1))
                .map_err(|error| simple_spawn_error(format!("Host process wait failed: {error}")));
            if let Ok(code) = result {
                exit_code_task.store(code, Ordering::Release);
            }
            let _ = exit_tx.send(Some(result));
        });
        Ok(Arc::new(LocalHostProcess {
            pid,
            exit_code,
            exit,
            stdin,
            stdout,
            stderr,
            ignored_stderr,
            disposed: AtomicBool::new(false),
        }))
    }
}

fn build_command(
    command: &str,
    args: &[String],
    options: &HostProcessOptions,
    base_environment: Option<&HashMap<String, String>>,
) -> Command {
    let mut process = match &options.shell {
        None => {
            let mut process = Command::new(command);
            process.args(args);
            process
        }
        Some(shell) => {
            let (shell, is_cmd) = match shell {
                ProcessShell::Default if cfg!(windows) => ("cmd.exe", true),
                ProcessShell::Default => ("/bin/sh", false),
                ProcessShell::Command(shell) => (
                    shell.as_str(),
                    cfg!(windows)
                        && Path::new(shell)
                            .file_stem()
                            .is_some_and(|name| name.eq_ignore_ascii_case("cmd")),
                ),
            };
            let mut process = Command::new(shell);
            if is_cmd {
                process.args(["/D", "/S", "/C"]);
                #[cfg(windows)]
                {
                    // `cmd.exe` parses its command tail directly rather than
                    // with the Windows C runtime argv rules. Passing it via
                    // `arg` would turn embedded quotes into literal `\"`.
                    process.raw_arg(shell_command(command, args, true));
                }
            } else {
                process.arg("-c");
                process.arg(shell_command(command, args, false));
            }
            process
        }
    };
    if let Some(environment) = base_environment {
        process.env_clear().envs(environment);
    }
    if let Some(cwd) = &options.cwd {
        process.current_dir(cwd);
    }
    if let Some(environment) = &options.env {
        process.envs(environment);
    }
    process
}

fn shell_command(command: &str, args: &[String], is_cmd: bool) -> String {
    std::iter::once(command.to_owned())
        .chain(args.iter().map(|argument| shell_quote(argument, is_cmd)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str, is_cmd: bool) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:".contains(&byte))
    {
        value.into()
    } else if is_cmd {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn simple_spawn_error(message: impl Into<String>) -> HostProcessError {
    HostProcessError::with_options(OS_PROCESS_SPAWN_FAILED, message, Error2Options::default())
}

fn spawn_error(
    error: std::io::Error,
    command: &str,
    args: &[String],
    options: &HostProcessOptions,
) -> HostProcessError {
    let errno = io_errno(&error);
    let details = Map::from_iter([
        ("command".into(), Value::String(command.into())),
        (
            "args".into(),
            Value::Array(args.iter().cloned().map(Value::String).collect()),
        ),
        (
            "cwd".into(),
            options.cwd.clone().map_or(Value::Null, Value::String),
        ),
        ("errno".into(), errno.map_or(Value::Null, Value::String)),
    ]);
    let message = format!("Failed to spawn \"{command}\": {error}");
    let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
    HostProcessError::with_options(
        OS_PROCESS_SPAWN_FAILED,
        message,
        Error2Options {
            details: Some(details),
            cause: Some(ErrorCause::Error(cause)),
            ..Error2Options::default()
        },
    )
}

fn io_errno(error: &std::io::Error) -> Option<String> {
    match error.kind() {
        std::io::ErrorKind::NotFound => Some("ENOENT".into()),
        std::io::ErrorKind::PermissionDenied => Some("EACCES".into()),
        _ => error.raw_os_error().map(|code| code.to_string()),
    }
}

#[cfg(unix)]
async fn kill_process_tree(pid: i64, signal: ProcessSignal) -> Result<(), HostProcessError> {
    let signal_name = signal_name(signal);
    let raw_signal = match signal {
        ProcessSignal::Terminate => libc::SIGTERM,
        ProcessSignal::Kill => libc::SIGKILL,
        ProcessSignal::Interrupt => libc::SIGINT,
    };
    let pid = i32::try_from(pid).unwrap_or(i32::MAX);
    // SAFETY: `kill` takes integer values only. The negative, validated child
    // PID targets the process group created by `Command::process_group(0)`.
    let result = unsafe { libc::kill(-pid, raw_signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    if error.raw_os_error() == Some(libc::EPERM) {
        // SAFETY: same integer-only OS API; positive PID falls back to the
        // direct child exactly like ChildProcess.kill() in the source.
        let _ = unsafe { libc::kill(pid, raw_signal) };
        return Ok(());
    }
    Err(kill_error(pid.into(), signal_name, error))
}

#[cfg(windows)]
async fn kill_process_tree(pid: i64, signal: ProcessSignal) -> Result<(), HostProcessError> {
    let output = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| kill_error(pid, signal_name(signal), error))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(kill_status_error(
        pid,
        signal_name(signal),
        output.status.code(),
        &stderr,
    ))
}

fn kill_error(pid: i64, signal: &str, error: std::io::Error) -> HostProcessError {
    let errno = io_errno(&error);
    let details = Map::from_iter([
        ("pid".into(), Value::from(pid)),
        ("signal".into(), Value::String(signal.into())),
        ("errno".into(), errno.map_or(Value::Null, Value::String)),
    ]);
    let message = format!("Failed to kill process {pid}: {error}");
    let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
    HostProcessError::with_options(
        OS_PROCESS_KILL_FAILED,
        message,
        Error2Options {
            details: Some(details),
            cause: Some(ErrorCause::Error(cause)),
            ..Error2Options::default()
        },
    )
}

fn signal_name(signal: ProcessSignal) -> &'static str {
    match signal {
        ProcessSignal::Terminate => "SIGTERM",
        ProcessSignal::Kill => "SIGKILL",
        ProcessSignal::Interrupt => "SIGINT",
    }
}

#[cfg(windows)]
fn kill_status_error(
    pid: i64,
    signal: &str,
    exit_code: Option<i32>,
    stderr: &str,
) -> HostProcessError {
    let details = Map::from_iter([
        ("pid".into(), Value::from(pid)),
        ("signal".into(), Value::String(signal.into())),
        (
            "exitCode".into(),
            exit_code.map_or(Value::Null, Value::from),
        ),
        ("stderr".into(), Value::String(stderr.into())),
    ]);
    let suffix = if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    };
    HostProcessError::with_options(
        OS_PROCESS_KILL_FAILED,
        format!("Failed to kill process {pid}{suffix}"),
        Error2Options {
            details: Some(details),
            ..Error2Options::default()
        },
    )
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use super::*;
    use crate::_base::di::{
        lifecycle::Disposable,
        scope::{Scope, ScopeOptions},
    };

    #[cfg(windows)]
    fn test_shell(script: &str) -> (String, Vec<String>) {
        (
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()),
            vec!["/D".into(), "/S".into(), "/C".into(), script.into()],
        )
    }

    #[cfg(not(windows))]
    fn test_shell(script: &str) -> (String, Vec<String>) {
        ("/bin/sh".into(), vec!["-c".into(), script.into()])
    }

    #[test]
    fn app_scope_registration_resolves_host_process_service() {
        register_local_host_process_service();
        let app = Scope::create_app(ScopeOptions::default());
        app.get(HOST_PROCESS_SERVICE_ID).unwrap();
        app.dispose().unwrap();
    }

    #[tokio::test]
    async fn spawns_captures_output_merges_environment_and_caches_exit() {
        let service = LocalHostProcessService::default();
        #[cfg(windows)]
        let script = "echo|set /p=%KIMI_HOST_PROCESS_TEST%&exit /b 0";
        #[cfg(not(windows))]
        let script = "printf %s \"$KIMI_HOST_PROCESS_TEST\"";
        let (shell, args) = test_shell(script);
        let process = service
            .spawn(
                &shell,
                &args,
                HostProcessOptions {
                    env: Some([("KIMI_HOST_PROCESS_TEST".into(), "ok".into())].into()),
                    ..HostProcessOptions::default()
                },
            )
            .await
            .unwrap();
        let mut output = String::new();
        process
            .stdout()
            .lock()
            .await
            .read_to_string(&mut output)
            .await
            .unwrap();
        assert_eq!(output, "ok");
        assert_eq!(process.wait().await.unwrap(), 0);
        assert_eq!(process.wait().await.unwrap(), 0);
        assert_eq!(process.exit_code(), Some(0));
    }

    #[tokio::test]
    async fn output_remains_readable_after_waiting_for_exit() {
        #[cfg(windows)]
        let script = "echo|set /p=buffered-after-wait&exit /b 0";
        #[cfg(not(windows))]
        let script = "printf %s buffered-after-wait";
        let (shell, args) = test_shell(script);
        let process = LocalHostProcessService::default()
            .spawn(&shell, &args, HostProcessOptions::default())
            .await
            .unwrap();

        assert_eq!(process.wait().await.unwrap(), 0);
        let mut output = String::new();
        process
            .stdout()
            .lock()
            .await
            .read_to_string(&mut output)
            .await
            .unwrap();
        assert_eq!(output, "buffered-after-wait");
    }

    #[tokio::test]
    async fn configured_environment_is_base_then_spawn_overrides_win() {
        let service = LocalHostProcessService::with_environment(Arc::new(
            [
                ("KIMI_BASE".into(), "base".into()),
                ("KIMI_OVERRIDE".into(), "old".into()),
            ]
            .into(),
        ));
        #[cfg(windows)]
        let script = "echo|set /p=%KIMI_BASE%:%KIMI_OVERRIDE%&exit /b 0";
        #[cfg(not(windows))]
        let script = "printf '%s:%s' \"$KIMI_BASE\" \"$KIMI_OVERRIDE\"";
        let (shell, args) = test_shell(script);
        let process = service
            .spawn(
                &shell,
                &args,
                HostProcessOptions {
                    env: Some([("KIMI_OVERRIDE".into(), "new".into())].into()),
                    ..HostProcessOptions::default()
                },
            )
            .await
            .unwrap();
        let mut output = String::new();
        process
            .stdout()
            .lock()
            .await
            .read_to_string(&mut output)
            .await
            .unwrap();
        assert_eq!(output, "base:new");
        assert_eq!(process.wait().await.unwrap(), 0);
    }

    // Original:
    //   packages/agent-core-v2/src/os/backends/node-local/hostProcessService.ts
    //   HostProcessService.spawn(command, args, { shell: true })
    #[tokio::test]
    async fn shell_option_executes_command_as_script_and_quotes_arguments() {
        let service = LocalHostProcessService::default();
        #[cfg(windows)]
        let (command, args, expected) = (
            "echo|set /p=shell-script:argument with spaces&exit /b 0",
            Vec::new(),
            "shell-script:argument with spaces",
        );
        #[cfg(not(windows))]
        let (command, args, expected) = (
            "printf '%s:%s' shell-script",
            vec!["argument with spaces".into()],
            "shell-script:argument with spaces",
        );
        let process = service
            .spawn(
                command,
                &args,
                HostProcessOptions {
                    shell: Some(ProcessShell::Default),
                    ..HostProcessOptions::default()
                },
            )
            .await
            .unwrap();
        let mut output = String::new();
        process
            .stdout()
            .lock()
            .await
            .read_to_string(&mut output)
            .await
            .unwrap();
        assert_eq!(process.wait().await.unwrap(), 0);
        assert_eq!(output, expected);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn default_cmd_shell_preserves_a_spaced_argument() {
        let process = LocalHostProcessService::default()
            .spawn(
                "echo|set /p=",
                &["argument with spaces".into()],
                HostProcessOptions {
                    shell: Some(ProcessShell::Default),
                    ..HostProcessOptions::default()
                },
            )
            .await
            .unwrap();
        let mut output = String::new();
        process
            .stdout()
            .lock()
            .await
            .read_to_string(&mut output)
            .await
            .unwrap();
        assert_eq!(process.wait().await.unwrap(), 1);
        assert_eq!(output, "argument with spaces");
    }

    #[tokio::test]
    async fn missing_command_returns_coded_error_with_details_and_cause() {
        let error = LocalHostProcessService::default()
            .spawn(
                "definitely-not-a-real-command-42",
                &[],
                HostProcessOptions::default(),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.code(), OS_PROCESS_SPAWN_FAILED);
        assert_eq!(
            error.error().details.as_ref().unwrap()["command"],
            "definitely-not-a-real-command-42"
        );
        assert_eq!(error.error().details.as_ref().unwrap()["errno"], "ENOENT");
        assert!(error.source().is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminates_a_running_process_group() {
        let process = LocalHostProcessService::default()
            .spawn(
                "sh",
                &["-c".into(), "sleep 30".into()],
                HostProcessOptions::default(),
            )
            .await
            .unwrap();
        assert!(process.pid() > 0);
        process.kill(None).await.unwrap();
        assert_ne!(process.wait().await.unwrap(), 0);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn terminates_a_running_process_tree() {
        let (shell, args) = test_shell("ping 127.0.0.1 -n 31 >nul");
        let process = LocalHostProcessService::default()
            .spawn(&shell, &args, HostProcessOptions::default())
            .await
            .unwrap();
        assert!(process.pid() > 0);
        let exit_code = tokio::time::timeout(Duration::from_secs(10), async {
            process.kill(None).await?;
            process.wait().await
        })
        .await
        .unwrap()
        .unwrap();
        assert_ne!(exit_code, 0);
    }
}
