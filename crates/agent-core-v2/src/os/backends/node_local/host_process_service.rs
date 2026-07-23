//! Tokio-backed host child-process service.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/hostProcessService.ts`.

use std::{
    error::Error,
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
    _base::errors::errors::{Error2Options, ErrorCause},
    os::interface::host_process::{
        HostProcess, HostProcessError, HostProcessOptions, HostProcessService,
        OS_PROCESS_KILL_FAILED, OS_PROCESS_SPAWN_FAILED, ProcessReader, ProcessShell,
        ProcessSignal, ProcessWriter, SharedProcessReader, SharedProcessWriter,
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
        if self.pid <= 0 {
            return Ok(());
        }
        kill_process_tree(self.pid, signal.unwrap_or(ProcessSignal::Terminate)).await
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
pub struct LocalHostProcessService;

#[async_trait]
impl HostProcessService for LocalHostProcessService {
    async fn spawn(
        &self,
        command: &str,
        args: &[String],
        options: HostProcessOptions,
    ) -> Result<Arc<dyn HostProcess>, HostProcessError> {
        let mut process = build_command(command, args, &options);
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
        let stdout: SharedProcessReader = Arc::new(Mutex::new(Box::new(stdout) as ProcessReader));
        let (stderr, ignored_stderr): (SharedProcessReader, Option<SharedProcessReader>) =
            if options.merge_stderr.unwrap_or(false) {
                (
                    Arc::clone(&stdout),
                    Some(Arc::new(Mutex::new(Box::new(raw_stderr) as ProcessReader))),
                )
            } else {
                (
                    Arc::new(Mutex::new(Box::new(raw_stderr) as ProcessReader)),
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

fn build_command(command: &str, args: &[String], options: &HostProcessOptions) -> Command {
    let mut process = match &options.shell {
        None => {
            let mut process = Command::new(command);
            process.args(args);
            process
        }
        Some(shell) => {
            let shell = match shell {
                ProcessShell::Default if cfg!(windows) => "cmd.exe",
                ProcessShell::Default => "/bin/sh",
                ProcessShell::Command(shell) => shell,
            };
            let mut process = Command::new(shell);
            if cfg!(windows) {
                process.arg("/C");
            } else {
                process.arg("-c");
            }
            process.arg(shell_command(command, args));
            process
        }
    };
    if let Some(cwd) = &options.cwd {
        process.current_dir(cwd);
    }
    if let Some(environment) = &options.env {
        process.envs(environment);
    }
    process
}

fn shell_command(command: &str, args: &[String]) -> String {
    std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:".contains(&byte))
    {
        value.into()
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
async fn kill_process_tree(pid: i64, _: ProcessSignal) -> Result<(), HostProcessError> {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    Ok(())
}

#[cfg(unix)]
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

#[cfg(unix)]
fn signal_name(signal: ProcessSignal) -> &'static str {
    match signal {
        ProcessSignal::Terminate => "SIGTERM",
        ProcessSignal::Kill => "SIGKILL",
        ProcessSignal::Interrupt => "SIGINT",
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt;

    use super::*;

    #[tokio::test]
    async fn spawns_captures_output_merges_environment_and_caches_exit() {
        let service = LocalHostProcessService;
        let process = service
            .spawn(
                "sh",
                &["-c".into(), "printf %s \"$KIMI_HOST_PROCESS_TEST\"".into()],
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
    async fn missing_command_returns_coded_error_with_details_and_cause() {
        let error = LocalHostProcessService
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
        let process = LocalHostProcessService
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
}
