//! Process collection helper used by session filesystem operations.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/fsProcess.ts`.

use std::collections::HashMap;

use tokio::io::AsyncReadExt;

use crate::{
    _base::utils::abort::AbortSignal,
    os::interface::host_process::{ProcessSignal, SharedProcessReader},
    session::process::{
        ProcessExecOptions, SessionProcessRunnerContract, SessionProcessRunnerError,
    },
};

#[derive(Clone, Default)]
pub struct RunCommandOptions {
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub async fn run_command(
    runner: &dyn SessionProcessRunnerContract,
    args: &[String],
    options: RunCommandOptions,
) -> Result<RunResult, SessionProcessRunnerError> {
    let process = runner
        .exec(
            args,
            Some(ProcessExecOptions {
                cwd: options.cwd,
                env: options.env,
            }),
        )
        .await?;
    let abort_task = if let Some(signal) = options.signal {
        if signal.aborted() {
            let _ = process.kill(Some(ProcessSignal::Kill)).await;
            None
        } else {
            let process = process.clone();
            Some(tokio::spawn(async move {
                signal.cancelled().await;
                let _ = process.kill(Some(ProcessSignal::Kill)).await;
            }))
        }
    } else {
        None
    };
    let stdout = read_stream(process.stdout());
    let stderr = read_stream(process.stderr());
    let wait = process.wait();
    let (stdout, stderr, exit_code) = tokio::join!(stdout, stderr, wait);
    if let Some(task) = abort_task {
        task.abort();
    }
    Ok(RunResult {
        exit_code: exit_code.unwrap_or(-1),
        stdout: stdout.map_err(|error| Box::new(error) as SessionProcessRunnerError)?,
        stderr: stderr.map_err(|error| Box::new(error) as SessionProcessRunnerError)?,
    })
}

pub async fn read_stream(stream: SharedProcessReader) -> std::io::Result<String> {
    let mut stream = stream.lock().await;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use async_trait::async_trait;
    use tokio::{
        io::{AsyncWriteExt, duplex, sink},
        sync::Mutex as AsyncMutex,
    };

    use crate::{
        _base::utils::abort::AbortController,
        os::interface::host_process::{
            HostProcess, HostProcessError, SharedProcessReader, SharedProcessWriter,
        },
        session::process::{SessionProcess, SessionProcessRunnerResult},
    };

    use super::*;

    struct Process {
        stdout: SharedProcessReader,
        stderr: SharedProcessReader,
        exit_code: i32,
        killed: AtomicBool,
    }

    #[async_trait]
    impl HostProcess for Process {
        fn pid(&self) -> i64 {
            1
        }
        fn exit_code(&self) -> Option<i32> {
            Some(self.exit_code)
        }
        fn stdin(&self) -> SharedProcessWriter {
            Arc::new(AsyncMutex::new(Box::new(sink())))
        }
        fn stdout(&self) -> SharedProcessReader {
            Arc::clone(&self.stdout)
        }
        fn stderr(&self) -> SharedProcessReader {
            Arc::clone(&self.stderr)
        }
        async fn wait(&self) -> Result<i32, HostProcessError> {
            Ok(self.exit_code)
        }
        async fn kill(&self, _: Option<ProcessSignal>) -> Result<(), HostProcessError> {
            self.killed.store(true, Ordering::Release);
            Ok(())
        }
        fn dispose(&self) {}
    }

    struct Runner {
        process: Arc<Process>,
        calls: Mutex<Vec<(Vec<String>, Option<ProcessExecOptions>)>>,
    }

    #[async_trait]
    impl SessionProcessRunnerContract for Runner {
        async fn exec(
            &self,
            args: &[String],
            options: Option<ProcessExecOptions>,
        ) -> SessionProcessRunnerResult<SessionProcess> {
            self.calls.lock().push((args.to_vec(), options));
            Ok(self.process.clone())
        }
    }

    async fn reader(value: &str) -> SharedProcessReader {
        let (mut writer, reader) = duplex(value.len().max(1));
        writer.write_all(value.as_bytes()).await.unwrap();
        drop(writer);
        Arc::new(AsyncMutex::new(Box::new(reader)))
    }

    #[tokio::test]
    async fn collects_streams_passes_options_and_kills_on_abort() {
        let process = Arc::new(Process {
            stdout: reader("hello").await,
            stderr: reader("warn").await,
            exit_code: 7,
            killed: AtomicBool::new(false),
        });
        let runner = Runner {
            process: Arc::clone(&process),
            calls: Mutex::new(Vec::new()),
        };
        let controller = AbortController::new();
        controller.abort(None);
        let result = run_command(
            &runner,
            &["git".into(), "status".into()],
            RunCommandOptions {
                cwd: Some("/repo".into()),
                env: Some(HashMap::from([("FOO".into(), "1".into())])),
                signal: Some(controller.signal()),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            RunResult {
                exit_code: 7,
                stdout: "hello".into(),
                stderr: "warn".into()
            }
        );
        assert!(process.killed.load(Ordering::Acquire));
        let calls = runner.calls.lock();
        assert_eq!(calls[0].0, ["git", "status"]);
        assert_eq!(calls[0].1.as_ref().unwrap().cwd.as_deref(), Some("/repo"));
        assert_eq!(
            calls[0].1.as_ref().unwrap().env.as_ref().unwrap()["FOO"],
            "1"
        );
    }
}
