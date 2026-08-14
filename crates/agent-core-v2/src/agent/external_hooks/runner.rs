use parking_lot::Mutex;
use std::sync::Arc;
use std::{collections::HashMap, future::pending, time::Duration};

use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    _base::utils::abort::AbortSignal,
    os::interface::host_process::{
        HostProcess, HostProcessError, HostProcessOptions, HostProcessService, ProcessShell,
        ProcessSignal, SharedProcessReader,
    },
};

use super::types::{HookAction, HookResult};

const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const KILL_GRACE: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub struct RunHookOptions {
    pub timeout: u64,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub signal: Option<AbortSignal>,
}

// Original:
//   packages/agent-core-v2/src/agent/externalHooks/runner.ts
//   buildHookSpawnOptions()
//
// The HostProcess backend inherits the parent environment before applying
// `env`, which is equivalent to the source's `{ ...process.env, ...env }`.
pub fn build_hook_spawn_options(
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
) -> HostProcessOptions {
    HostProcessOptions {
        shell: Some(ProcessShell::Default),
        cwd,
        env,
        detached: Some(!cfg!(windows)),
        windows_hide: Some(true),
        ..HostProcessOptions::default()
    }
}

// Original: runner.ts, runHook().
pub async fn run_hook(
    host_process: &dyn HostProcessService,
    command: &str,
    input: &Map<String, Value>,
    options: RunHookOptions,
) -> HookResult {
    let process = match host_process
        .spawn(
            command,
            &[],
            build_hook_spawn_options(options.cwd, options.env),
        )
        .await
    {
        Ok(process) => process,
        Err(error) => return allow_result(None, None, Some(error.to_string()), None, None, None),
    };

    let input = Value::Object(input.clone()).to_string().into_bytes();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let completion = complete_process(
        Arc::clone(&process),
        input,
        Arc::clone(&stdout),
        Arc::clone(&stderr),
    );
    tokio::pin!(completion);

    let timeout = timeout_duration(options.timeout);
    let cancellation = async {
        match options.signal {
            Some(signal) => {
                signal.cancelled().await;
            }
            None => pending().await,
        }
    };
    tokio::pin!(cancellation);

    tokio::select! {
        biased;
        () = &mut cancellation => {
            let result = allow_result(
                None,
                Some(buffer_text(&stdout)),
                Some(buffer_text(&stderr)),
                None,
                None,
                None,
            );
            kill_process(&process).await;
            result
        }
        result = &mut completion => {
            process.dispose();
            result
        }
        () = tokio::time::sleep(timeout) => {
            let result = allow_result(
                None,
                Some(buffer_text(&stdout)),
                Some(buffer_text(&stderr)),
                None,
                Some(true),
                None,
            );
            kill_process(&process).await;
            result
        }
    }
}

async fn complete_process(
    process: Arc<dyn HostProcess>,
    input: Vec<u8>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
) -> HookResult {
    let completion = tokio::try_join!(
        async {
            write_input(process.stdin(), input).await;
            Ok::<(), HookIoError>(())
        },
        async { process.wait().await.map_err(HookIoError::Wait) },
        async {
            collect_stream(process.stdout(), Arc::clone(&stdout))
                .await
                .map_err(HookIoError::Stdout)
        },
        async {
            collect_stream(process.stderr(), Arc::clone(&stderr))
                .await
                .map_err(HookIoError::Stderr)
        },
    );
    let stdout = buffer_text(&stdout);
    let mut stderr_text = buffer_text(&stderr);

    let (_, exit_code, _, _) = match completion {
        Ok(completion) => completion,
        Err(error) => {
            stderr_text.push_str(&error.to_string());
            return allow_result(None, Some(stdout), Some(stderr_text), None, None, None);
        }
    };
    result_from_exit_code(exit_code, stdout, stderr_text)
}

#[derive(Debug, thiserror::Error)]
enum HookIoError {
    #[error(transparent)]
    Wait(#[from] HostProcessError),
    #[error(transparent)]
    Stdout(#[from] std::io::Error),
    #[error(transparent)]
    Stderr(std::io::Error),
}

async fn write_input(
    stream: crate::os::interface::host_process::SharedProcessWriter,
    input: Vec<u8>,
) {
    let mut stream = stream.lock().await;
    let _ = stream.write_all(&input).await;
    let _ = stream.shutdown().await;
    *stream = Box::new(tokio::io::sink());
}

async fn collect_stream(
    stream: SharedProcessReader,
    output: Arc<Mutex<Vec<u8>>>,
) -> Result<(), std::io::Error> {
    let mut stream = stream.lock().await;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        output.lock().extend_from_slice(&buffer[..count]);
    }
}

fn buffer_text(output: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&output.lock()).into_owned()
}

fn timeout_duration(timeout: u64) -> Duration {
    let seconds = if timeout > 0 {
        timeout
    } else {
        DEFAULT_TIMEOUT_SECONDS
    };
    Duration::from_secs(seconds)
}

// Original: runner.ts, resultFromExitCode().
fn result_from_exit_code(exit_code: i32, stdout: String, stderr: String) -> HookResult {
    if exit_code == 2 {
        let message = stderr.trim().to_owned();
        return HookResult {
            action: HookAction::Block,
            message: Some(message.clone()),
            reason: Some(message),
            stdout: Some(stdout),
            stderr: Some(stderr),
            exit_code: Some(exit_code),
            timed_out: None,
            structured_output: None,
        };
    }

    let structured = (exit_code == 0)
        .then(|| structured_output(&stdout))
        .flatten();
    if structured.as_ref().is_some_and(|output| output.block) {
        let structured = structured.unwrap();
        return HookResult {
            action: HookAction::Block,
            message: structured
                .message
                .clone()
                .or_else(|| structured.reason.clone()),
            reason: structured.reason,
            stdout: Some(stdout),
            stderr: Some(stderr),
            exit_code: Some(exit_code),
            timed_out: None,
            structured_output: Some(true),
        };
    }
    allow_result(
        structured
            .as_ref()
            .and_then(|output| output.message.clone()),
        Some(stdout),
        Some(stderr),
        Some(exit_code),
        None,
        structured.as_ref().map(|_| true),
    )
}

struct StructuredOutput {
    block: bool,
    reason: Option<String>,
    message: Option<String>,
}

// Original: runner.ts, structuredOutput().
fn structured_output(stdout: &str) -> Option<StructuredOutput> {
    let text = stdout.trim();
    if text.is_empty() {
        return None;
    }
    let object = serde_json::from_str::<Value>(text).ok()?;
    let object = object.as_object()?;
    let hook_specific = object.get("hookSpecificOutput").and_then(Value::as_object);
    let message = optional_string(object.get("message"))
        .or_else(|| hook_specific.and_then(|value| optional_string(value.get("message"))));
    let denied = hook_specific.and_then(|value| value.get("permissionDecision"))
        == Some(&Value::String("deny".into()));
    let reason = denied
        .then(|| {
            hook_specific
                .and_then(|value| value.get("permissionDecisionReason"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .flatten();
    Some(StructuredOutput {
        block: denied,
        reason,
        message,
    })
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null | Value::Array(_) | Value::Object(_) => None,
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
    }
}

fn allow_result(
    message: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
    exit_code: Option<i32>,
    timed_out: Option<bool>,
    structured_output: Option<bool>,
) -> HookResult {
    HookResult {
        action: HookAction::Allow,
        message,
        reason: None,
        stdout,
        stderr,
        exit_code,
        timed_out,
        structured_output,
    }
}

// Original: runner.ts, killProcess(). Rust keeps the grace timer inside the
// caller's async lifetime instead of creating an unmanaged background task.
async fn kill_process(process: &Arc<dyn HostProcess>) {
    let _ = process.kill(Some(ProcessSignal::Terminate)).await;
    tokio::time::sleep(KILL_GRACE).await;
    let _ = process.kill(Some(ProcessSignal::Kill)).await;
    process.dispose();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_options_preserve_shell_console_cwd_and_environment() {
        let options = build_hook_spawn_options(
            Some("/repo".into()),
            Some(HashMap::from([("FOO".into(), "bar".into())])),
        );
        assert_eq!(options.shell, Some(ProcessShell::Default));
        assert_eq!(options.cwd.as_deref(), Some("/repo"));
        assert_eq!(options.env.as_ref().unwrap()["FOO"], "bar");
        assert_eq!(options.windows_hide, Some(true));
        assert_eq!(options.detached, Some(!cfg!(windows)));
    }

    #[test]
    fn parses_structured_messages_coercions_and_denials() {
        let result = result_from_exit_code(
            0,
            r#"{"message":42,"hookSpecificOutput":{"message":"inner"}}"#.into(),
            String::new(),
        );
        assert_eq!(result.action, HookAction::Allow);
        assert_eq!(result.message.as_deref(), Some("42"));
        assert_eq!(result.structured_output, Some(true));

        let denied = result_from_exit_code(
            0,
            r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"use rg"}}"#.into(),
            String::new(),
        );
        assert_eq!(denied.action, HookAction::Block);
        assert_eq!(denied.reason.as_deref(), Some("use rg"));
        assert_eq!(denied.structured_output, Some(true));
    }

    #[test]
    fn exit_two_blocks_before_structured_parsing_and_other_failures_allow() {
        let blocked = result_from_exit_code(2, "{}".into(), " blocked\n".into());
        assert_eq!(blocked.action, HookAction::Block);
        assert_eq!(blocked.message.as_deref(), Some("blocked"));
        assert_eq!(blocked.reason.as_deref(), Some("blocked"));
        assert_eq!(blocked.structured_output, None);

        let failed = result_from_exit_code(1, "{}".into(), "bad".into());
        assert_eq!(failed.action, HookAction::Allow);
        assert_eq!(failed.structured_output, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_shell_hook_writes_json_and_captures_output() {
        let input = Map::from_iter([("tool_name".into(), Value::String("Write".into()))]);
        let result = run_hook(
            &crate::os::backends::node_local::host_process_service::LocalHostProcessService::default(),
            "cat",
            &input,
            RunHookOptions {
                timeout: 5,
                cwd: None,
                env: None,
                signal: None,
            },
        )
        .await;
        assert_eq!(result.action, HookAction::Allow);
        assert_eq!(
            serde_json::from_str::<Value>(result.stdout.as_deref().unwrap()).unwrap(),
            Value::Object(input)
        );
        assert_eq!(result.exit_code, Some(0));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_and_preaborted_signal_fail_open_without_waiting_for_exit() {
        let host =
            crate::os::backends::node_local::host_process_service::LocalHostProcessService::default(
            );
        let timed_out = run_hook(
            &host,
            "sleep 10",
            &Map::new(),
            RunHookOptions {
                timeout: 1,
                cwd: None,
                env: None,
                signal: None,
            },
        )
        .await;
        assert_eq!(timed_out.action, HookAction::Allow);
        assert_eq!(timed_out.timed_out, Some(true));

        let controller = crate::_base::utils::abort::AbortController::new();
        controller.abort(None);
        let aborted = run_hook(
            &host,
            "sleep 10",
            &Map::new(),
            RunHookOptions {
                timeout: 5,
                cwd: None,
                env: None,
                signal: Some(controller.signal()),
            },
        )
        .await;
        assert_eq!(aborted.action, HookAction::Allow);
        assert_eq!(aborted.timed_out, None);
    }
}
