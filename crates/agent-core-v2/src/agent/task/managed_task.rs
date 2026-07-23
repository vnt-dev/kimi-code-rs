//! Source-specific projection of live task state into `AgentTaskInfo`.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `toInfo()` and the source distinction used by `appendOutput()`.

use std::sync::Arc;

use super::{AgentTask, AgentTaskInfo, AgentTaskInfoBase, AgentTaskStatus, AgentTaskToInfo};

pub enum ManagedTaskInfoProjection {
    Registered(Arc<dyn AgentTask>),
    Tracked {
        description: String,
        to_info: AgentTaskToInfo,
    },
}

impl ManagedTaskInfoProjection {
    pub fn registered(task: Arc<dyn AgentTask>) -> Self {
        Self::Registered(task)
    }

    pub fn tracked(description: impl Into<String>, to_info: AgentTaskToInfo) -> Self {
        Self::Tracked {
            description: description.into(),
            to_info,
        }
    }

    // Original: taskService.ts, appendOutput() `entry.task?.kind ===
    // 'process'`. Tracked handles do not enforce this limit even if their
    // projected AgentTaskInfo kind is `process`.
    pub fn enforces_process_output_limit(&self) -> bool {
        matches!(self, Self::Registered(task) if task.kind() == "process")
    }

    // Original: taskService.ts, toInfo().
    #[allow(clippy::too_many_arguments)]
    pub fn to_info(
        &self,
        task_id: &str,
        status: AgentTaskStatus,
        detached: bool,
        started_at: i64,
        ended_at: Option<i64>,
        stop_reason: Option<String>,
        terminal_notification_suppressed: Option<bool>,
        timeout_ms: Option<u64>,
    ) -> AgentTaskInfo {
        let description = match self {
            Self::Registered(task) => task.description().to_owned(),
            Self::Tracked { description, .. } => description.clone(),
        };
        let base = AgentTaskInfoBase {
            task_id: task_id.into(),
            description,
            status,
            detached: Some(detached),
            started_at,
            ended_at,
            stop_reason,
            terminal_notification_suppressed,
            timeout_ms,
        };
        match self {
            Self::Registered(task) => task.to_info(base),
            Self::Tracked { to_info, .. } => to_info(base),
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{Map, Value};

    use super::*;
    use crate::agent::task::{AgentTaskError, AgentTaskSink};

    struct StubTask {
        kind: &'static str,
    }

    #[async_trait]
    impl AgentTask for StubTask {
        fn id_prefix(&self) -> &str {
            "bash"
        }

        fn kind(&self) -> &str {
            self.kind
        }

        fn description(&self) -> &str {
            "registered command"
        }

        async fn start(&self, _sink: &dyn AgentTaskSink) -> Result<(), AgentTaskError> {
            Ok(())
        }

        fn to_info(&self, base: AgentTaskInfoBase) -> AgentTaskInfo {
            AgentTaskInfo {
                base,
                kind: self.kind.into(),
                details: Map::from_iter([("command".into(), Value::String("pwd".into()))]),
            }
        }
    }

    #[test]
    fn registered_projection_uses_task_description_kind_and_details() {
        let projection =
            ManagedTaskInfoProjection::registered(Arc::new(StubTask { kind: "process" }));
        assert!(projection.enforces_process_output_limit());
        let info = projection.to_info(
            "bash-12345678",
            AgentTaskStatus::Running,
            false,
            10,
            None,
            Some("reason".into()),
            Some(true),
            Some(20),
        );
        assert_eq!(info.base.task_id, "bash-12345678");
        assert_eq!(info.base.description, "registered command");
        assert_eq!(info.base.detached, Some(false));
        assert_eq!(info.base.stop_reason.as_deref(), Some("reason"));
        assert_eq!(info.base.terminal_notification_suppressed, Some(true));
        assert_eq!(info.base.timeout_ms, Some(20));
        assert_eq!(info.kind, "process");
        assert_eq!(info.details["command"], "pwd");
    }

    #[test]
    fn tracked_process_projection_does_not_enable_registered_process_limit() {
        let projection = ManagedTaskInfoProjection::tracked(
            "tracked command",
            Arc::new(|base| AgentTaskInfo {
                base,
                kind: "process".into(),
                details: Map::from_iter([("pid".into(), Value::from(42))]),
            }),
        );
        assert!(!projection.enforces_process_output_limit());
        let info = projection.to_info(
            "task-12345678",
            AgentTaskStatus::Completed,
            true,
            1,
            Some(2),
            None,
            None,
            None,
        );
        assert_eq!(info.base.description, "tracked command");
        assert_eq!(info.base.detached, Some(true));
        assert_eq!(info.kind, "process");
        assert_eq!(info.details["pid"], 42);
    }

    #[test]
    fn non_process_registered_task_does_not_enable_output_limit() {
        let projection =
            ManagedTaskInfoProjection::registered(Arc::new(StubTask { kind: "agent" }));
        assert!(!projection.enforces_process_output_limit());
    }
}
