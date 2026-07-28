//! Shared ripgrep subprocess plumbing.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/runRg.ts`.

use std::{error::Error, sync::Arc, time::Duration};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    _base::utils::abort::AbortSignal,
    os::interface::host_process::{HostProcess, ProcessSignal},
    session::process::{ProcessExecOptions, SessionProcessRunnerContract},
};

pub const DEFAULT_TIMEOUT_MS: u64 = 20_000;
pub const SIGTERM_GRACE_MS: u64 = 5_000;
pub const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunRgResult {
    pub exit_code: i32,
    pub stdout_text: String,
    pub stderr_text: String,
    pub buffer_truncated: bool,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunRgOutcome {
    Result(RunRgResult),
    Aborted,
}

pub async fn run_rg_once(
    runner: &dyn SessionProcessRunnerContract,
    rg_args: &[String],
    signal: &AbortSignal,
    cwd: Option<String>,
) -> Result<RunRgOutcome, Box<dyn Error + Send + Sync>> {
    if signal.aborted() {
        return Ok(RunRgOutcome::Aborted);
    }

    let process = runner
        .exec(rg_args, Some(ProcessExecOptions { cwd, env: None }))
        .await?;
    {
        let stdin = process.stdin();
        let mut stdin = stdin.lock().await;
        let _ = stdin.shutdown().await;
    }

    let stdout_process = Arc::clone(&process);
    let stdout_task = tokio::spawn(async move { read_stream_with_cap(stdout_process, true).await });
    let stderr_process = Arc::clone(&process);
    let stderr_task =
        tokio::spawn(async move { read_stream_with_cap(stderr_process, false).await });
    let wait_process = Arc::clone(&process);
    let wait_task = tokio::spawn(async move { wait_process.wait().await });

    let timed_out;
    let aborted;
    tokio::select! {
        _ = signal.cancelled() => {
            aborted = true;
            timed_out = false;
            terminate_process(&process).await;
        }
        _ = tokio::time::sleep(Duration::from_millis(DEFAULT_TIMEOUT_MS)) => {
            aborted = false;
            timed_out = true;
            terminate_process(&process).await;
        }
        _ = async {
            while process.exit_code().is_none() && !wait_task.is_finished() {
                tokio::task::yield_now().await;
            }
        } => {
            aborted = false;
            timed_out = false;
        }
    }

    let stdout = stdout_task.await;
    let stderr = stderr_task.await;
    let exit_code = wait_task.await;
    process.dispose();

    if aborted {
        return Ok(RunRgOutcome::Aborted);
    }

    let terminating = timed_out;
    let stdout = flatten_stream_task(stdout, terminating)?;
    let stderr = flatten_stream_task(stderr, terminating)?;
    let exit_code = exit_code
        .ok()
        .and_then(Result::ok)
        .unwrap_or_else(|| process.exit_code().unwrap_or(0));
    Ok(RunRgOutcome::Result(RunRgResult {
        exit_code,
        stdout_text: stdout.text,
        stderr_text: stderr.text,
        buffer_truncated: stdout.truncated,
        timed_out,
    }))
}

async fn terminate_process(process: &Arc<dyn HostProcess>) {
    let _ = process.kill(Some(ProcessSignal::Terminate)).await;
    if tokio::time::timeout(Duration::from_millis(SIGTERM_GRACE_MS), process.wait())
        .await
        .is_err()
        && process.exit_code().is_none()
    {
        let _ = process.kill(Some(ProcessSignal::Kill)).await;
    }
    process.dispose();
}

#[derive(Clone, Debug)]
struct CappedStreamResult {
    text: String,
    truncated: bool,
}

async fn read_stream_with_cap(
    process: Arc<dyn HostProcess>,
    stdout: bool,
) -> std::io::Result<CappedStreamResult> {
    let stream = if stdout {
        process.stdout()
    } else {
        process.stderr()
    };
    let mut stream = stream.lock().await;
    let mut buffer = vec![0_u8; 8 * 1024];
    let mut collected = Vec::new();
    let mut truncated = false;
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        if collected.len() < MAX_OUTPUT_BYTES {
            let remaining = MAX_OUTPUT_BYTES - collected.len();
            collected.extend_from_slice(&buffer[..read.min(remaining)]);
            if read > remaining {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }
    Ok(CappedStreamResult {
        text: String::from_utf8_lossy(&collected).into_owned(),
        truncated,
    })
}

fn flatten_stream_task(
    result: Result<std::io::Result<CappedStreamResult>, tokio::task::JoinError>,
    suppress_error: bool,
) -> Result<CappedStreamResult, Box<dyn Error + Send + Sync>> {
    match result {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) | Err(_) if suppress_error => Ok(CappedStreamResult {
            text: String::new(),
            truncated: false,
        }),
        Ok(Err(error)) => Err(Box::new(error)),
        Err(error) => Err(Box::new(error)),
    }
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

    #[test]
    fn retry_requires_non_search_failure_eagain_without_timeout() {
        let mut result = RunRgResult {
            exit_code: 2,
            stdout_text: String::new(),
            stderr_text: "Resource temporarily unavailable".into(),
            buffer_truncated: false,
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
