//! Background-task cancellation tool.
//!
//! Original: `packages/agent-core-v2/src/agent/task/tools/task-stop.ts`.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::{
        task::{
            AGENT_TASK_SERVICE_ID, AgentTaskInfo, AgentTaskServiceHandle, AgentTaskServiceResult,
        },
        tool_registry::{ToolContributionOptions, register_tool},
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema, rule_match::matches_glob_rule_subject,
    },
};

const TASK_STOP_DESCRIPTION: &str = include_str!("task-stop.md");
const DEFAULT_STOP_REASON: &str = "Stopped by TaskStop";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskStopInput {
    pub task_id: String,
    pub reason: Option<String>,
}

impl<'de> Deserialize<'de> for TaskStopInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_task_stop_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_task_stop_input(value: &Value) -> Result<TaskStopInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "TaskStop input must be an object".to_owned())?;
    let task_id = object
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "task_id must be a string".to_owned())?
        .to_owned();
    let reason = match object.get("reason") {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err("reason must be a string".into()),
    };
    Ok(TaskStopInput { task_id, reason })
}

pub static TASK_STOP_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The background task ID to stop."
                },
                "reason": {
                    "type": "string",
                    "default": DEFAULT_STOP_REASON,
                    "description": "Short reason recorded when the task is stopped."
                }
            },
            "required": ["task_id"]
        })
        .as_object()
        .cloned()
        .expect("TaskStop schema is an object"),
    )
});

#[async_trait]
pub trait TaskStopProvider: Send + Sync {
    fn get_task(&self, task_id: &str) -> Option<AgentTaskInfo>;
    async fn suppress_terminal_notification(&self, task_id: &str) -> AgentTaskServiceResult<()>;
    async fn stop(
        &self,
        task_id: &str,
        reason: &str,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>>;
}

#[async_trait]
impl TaskStopProvider for AgentTaskServiceHandle {
    fn get_task(&self, task_id: &str) -> Option<AgentTaskInfo> {
        (**self).get_task(task_id)
    }

    async fn suppress_terminal_notification(&self, task_id: &str) -> AgentTaskServiceResult<()> {
        (**self).suppress_terminal_notification(task_id).await
    }

    async fn stop(
        &self,
        task_id: &str,
        reason: &str,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        (**self).stop(task_id, Some(reason)).await
    }
}

pub struct TaskStopTool {
    tasks: Arc<dyn TaskStopProvider>,
    definition: Tool,
}

impl TaskStopTool {
    pub fn new(tasks: Arc<dyn TaskStopProvider>) -> Self {
        Self {
            tasks,
            definition: Tool {
                name: "TaskStop".into(),
                description: TASK_STOP_DESCRIPTION.into(),
                parameters: TASK_STOP_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }

    pub fn from_task_service(tasks: AgentTaskServiceHandle) -> Self {
        Self::new(Arc::new(tasks))
    }
}

#[async_trait]
impl ExecutableTool for TaskStopTool {
    type Input = TaskStopInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    // Original: task-stop.ts, TaskStopTool.resolveExecution().
    async fn resolve_execution(&self, args: TaskStopInput) -> ToolExecution {
        let description = format!("Stopping task {}", args.task_id);
        let rule_task_id = args.task_id.clone();
        let tasks = Arc::clone(&self.tasks);
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let args = args.clone();
            let tasks = Arc::clone(&tasks);
            Box::pin(async move { execute_stop(tasks.as_ref(), args).await })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("TaskStop", execute);
        execution.description = Some(description);
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, &rule_task_id)
        }));
        ToolExecution::Runnable(execution)
    }
}

async fn execute_stop(tasks: &dyn TaskStopProvider, args: TaskStopInput) -> ExecutableToolResult {
    let Some(info) = tasks.get_task(&args.task_id) else {
        return ExecutableToolResult::error(format!("Task not found: {}", args.task_id));
    };
    let reason = normalized_reason(args.reason.as_deref());
    if info.base.status.is_terminal() {
        return ExecutableToolResult::success(format!(
            "task_id: {}\nstatus: {}\nreason: {}",
            info.base.task_id,
            status_name(info.base.status),
            terminal_stop_reason(info.base.stop_reason.as_deref())
        ));
    }
    if let Err(error) = tasks.suppress_terminal_notification(&args.task_id).await {
        return ExecutableToolResult::error(error.to_string());
    }
    let result = match tasks.stop(&args.task_id, &reason).await {
        Ok(result) => result,
        Err(error) => return ExecutableToolResult::error(error.to_string()),
    };
    let Some(result) = result else {
        return ExecutableToolResult::error(format!("Failed to stop task: {}", args.task_id));
    };
    ExecutableToolResult::success(format!(
        "task_id: {}\nstatus: {}\nreason: {}",
        result.base.task_id,
        status_name(result.base.status),
        result.base.stop_reason.as_deref().unwrap_or(&reason)
    ))
}

fn normalized_reason(reason: Option<&str>) -> String {
    reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or(DEFAULT_STOP_REASON)
        .to_owned()
}

fn terminal_stop_reason(reason: Option<&str>) -> &str {
    reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or("Task already in terminal state")
}

fn status_name(status: crate::agent::task::AgentTaskStatus) -> &'static str {
    match status {
        crate::agent::task::AgentTaskStatus::Running => "running",
        crate::agent::task::AgentTaskStatus::Completed => "completed",
        crate::agent::task::AgentTaskStatus::Failed => "failed",
        crate::agent::task::AgentTaskStatus::TimedOut => "timed_out",
        crate::agent::task::AgentTaskStatus::Killed => "killed",
        crate::agent::task::AgentTaskStatus::Lost => "lost",
    }
}

// Original: task-stop.ts, registerTool(TaskStopTool).
pub fn register_task_stop_tool() {
    register_tool(
        Arc::new(|accessor| {
            let tasks = accessor.get(AGENT_TASK_SERVICE_ID)?;
            Ok(Arc::new(TaskStopTool::from_task_service((*tasks).clone())))
        }),
        ToolContributionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        agent::task::{AgentTaskInfoBase, AgentTaskStatus},
        tool::{ExecutableToolOutput, ToolExecution},
    };

    fn task(status: AgentTaskStatus, stop_reason: Option<&str>) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: "bash-12345678".into(),
                description: "command".into(),
                status,
                detached: Some(true),
                started_at: 10,
                ended_at: status.is_terminal().then_some(20),
                stop_reason: stop_reason.map(str::to_owned),
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::new(),
        }
    }

    struct StubTasks {
        current: AgentTaskInfo,
        stopped: Option<AgentTaskInfo>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TaskStopProvider for StubTasks {
        fn get_task(&self, _task_id: &str) -> Option<AgentTaskInfo> {
            self.calls.lock().push("get".into());
            Some(self.current.clone())
        }

        async fn suppress_terminal_notification(
            &self,
            _task_id: &str,
        ) -> AgentTaskServiceResult<()> {
            self.calls.lock().push("suppress".into());
            Ok(())
        }

        async fn stop(
            &self,
            _task_id: &str,
            reason: &str,
        ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
            self.calls.lock().push(format!("stop:{reason}"));
            Ok(self.stopped.clone())
        }
    }

    fn context() -> ExecutableToolContext {
        ExecutableToolContext {
            turn_id: crate::agent::TurnId::new(1),
            tool_call_id: "call-1".into(),
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
            on_foreground_task_start: None,
        }
    }

    #[test]
    fn input_schema_requires_task_id_and_rejects_null_reason() {
        assert_eq!(
            parse_task_stop_input(&json!({"task_id": "bash-12345678"})).unwrap(),
            TaskStopInput {
                task_id: "bash-12345678".into(),
                reason: None
            }
        );
        for invalid in [
            json!({}),
            json!({"task_id": 1}),
            json!({"task_id": "x", "reason": null}),
        ] {
            assert!(parse_task_stop_input(&invalid).is_err(), "{invalid}");
        }
        assert_eq!(TASK_STOP_PARAMETERS["required"], json!(["task_id"]));
        assert_eq!(TASK_STOP_PARAMETERS["additionalProperties"], false);
    }

    #[tokio::test]
    async fn terminal_task_returns_existing_status_without_side_effects() {
        let tasks = Arc::new(StubTasks {
            current: task(AgentTaskStatus::Completed, Some("  already done  ")),
            stopped: None,
            calls: Mutex::new(Vec::new()),
        });
        let tool = TaskStopTool::new(tasks.clone());
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(TaskStopInput {
                task_id: "bash-12345678".into(),
                reason: Some("ignored".into()),
            })
            .await
        else {
            panic!("expected runnable execution")
        };
        assert!(execution.matches_rule("bash-*"));
        let result = execution.execute(context()).await;
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text(
                "task_id: bash-12345678\nstatus: completed\nreason: already done".into()
            )
        );
        assert_eq!(*tasks.calls.lock(), ["get"]);
    }

    #[tokio::test]
    async fn running_task_suppresses_then_stops_with_normalized_reason() {
        let tasks = Arc::new(StubTasks {
            current: task(AgentTaskStatus::Running, None),
            stopped: Some(task(AgentTaskStatus::Killed, None)),
            calls: Mutex::new(Vec::new()),
        });
        let tool = TaskStopTool::new(tasks.clone());
        assert_eq!(
            ExecutableTool::tool(&tool).description,
            TASK_STOP_DESCRIPTION
        );
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(TaskStopInput {
                task_id: "bash-12345678".into(),
                reason: Some("   ".into()),
            })
            .await
        else {
            panic!("expected runnable execution")
        };
        let result = execution.execute(context()).await;
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text(
                "task_id: bash-12345678\nstatus: killed\nreason: Stopped by TaskStop".into()
            )
        );
        assert_eq!(
            *tasks.calls.lock(),
            ["get", "suppress", "stop:Stopped by TaskStop"]
        );
    }
}
