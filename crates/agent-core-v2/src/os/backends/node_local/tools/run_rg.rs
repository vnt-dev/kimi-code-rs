//! Shared ripgrep subprocess timeout, cancellation and output plumbing.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/tools/runRg.ts`.

use std::{sync::Arc, time::Duration};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    _base::utils::abort::AbortSignal,
    os::interface::host_process::{
        HostProcess, HostProcessError, HostProcessOptions, HostProcessService, ProcessSignal,
        SharedProcessReader,
    },
};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
pub const SIGTERM_GRACE: Duration = Duration::from_secs(5);
pub const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunRgResult {
    pub exit_code: i32,
    pub stdout_text: String,
    pub stderr_text: String,
    pub buffer_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunRgOutcome {
    Result(RunRgResult),
    Aborted,
}

#[derive(Debug, thiserror::Error)]
pub enum RunRgError {
    #[error("runRgOnce: rgArgs must not be empty")]
    EmptyArguments,
    #[error(transparent)]
    Process(#[from] HostProcessError),
    #[error("failed to read ripgrep output: {0}")]
    Read(#[from] std::io::Error),
}

#[derive(Clone, Copy)]
struct RunOptions {
    timeout: Duration,
    grace: Duration,
    max_output_bytes: usize,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            grace: SIGTERM_GRACE,
            max_output_bytes: MAX_OUTPUT_BYTES,
        }
    }
}

pub async fn run_rg_once(
    process_service: &dyn HostProcessService,
    rg_args: &[String],
    signal: &AbortSignal,
    cwd: Option<String>,
) -> Result<RunRgOutcome, RunRgError> {
    run_rg_with_options(process_service, rg_args, signal, cwd, RunOptions::default()).await
}

async fn run_rg_with_options(
    process_service: &dyn HostProcessService,
    rg_args: &[String],
    signal: &AbortSignal,
    cwd: Option<String>,
    options: RunOptions,
) -> Result<RunRgOutcome, RunRgError> {
    if signal.aborted() {
        return Ok(RunRgOutcome::Aborted);
    }
    let (command, args) = rg_args.split_first().ok_or(RunRgError::EmptyArguments)?;
    let process = process_service
        .spawn(
            command,
            args,
            HostProcessOptions {
                cwd,
                ..HostProcessOptions::default()
            },
        )
        .await?;
    if let Ok(mut input) = process.stdin().try_lock() {
        let _ = input.shutdown().await;
    }

    let stdout = read_stream_with_cap(process.stdout(), options.max_output_bytes);
    let stderr = read_stream_with_cap(process.stderr(), options.max_output_bytes);
    let wait = process.wait();
    let collection = async {
        tokio::try_join!(
            async { stdout.await.map_err(RunRgError::from) },
            async { stderr.await.map_err(RunRgError::from) },
            async { wait.await.map_err(RunRgError::from) },
        )
    };
    tokio::pin!(collection);

    let mut timed_out = false;
    let mut aborted = false;
    let collected = tokio::select! {
        result = &mut collection => result,
        _ = signal.cancelled() => {
            aborted = true;
            terminate(&process, options.grace).await;
            collection.await
        }
        _ = tokio::time::sleep(options.timeout) => {
            timed_out = true;
            terminate(&process, options.grace).await;
            collection.await
        }
    };
    process.dispose();
    let (stdout, stderr, exit_code) = collected?;
    if aborted {
        return Ok(RunRgOutcome::Aborted);
    }
    Ok(RunRgOutcome::Result(RunRgResult {
        exit_code,
        stdout_text: String::from_utf8_lossy(&stdout.bytes).into_owned(),
        stderr_text: String::from_utf8_lossy(&stderr.bytes).into_owned(),
        buffer_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        timed_out,
    }))
}

async fn terminate(process: &Arc<dyn HostProcess>, grace: Duration) {
    let _ = process.kill(Some(ProcessSignal::Terminate)).await;
    if tokio::time::timeout(grace, process.wait()).await.is_err() && process.exit_code().is_none() {
        let _ = process.kill(Some(ProcessSignal::Kill)).await;
    }
    process.dispose();
}

struct CappedStreamResult {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_stream_with_cap(
    stream: SharedProcessReader,
    max_bytes: usize,
) -> Result<CappedStreamResult, std::io::Error> {
    let mut stream = stream.lock().await;
    let mut output = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(output.len());
        if count > remaining {
            truncated = true;
        }
        output.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    Ok(CappedStreamResult {
        bytes: output,
        truncated,
    })
}

pub fn should_retry_ripgrep_eagain(result: &RunRgResult) -> bool {
    result.exit_code != 0
        && result.exit_code != 1
        && !result.timed_out
        && (result.stderr_text.contains("os error 11")
            || result
                .stderr_text
                .contains("Resource temporarily unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        os::backends::node_local::host_process_service::LocalHostProcessService,
    };

    #[cfg(windows)]
    fn test_shell(script: &str) -> Vec<String> {
        vec![
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()),
            "/D".into(),
            "/S".into(),
            "/C".into(),
            script.into(),
        ]
    }

    #[cfg(not(windows))]
    fn test_shell(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    #[tokio::test]
    async fn captures_caps_and_reports_normal_result() {
        let controller = AbortController::new();
        #[cfg(windows)]
        let command = "echo|set /p=123456789&exit /b 0";
        #[cfg(not(windows))]
        let command = "printf 123456789";
        let result = run_rg_with_options(
            &LocalHostProcessService::default(),
            &test_shell(command),
            &controller.signal(),
            None,
            RunOptions {
                max_output_bytes: 5,
                ..RunOptions::default()
            },
        )
        .await
        .unwrap();
        let RunRgOutcome::Result(result) = result else {
            panic!("expected result");
        };
        assert_eq!(result.stdout_text, "12345");
        assert!(result.buffer_truncated);
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn aborts_before_and_during_execution() {
        let before = AbortController::new();
        before.abort(None);
        assert_eq!(
            run_rg_once(
                &LocalHostProcessService::default(),
                &["rg".into()],
                &before.signal(),
                None
            )
            .await
            .unwrap(),
            RunRgOutcome::Aborted
        );

        let during = AbortController::new();
        let signal = during.signal();
        #[cfg(windows)]
        let command = "ping 127.0.0.1 -n 31 >nul";
        #[cfg(not(windows))]
        let command = "sleep 30";
        let shell = test_shell(command);
        let task = tokio::spawn(async move {
            run_rg_with_options(
                &LocalHostProcessService::default(),
                &shell,
                &signal,
                None,
                RunOptions {
                    grace: Duration::from_millis(20),
                    ..RunOptions::default()
                },
            )
            .await
        });
        tokio::task::yield_now().await;
        during.abort(None);
        assert_eq!(task.await.unwrap().unwrap(), RunRgOutcome::Aborted);
    }

    #[test]
    fn retries_only_nonstandard_non_timeout_eagain_failures() {
        let mut result = RunRgResult {
            exit_code: 2,
            stdout_text: String::new(),
            stderr_text: "Resource temporarily unavailable".into(),
            buffer_truncated: false,
            stderr_truncated: false,
            timed_out: false,
        };
        assert!(should_retry_ripgrep_eagain(&result));
        result.exit_code = 1;
        assert!(!should_retry_ripgrep_eagain(&result));
        result.exit_code = 2;
        result.timed_out = true;
        assert!(!should_retry_ripgrep_eagain(&result));
    }
}
