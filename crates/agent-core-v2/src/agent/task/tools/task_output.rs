//! Managed-task output snapshot tool.
//!
//! Original: `packages/agent-core-v2/src/agent/task/tools/task-output.ts`.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::{di::instantiation::ServicesAccessorExt, utils::abort::AbortSignal},
    agent::{
        task::{
            AGENT_TASK_SERVICE_ID, AgentTaskInfo, AgentTaskOutputSnapshot, AgentTaskServiceHandle,
            AgentTaskServiceResult, AgentTaskStatus,
        },
        tool_registry::{ToolContributionOptions, register_tool},
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema, rule_match::matches_glob_rule_subject,
    },
};

use super::format_plain_object;

const TASK_OUTPUT_DESCRIPTION: &str = include_str!("task-output.md");
const OUTPUT_PREVIEW_BYTES: usize = 32 * 1024;
const PAGING_HINT_LINES: usize = 300;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskOutputInput {
    pub task_id: String,
    pub block: Option<bool>,
    pub timeout: Option<u64>,
}

impl<'de> Deserialize<'de> for TaskOutputInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_task_output_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_task_output_input(value: &Value) -> Result<TaskOutputInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "TaskOutput input must be an object".to_owned())?;
    let task_id = object
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "task_id must be a string".to_owned())?
        .to_owned();
    let block = match object.get("block") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err("block must be a boolean".into()),
    };
    let timeout = match object.get("timeout") {
        None => None,
        Some(value) => {
            let value = value
                .as_u64()
                .ok_or_else(|| "timeout must be an integer from 0 through 3600".to_owned())?;
            if value > 3600 {
                return Err("timeout must be an integer from 0 through 3600".into());
            }
            Some(value)
        }
    };
    Ok(TaskOutputInput {
        task_id,
        block,
        timeout,
    })
}

pub static TASK_OUTPUT_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The background task ID to inspect."
                },
                "block": {
                    "type": "boolean",
                    "default": false,
                    "description": "Whether to wait for the task to finish before returning. Discouraged — background tasks notify automatically on completion; use only when the user explicitly asked you to wait."
                },
                "timeout": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 3600,
                    "default": 30,
                    "description": "Maximum number of seconds to wait when block=true."
                }
            },
            "required": ["task_id"]
        })
        .as_object()
        .cloned()
        .expect("TaskOutput schema is an object"),
    )
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetrievalStatus {
    Success,
    Timeout,
    NotReady,
}

impl RetrievalStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Timeout => "timeout",
            Self::NotReady => "not_ready",
        }
    }
}

fn retrieval_status(status: AgentTaskStatus, block: Option<bool>) -> RetrievalStatus {
    if status.is_terminal() {
        RetrievalStatus::Success
    } else if block == Some(true) {
        RetrievalStatus::Timeout
    } else {
        RetrievalStatus::NotReady
    }
}

fn terminal_reason(info: &AgentTaskInfo) -> Option<&'static str> {
    match info.base.status {
        AgentTaskStatus::TimedOut => Some("timed_out"),
        AgentTaskStatus::Killed if info.base.stop_reason.is_some() => Some("stopped"),
        AgentTaskStatus::Failed if info.base.stop_reason.is_some() => Some("failed"),
        _ => None,
    }
}

fn full_output_hint(output: &AgentTaskOutputSnapshot) -> Option<String> {
    if !output.full_output_available || output.output_path.is_none() {
        return None;
    }
    if output.truncated {
        Some(format!(
            "Only the last {OUTPUT_PREVIEW_BYTES} bytes are shown above. Use the Read tool with the output_path to page through the full log (parameters: path, line_offset, n_lines; read about {PAGING_HINT_LINES} lines per page)."
        ))
    } else {
        Some(format!(
            "The preview above is the complete output. Use the Read tool with the output_path if you need to re-read the full log later (parameters: path, line_offset, n_lines; read about {PAGING_HINT_LINES} lines per page)."
        ))
    }
}

#[async_trait]
pub trait TaskOutputProvider: Send + Sync {
    fn get_task(&self, task_id: &str) -> Option<AgentTaskInfo>;
    async fn wait(
        &self,
        task_id: &str,
        timeout_ms: f64,
        signal: AbortSignal,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>>;
    async fn get_output_snapshot(
        &self,
        task_id: &str,
        max_preview_bytes: f64,
    ) -> AgentTaskServiceResult<AgentTaskOutputSnapshot>;
}

#[async_trait]
impl TaskOutputProvider for AgentTaskServiceHandle {
    fn get_task(&self, task_id: &str) -> Option<AgentTaskInfo> {
        (**self).get_task(task_id)
    }

    async fn wait(
        &self,
        task_id: &str,
        timeout_ms: f64,
        signal: AbortSignal,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        (**self).wait(task_id, Some(timeout_ms), Some(signal)).await
    }

    async fn get_output_snapshot(
        &self,
        task_id: &str,
        max_preview_bytes: f64,
    ) -> AgentTaskServiceResult<AgentTaskOutputSnapshot> {
        (**self)
            .get_output_snapshot(task_id, max_preview_bytes)
            .await
    }
}

pub struct TaskOutputTool {
    tasks: Arc<dyn TaskOutputProvider>,
    definition: Tool,
}

impl TaskOutputTool {
    pub fn new(tasks: Arc<dyn TaskOutputProvider>) -> Self {
        Self {
            tasks,
            definition: Tool {
                name: "TaskOutput".into(),
                description: TASK_OUTPUT_DESCRIPTION.into(),
                parameters: TASK_OUTPUT_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }

    pub fn from_task_service(tasks: AgentTaskServiceHandle) -> Self {
        Self::new(Arc::new(tasks))
    }
}

#[async_trait]
impl ExecutableTool for TaskOutputTool {
    type Input = TaskOutputInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    // Original: task-output.ts, TaskOutputTool.resolveExecution().
    async fn resolve_execution(&self, args: TaskOutputInput) -> ToolExecution {
        let description = format!("Reading output of task {}", args.task_id);
        let rule_task_id = args.task_id.clone();
        let tasks = Arc::clone(&self.tasks);
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let tasks = Arc::clone(&tasks);
            let args = args.clone();
            Box::pin(async move { execute_output(tasks.as_ref(), args, context.signal).await })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("TaskOutput", execute);
        execution.description = Some(description);
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, &rule_task_id)
        }));
        ToolExecution::Runnable(execution)
    }
}

async fn execute_output(
    tasks: &dyn TaskOutputProvider,
    args: TaskOutputInput,
    signal: AbortSignal,
) -> ExecutableToolResult {
    let Some(info) = tasks.get_task(&args.task_id) else {
        return ExecutableToolResult::error(format!("Task not found: {}", args.task_id));
    };
    if args.block == Some(true)
        && !info.base.status.is_terminal()
        && let Err(error) = tasks
            .wait(
                &args.task_id,
                args.timeout.unwrap_or(30) as f64 * 1000.0,
                signal,
            )
            .await
    {
        return ExecutableToolResult::error(error.to_string());
    }
    let Some(current) = tasks.get_task(&args.task_id) else {
        return ExecutableToolResult::error(format!("Task not found: {}", args.task_id));
    };
    let output = match tasks
        .get_output_snapshot(&args.task_id, OUTPUT_PREVIEW_BYTES as f64)
        .await
    {
        Ok(output) => output,
        Err(error) => return ExecutableToolResult::error(error.to_string()),
    };
    ExecutableToolResult::success(render_output(&current, &output, args.block))
}

fn render_output(
    current: &AgentTaskInfo,
    output: &AgentTaskOutputSnapshot,
    block: Option<bool>,
) -> String {
    let mut fields = Map::new();
    fields.insert(
        "retrievalStatus".into(),
        Value::String(retrieval_status(current.base.status, block).as_str().into()),
    );
    fields.extend(
        serde_json::to_value(current)
            .expect("AgentTaskInfo is serializable")
            .as_object()
            .expect("AgentTaskInfo serializes as an object")
            .clone(),
    );
    if let Some(path) = &output.output_path {
        fields.insert("outputPath".into(), Value::String(path.clone()));
    }
    if let Some(reason) = terminal_reason(current) {
        fields.insert("terminalReason".into(), Value::String(reason.into()));
    }
    fields.insert(
        "outputSizeBytes".into(),
        Value::from(output.output_size_bytes),
    );
    fields.insert(
        "outputPreviewBytes".into(),
        Value::from(output.preview_bytes),
    );
    fields.insert("outputTruncated".into(), Value::Bool(output.truncated));
    fields.insert(
        "fullOutputAvailable".into(),
        Value::Bool(output.full_output_available),
    );
    if output.full_output_available && output.output_path.is_some() {
        fields.insert("fullOutputTool".into(), Value::String("Read".into()));
    }
    if let Some(hint) = full_output_hint(output) {
        fields.insert("fullOutputHint".into(), Value::String(hint));
    }
    if block == Some(true) && !current.base.status.is_terminal() {
        fields.insert(
            "nextStep".into(),
            Value::String("The task is still running after waiting. Do not block on it again — continue with other work or hand back to the user; you will be notified automatically when it completes.".into()),
        );
    }
    let mut lines = vec![format_plain_object(&fields), String::new()];
    if output.truncated {
        lines.push(match &output.output_path {
            Some(path) if output.full_output_available => {
                format!("[Truncated. Full output: {path}]")
            }
            _ => "[Truncated. No persisted full log is available for this task.]".into(),
        });
    }
    lines.push("[output]".into());
    lines.push(if output.preview.is_empty() {
        "[no output available]".into()
    } else {
        output.preview.clone()
    });
    lines.join("\n")
}

// Original: task-output.ts, registerTool(TaskOutputTool).
pub fn register_task_output_tool() {
    register_tool(
        Arc::new(|accessor| {
            let tasks = accessor.get(AGENT_TASK_SERVICE_ID)?;
            Ok(Arc::new(TaskOutputTool::from_task_service(
                (*tasks).clone(),
            )))
        }),
        ToolContributionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        agent::task::AgentTaskInfoBase,
        tool::{ExecutableToolOutput, ToolExecution},
    };

    fn task(status: AgentTaskStatus, reason: Option<&str>) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: "bash-12345678".into(),
                description: "command".into(),
                status,
                detached: Some(true),
                started_at: 10,
                ended_at: status.is_terminal().then_some(20),
                stop_reason: reason.map(str::to_owned),
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::new(),
        }
    }

    struct StubTasks {
        current: AgentTaskInfo,
        calls: Mutex<Vec<String>>,
        output: AgentTaskOutputSnapshot,
    }

    #[async_trait]
    impl TaskOutputProvider for StubTasks {
        fn get_task(&self, _task_id: &str) -> Option<AgentTaskInfo> {
            self.calls.lock().unwrap().push("get".into());
            Some(self.current.clone())
        }
        async fn wait(
            &self,
            _task_id: &str,
            timeout_ms: f64,
            _signal: AbortSignal,
        ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("wait:{timeout_ms}"));
            Ok(Some(self.current.clone()))
        }
        async fn get_output_snapshot(
            &self,
            _task_id: &str,
            max_preview_bytes: f64,
        ) -> AgentTaskServiceResult<AgentTaskOutputSnapshot> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("snapshot:{max_preview_bytes}"));
            Ok(self.output.clone())
        }
    }

    #[test]
    fn input_and_status_helpers_match_source_boundaries() {
        assert_eq!(
            parse_task_output_input(&json!({"task_id":"x"})).unwrap(),
            TaskOutputInput {
                task_id: "x".into(),
                block: None,
                timeout: None
            }
        );
        for invalid in [
            json!({}),
            json!({"task_id":"x","block":null}),
            json!({"task_id":"x","timeout":3601}),
            json!({"task_id":"x","timeout":1.5}),
        ] {
            assert!(parse_task_output_input(&invalid).is_err(), "{invalid}");
        }
        assert_eq!(
            retrieval_status(AgentTaskStatus::Running, None),
            RetrievalStatus::NotReady
        );
        assert_eq!(
            retrieval_status(AgentTaskStatus::Running, Some(true)),
            RetrievalStatus::Timeout
        );
        assert_eq!(
            retrieval_status(AgentTaskStatus::Killed, None),
            RetrievalStatus::Success
        );
    }

    #[tokio::test]
    async fn blocking_running_snapshot_waits_and_renders_truncation_guidance() {
        let tasks = Arc::new(StubTasks {
            current: task(AgentTaskStatus::Running, None),
            calls: Mutex::new(Vec::new()),
            output: AgentTaskOutputSnapshot {
                output_path: Some("/tmp/output.log".into()),
                output_size_bytes: 40_000,
                preview_bytes: OUTPUT_PREVIEW_BYTES,
                truncated: true,
                full_output_available: true,
                preview: "tail".into(),
            },
        });
        let tool = TaskOutputTool::new(tasks.clone());
        assert_eq!(
            ExecutableTool::tool(&tool).description,
            TASK_OUTPUT_DESCRIPTION
        );
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(TaskOutputInput {
                task_id: "bash-12345678".into(),
                block: Some(true),
                timeout: None,
            })
            .await
        else {
            panic!("runnable")
        };
        assert!(execution.matches_rule("bash-*"));
        let result = execution
            .execute(ExecutableToolContext {
                turn_id: crate::agent::TurnId::new(1),
                tool_call_id: "c".into(),
                trace: None,
                metadata: None,
                signal: AbortController::new().signal(),
                on_update: None,
                on_foreground_task_start: None,
            })
            .await;
        let ExecutableToolOutput::Text(text) = result.output else {
            panic!("text")
        };
        assert!(text.contains("retrieval_status: timeout"));
        assert!(text.contains("full_output_tool: Read"));
        assert!(text.contains("[Truncated. Full output: /tmp/output.log]\n[output]\ntail"));
        assert!(text.contains("Do not block on it again"));
        assert_eq!(
            *tasks.calls.lock().unwrap(),
            ["get", "wait:30000", "get", "snapshot:32768"]
        );
    }

    #[test]
    fn terminal_reason_and_full_output_hints_preserve_categories() {
        assert_eq!(
            terminal_reason(&task(AgentTaskStatus::TimedOut, None)),
            Some("timed_out")
        );
        assert_eq!(
            terminal_reason(&task(AgentTaskStatus::Killed, Some("stop"))),
            Some("stopped")
        );
        assert_eq!(terminal_reason(&task(AgentTaskStatus::Failed, None)), None);
        assert!(
            full_output_hint(&AgentTaskOutputSnapshot {
                output_path: None,
                full_output_available: true,
                ..AgentTaskOutputSnapshot::default()
            })
            .is_none()
        );
    }
}
