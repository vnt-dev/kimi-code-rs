//! Host-process adapter for Agent-managed tasks.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/tools/process-task.ts`.

use std::{
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::io::AsyncReadExt;

use crate::{
    _base::utils::abort::{AbortError, AbortSignal},
    agent::task::types::{
        AgentTask, AgentTaskError, AgentTaskInfo, AgentTaskInfoBase, AgentTaskSettlement,
        AgentTaskSettlementStatus, AgentTaskSink,
    },
    os::interface::host_process::{
        HostProcess, HostProcessError, ProcessSignal, SharedProcessReader,
    },
};

const STREAM_DRAIN_GRACE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessTaskOutputKind {
    Stdout,
    Stderr,
}

pub type ProcessTaskOutputCallback = Arc<dyn Fn(ProcessTaskOutputKind, &str) + Send + Sync>;

pub struct ProcessTask {
    process: Arc<dyn HostProcess>,
    command: String,
    description: String,
    on_output: Option<ProcessTaskOutputCallback>,
    exit_code: Mutex<Option<i32>>,
}

impl ProcessTask {
    pub fn new(
        process: Arc<dyn HostProcess>,
        command: impl Into<String>,
        description: impl Into<String>,
        on_output: Option<ProcessTaskOutputCallback>,
    ) -> Self {
        Self {
            process,
            command: command.into(),
            description: description.into(),
            on_output,
            exit_code: Mutex::new(None),
        }
    }

    pub fn process(&self) -> &Arc<dyn HostProcess> {
        &self.process
    }

    fn dispose_process(&self) {
        self.process.dispose();
    }
}

#[async_trait::async_trait]
impl AgentTask for ProcessTask {
    fn id_prefix(&self) -> &str {
        "bash"
    }
    fn kind(&self) -> &str {
        "process"
    }
    fn description(&self) -> &str {
        &self.description
    }

    async fn start(&self, sink: &dyn AgentTaskSink) -> Result<(), AgentTaskError> {
        let signal = sink.signal();
        let process_for_abort = Arc::clone(&self.process);
        let abort_signal = signal.clone();
        let abort_task = tokio::spawn(async move {
            abort_signal.cancelled().await;
            let _ = process_for_abort.kill(Some(ProcessSignal::Terminate)).await;
        });

        let (wait, drain) =
            wait_and_observe(&self.process, sink, &signal, self.on_output.as_ref()).await;
        abort_task.abort();
        let settlement = match wait {
            Ok(exit_code) if drain.is_ok() => {
                *self.exit_code.lock().unwrap() = Some(exit_code);
                AgentTaskSettlement {
                    status: if signal.aborted() {
                        AgentTaskSettlementStatus::Killed
                    } else if exit_code == 0 {
                        AgentTaskSettlementStatus::Completed
                    } else {
                        AgentTaskSettlementStatus::Failed
                    },
                    stop_reason: None,
                }
            }
            result => {
                *self.exit_code.lock().unwrap() = self.process.exit_code();
                let reason = result
                    .err()
                    .map(|error| error.to_string())
                    .or_else(|| drain.err().map(|error| error.to_string()));
                AgentTaskSettlement {
                    status: if signal.aborted() {
                        AgentTaskSettlementStatus::Killed
                    } else {
                        AgentTaskSettlementStatus::Failed
                    },
                    stop_reason: (!signal.aborted()).then_some(reason).flatten(),
                }
            }
        };
        self.dispose_process();
        sink.settle(settlement).await?;
        Ok(())
    }

    async fn force_stop(&self) -> Result<(), AgentTaskError> {
        let result = if self.process.exit_code().is_none() {
            self.process
                .kill(Some(ProcessSignal::Kill))
                .await
                .map_err(|error| Box::new(error) as AgentTaskError)
        } else {
            Ok(())
        };
        self.dispose_process();
        result
    }

    fn to_info(&self, base: AgentTaskInfoBase) -> AgentTaskInfo {
        AgentTaskInfo {
            base,
            kind: "process".into(),
            details: serde_json::Map::from_iter([
                (
                    "command".into(),
                    serde_json::Value::String(self.command.clone()),
                ),
                ("pid".into(), serde_json::Value::from(self.process.pid())),
                (
                    "exitCode".into(),
                    self.exit_code
                        .lock()
                        .unwrap()
                        .map_or(serde_json::Value::Null, serde_json::Value::from),
                ),
            ]),
        }
    }
}

async fn wait_and_observe(
    process: &Arc<dyn HostProcess>,
    sink: &dyn AgentTaskSink,
    signal: &AbortSignal,
    callback: Option<&ProcessTaskOutputCallback>,
) -> (Result<i32, HostProcessError>, Result<(), std::io::Error>) {
    let drain = async {
        tokio::try_join!(
            observe_stream(
                process.stdout(),
                ProcessTaskOutputKind::Stdout,
                sink,
                signal,
                callback,
            ),
            observe_stream(
                process.stderr(),
                ProcessTaskOutputKind::Stderr,
                sink,
                signal,
                callback,
            ),
        )
        .map(|_| ())
    };
    tokio::pin!(drain);
    let wait = process.wait();
    tokio::pin!(wait);
    tokio::select! {
        wait_result = &mut wait => {
            let drain_result = tokio::time::timeout(STREAM_DRAIN_GRACE, &mut drain)
                .await
                .unwrap_or(Ok(()));
            (wait_result, drain_result)
        }
        drain_result = &mut drain => (wait.await, drain_result),
    }
}

async fn observe_stream(
    stream: SharedProcessReader,
    kind: ProcessTaskOutputKind,
    sink: &dyn AgentTaskSink,
    signal: &AbortSignal,
    callback: Option<&ProcessTaskOutputCallback>,
) -> Result<(), std::io::Error> {
    let mut stream = stream.lock().await;
    let mut buffer = [0_u8; 8192];
    let mut pending = Vec::new();
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        pending.extend_from_slice(&buffer[..count]);
        for text in decode_available(&mut pending, false) {
            forward_output(&text, kind, sink, signal, callback);
        }
    }
    for text in decode_available(&mut pending, true) {
        forward_output(&text, kind, sink, signal, callback);
    }
    Ok(())
}

fn forward_output(
    text: &str,
    kind: ProcessTaskOutputKind,
    sink: &dyn AgentTaskSink,
    signal: &AbortSignal,
    callback: Option<&ProcessTaskOutputCallback>,
) {
    if text.is_empty() {
        return;
    }
    sink.append_output(text);
    if !signal.aborted()
        && let Some(callback) = callback
    {
        callback(kind, text);
    }
}

fn decode_available(pending: &mut Vec<u8>, eof: bool) -> Vec<String> {
    let mut output = Vec::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(text) => {
                if !text.is_empty() {
                    output.push(text.to_owned());
                }
                pending.clear();
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    output.push(String::from_utf8(pending.drain(..valid).collect()).unwrap());
                    continue;
                }
                if let Some(length) = error.error_len() {
                    pending.drain(..length);
                    output.push(char::REPLACEMENT_CHARACTER.to_string());
                    continue;
                }
                if eof {
                    output.push(String::from_utf8_lossy(pending).into_owned());
                    pending.clear();
                }
                break;
            }
        }
    }
    output
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessTaskResult {
    pub exit_code: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessExecutorError {
    #[error(transparent)]
    Aborted(Arc<AbortError>),
    #[error(transparent)]
    Process(#[from] HostProcessError),
    #[error("process output failed: {0}")]
    Output(#[from] std::io::Error),
    #[error(transparent)]
    Exit(#[from] ProcessExitError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessExitError {
    pub exit_code: Option<i32>,
}

impl fmt::Display for ProcessExitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.exit_code {
            Some(code) => write!(formatter, "Process exited with code {code}"),
            None => formatter.write_str("Process exited with code null"),
        }
    }
}

impl std::error::Error for ProcessExitError {}

pub async fn execute_process(
    process: Arc<dyn HostProcess>,
    signal: &AbortSignal,
    output: &(dyn Fn(&str) + Send + Sync),
    on_output: Option<&ProcessTaskOutputCallback>,
) -> Result<ProcessTaskResult, ProcessExecutorError> {
    let sink = RawOutputSink {
        signal: signal.clone(),
        output,
    };
    let process_for_abort = Arc::clone(&process);
    let abort_signal = signal.clone();
    let abort_task = tokio::spawn(async move {
        abort_signal.cancelled().await;
        let _ = process_for_abort.kill(Some(ProcessSignal::Terminate)).await;
    });
    let (wait, drain) = wait_and_observe(&process, &sink, signal, on_output).await;
    abort_task.abort();
    process.dispose();
    drain?;
    let exit_code = wait?;
    if let Some(reason) = signal.reason() {
        return Err(ProcessExecutorError::Aborted(reason));
    }
    if exit_code != 0 {
        return Err(ProcessExitError {
            exit_code: Some(exit_code),
        }
        .into());
    }
    Ok(ProcessTaskResult {
        exit_code: Some(exit_code),
    })
}

struct RawOutputSink<'a> {
    signal: AbortSignal,
    output: &'a (dyn Fn(&str) + Send + Sync),
}

#[async_trait::async_trait]
impl AgentTaskSink for RawOutputSink<'_> {
    fn signal(&self) -> AbortSignal {
        self.signal.clone()
    }
    fn append_output(&self, chunk: &str) {
        (self.output)(chunk);
    }
    async fn settle(&self, _: AgentTaskSettlement) -> Result<bool, AgentTaskError> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        _base::utils::abort::AbortController,
        os::{
            backends::node_local::host_process_service::LocalHostProcessService,
            interface::host_process::{HostProcessOptions, HostProcessService},
        },
    };

    use super::*;

    struct Sink {
        signal: AbortSignal,
        output: Mutex<String>,
        settlement: Mutex<Option<AgentTaskSettlement>>,
    }

    #[async_trait::async_trait]
    impl AgentTaskSink for Sink {
        fn signal(&self) -> AbortSignal {
            self.signal.clone()
        }
        fn append_output(&self, chunk: &str) {
            self.output.lock().unwrap().push_str(chunk);
        }
        async fn settle(&self, settlement: AgentTaskSettlement) -> Result<bool, AgentTaskError> {
            *self.settlement.lock().unwrap() = Some(settlement);
            Ok(true)
        }
    }

    async fn shell(command: &str) -> Arc<dyn HostProcess> {
        LocalHostProcessService::default()
            .spawn(
                "/bin/sh",
                &["-c".into(), command.into()],
                HostProcessOptions::default(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn task_forwards_both_streams_settles_and_reports_info() {
        let task = ProcessTask::new(
            shell("printf out; printf err >&2").await,
            "command",
            "description",
            None,
        );
        let controller = AbortController::new();
        let sink = Sink {
            signal: controller.signal(),
            output: Mutex::new(String::new()),
            settlement: Mutex::new(None),
        };
        task.start(&sink).await.unwrap();
        let output = sink.output.lock().unwrap().clone();
        assert!(output.contains("out"));
        assert!(output.contains("err"));
        assert_eq!(
            sink.settlement.lock().unwrap().as_ref().unwrap().status,
            AgentTaskSettlementStatus::Completed
        );
        let info = task.to_info(AgentTaskInfoBase {
            task_id: "bash-1".into(),
            description: "description".into(),
            status: crate::agent::task::types::AgentTaskStatus::Completed,
            detached: None,
            started_at: 1,
            ended_at: Some(2),
            stop_reason: None,
            terminal_notification_suppressed: None,
            timeout_ms: None,
        });
        assert_eq!(info.details["exitCode"], 0);
    }

    #[tokio::test]
    async fn executor_returns_typed_nonzero_exit_error() {
        let process = shell("printf output; exit 7").await;
        let output = Mutex::new(String::new());
        let error = execute_process(
            process,
            &AbortController::new().signal(),
            &|chunk| output.lock().unwrap().push_str(chunk),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            ProcessExecutorError::Exit(ProcessExitError { exit_code: Some(7) })
        ));
        assert_eq!(*output.lock().unwrap(), "output");
    }

    #[tokio::test]
    async fn already_aborted_task_terminates_and_settles_killed() {
        let task = ProcessTask::new(shell("sleep 30").await, "sleep", "sleeping", None);
        let controller = AbortController::new();
        controller.abort(None);
        let sink = Sink {
            signal: controller.signal(),
            output: Mutex::new(String::new()),
            settlement: Mutex::new(None),
        };
        tokio::time::timeout(Duration::from_secs(3), task.start(&sink))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            sink.settlement.lock().unwrap().as_ref().unwrap().status,
            AgentTaskSettlementStatus::Killed
        );
    }
}
