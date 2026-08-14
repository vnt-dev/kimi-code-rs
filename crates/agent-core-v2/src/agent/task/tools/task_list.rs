//! Read-only background-task listing tool.
//!
//! Original: `packages/agent-core-v2/src/agent/task/tools/task-list.ts`.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::{
        task::{AGENT_TASK_SERVICE_ID, AgentTaskInfo, AgentTaskServiceHandle},
        tool_registry::{ToolContributionOptions, register_tool},
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema, rule_match::matches_glob_rule_subject,
    },
};

use super::format_plain_object;

const TASK_LIST_DESCRIPTION: &str = include_str!("task-list.md");

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskListInput {
    pub active_only: Option<bool>,
    pub limit: Option<usize>,
}

impl<'de> Deserialize<'de> for TaskListInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_task_list_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_task_list_input(value: &Value) -> Result<TaskListInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "TaskList input must be an object".to_owned())?;
    let active_only = match object.get("active_only") {
        None => Some(true),
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err("active_only must be a boolean".into()),
    };
    let limit = match object.get("limit") {
        None => None,
        Some(value) => {
            let value = value
                .as_u64()
                .ok_or_else(|| "limit must be an integer from 1 through 100".to_owned())?;
            if !(1..=100).contains(&value) {
                return Err("limit must be an integer from 1 through 100".into());
            }
            Some(value as usize)
        }
    };
    Ok(TaskListInput { active_only, limit })
}

pub static TASK_LIST_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "active_only": {
                    "type": "boolean",
                    "default": true,
                    "description": "Whether to list only non-terminal background tasks."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20,
                    "description": "Maximum number of tasks to return."
                }
            }
        })
        .as_object()
        .cloned()
        .expect("TaskList schema is an object"),
    )
});

pub trait TaskListProvider: Send + Sync {
    fn list(&self, active_only: bool, limit: usize) -> Vec<AgentTaskInfo>;
}

impl TaskListProvider for AgentTaskServiceHandle {
    fn list(&self, active_only: bool, limit: usize) -> Vec<AgentTaskInfo> {
        (**self).list(Some(active_only), Some(limit))
    }
}

pub fn format_task_list(tasks: &[AgentTaskInfo], active_only: bool) -> String {
    let label = if active_only {
        "active_background_tasks"
    } else {
        "background_tasks"
    };
    let header = format!("{label}: {}", tasks.len());
    if tasks.is_empty() {
        return format!("{header}\nNo background tasks found.");
    }
    let entries = tasks
        .iter()
        .map(|task| {
            serde_json::to_value(task)
                .expect("AgentTaskInfo is always serializable")
                .as_object()
                .map(format_plain_object)
                .expect("AgentTaskInfo serializes as an object")
        })
        .collect::<Vec<_>>()
        .join("\n---\n");
    format!("{header}\n{entries}")
}

pub struct TaskListTool {
    tasks: Arc<dyn TaskListProvider>,
    definition: Tool,
}

impl TaskListTool {
    pub fn new(tasks: Arc<dyn TaskListProvider>) -> Self {
        Self {
            tasks,
            definition: Tool {
                name: "TaskList".into(),
                description: TASK_LIST_DESCRIPTION.into(),
                parameters: TASK_LIST_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }

    pub fn from_task_service(tasks: AgentTaskServiceHandle) -> Self {
        Self::new(Arc::new(tasks))
    }
}

#[async_trait]
impl ExecutableTool for TaskListTool {
    type Input = TaskListInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    // Original: task-list.ts, TaskListTool.resolveExecution().
    async fn resolve_execution(&self, args: TaskListInput) -> ToolExecution {
        let active_only = args.active_only.unwrap_or(true);
        let limit = args.limit.unwrap_or(20);
        let list_scope = if active_only { "active" } else { "all" };
        let tasks = Arc::clone(&self.tasks);
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let tasks = Arc::clone(&tasks);
            Box::pin(async move {
                let tasks = tasks.list(active_only, limit);
                ExecutableToolResult::success(format_task_list(&tasks, active_only))
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("TaskList", execute);
        execution.description = Some("Listing background tasks".into());
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, list_scope)
        }));
        ToolExecution::Runnable(execution)
    }
}

// Original: task-list.ts, registerTool(TaskListTool).
pub fn register_task_list_tool() {
    register_tool(
        Arc::new(|accessor| {
            let tasks = accessor.get(AGENT_TASK_SERVICE_ID)?;
            Ok(Arc::new(TaskListTool::from_task_service((*tasks).clone())))
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

    fn task(task_id: &str, status: AgentTaskStatus) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: task_id.into(),
                description: "command".into(),
                status,
                detached: Some(true),
                started_at: 10,
                ended_at: None,
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::from_iter([
                ("command".into(), Value::String("pwd".into())),
                ("pid".into(), Value::from(42)),
                ("exitCode".into(), Value::Null),
            ]),
        }
    }

    #[derive(Default)]
    struct StubTasks {
        calls: Mutex<Vec<(bool, usize)>>,
        tasks: Vec<AgentTaskInfo>,
    }

    impl TaskListProvider for StubTasks {
        fn list(&self, active_only: bool, limit: usize) -> Vec<AgentTaskInfo> {
            self.calls.lock().push((active_only, limit));
            self.tasks.iter().take(limit).cloned().collect()
        }
    }

    #[test]
    fn input_parser_applies_source_defaults_and_rejects_constraints() {
        assert_eq!(
            parse_task_list_input(&json!({})).unwrap(),
            TaskListInput {
                active_only: Some(true),
                limit: None
            }
        );
        assert_eq!(
            parse_task_list_input(&json!({"active_only": false, "limit": 100})).unwrap(),
            TaskListInput {
                active_only: Some(false),
                limit: Some(100)
            }
        );
        for invalid in [
            json!(null),
            json!({"active_only": null}),
            json!({"limit": 0}),
            json!({"limit": 101}),
            json!({"limit": 1.5}),
        ] {
            assert!(parse_task_list_input(&invalid).is_err(), "{invalid}");
        }
        assert_eq!(TASK_LIST_PARAMETERS["additionalProperties"], false);
    }

    #[test]
    fn formatter_preserves_empty_and_task_record_shapes() {
        assert_eq!(
            format_task_list(&[], true),
            "active_background_tasks: 0\nNo background tasks found."
        );
        assert_eq!(
            format_task_list(&[task("bash-12345678", AgentTaskStatus::Running)], false),
            "background_tasks: 1\ntask_id: bash-12345678\ndescription: command\nstatus: running\ndetached: true\nstarted_at: 10\nkind: process\ncommand: pwd\npid: 42"
        );
    }

    #[tokio::test]
    async fn execution_uses_defaults_formats_output_and_matches_scope_rules() {
        let tasks = Arc::new(StubTasks {
            tasks: vec![task("bash-12345678", AgentTaskStatus::Running)],
            ..StubTasks::default()
        });
        let tool = TaskListTool::new(tasks.clone());
        assert_eq!(ExecutableTool::tool(&tool).name, "TaskList");
        assert_eq!(
            ExecutableTool::tool(&tool).description,
            TASK_LIST_DESCRIPTION
        );
        let ToolExecution::Runnable(execution) =
            tool.resolve_execution(TaskListInput::default()).await
        else {
            panic!("expected runnable execution")
        };
        assert_eq!(
            execution.description.as_deref(),
            Some("Listing background tasks")
        );
        assert_eq!(execution.approval_rule, "TaskList");
        assert!(execution.matches_rule("active"));
        assert!(!execution.matches_rule("all"));
        let result = execution
            .execute(ExecutableToolContext {
                turn_id: crate::agent::TurnId::new(1),
                tool_call_id: "call-1".into(),
                trace: None,
                metadata: None,
                signal: AbortController::new().signal(),
                on_update: None,
                on_foreground_task_start: None,
            })
            .await;
        assert!(!result.is_error);
        assert!(matches!(
            result.output,
            ExecutableToolOutput::Text(ref output)
                if output.starts_with("active_background_tasks: 1\n")
        ));
        assert_eq!(*tasks.calls.lock(), [(true, 20)]);
    }
}
