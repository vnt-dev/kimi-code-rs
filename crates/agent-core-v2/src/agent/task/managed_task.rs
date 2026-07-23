//! Source-specific projection of live task state into `AgentTaskInfo`.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `toInfo()` and the source distinction used by `appendOutput()`.

use std::sync::Arc;

use futures_util::{FutureExt, future::ready};

use crate::_base::utils::abort::AbortController;

use super::{
    AgentTask, AgentTaskForceStop, AgentTaskInfo, AgentTaskInfoBase, AgentTaskOnDetach,
    AgentTaskSettlement, AgentTaskStatus, AgentTaskToInfo, AgentTaskTrackOptions,
    ForegroundRelease, ForegroundTaskReleaseFuture, ForegroundTaskReleaseReason,
    RegisterAgentTaskOptions, TaskOutputBuffer, apply_task_settlement,
};

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

    pub fn registered_task(&self) -> Option<Arc<dyn AgentTask>> {
        match self {
            Self::Registered(task) => Some(Arc::clone(task)),
            Self::Tracked { .. } => None,
        }
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

pub struct ManagedTaskState {
    pub task_id: String,
    pub projection: ManagedTaskInfoProjection,
    pub options: RegisterAgentTaskOptions,
    pub status: AgentTaskStatus,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub foreground_release: Option<ForegroundRelease>,
    pub stop_reason: Option<String>,
    pub terminal_notification_suppressed: Option<bool>,
    pub terminal_fired: bool,
    pub timed_out: bool,
    pub abort_controller: AbortController,
    pub output: TaskOutputBuffer,
    pub force_stop: Option<AgentTaskForceStop>,
    pub on_detach: Option<AgentTaskOnDetach>,
}

impl ManagedTaskState {
    // Original: AgentTaskService.registerTask() entry construction.
    pub fn registered(
        task_id: String,
        task: Arc<dyn AgentTask>,
        options: RegisterAgentTaskOptions,
        started_at: i64,
    ) -> Self {
        let detached = options.detached.unwrap_or(true);
        let timeout_ms = options.timeout_ms.or_else(|| task.timeout_ms());
        let options = RegisterAgentTaskOptions {
            detached: Some(detached),
            timeout_ms,
            detach_timeout_ms: options.detach_timeout_ms,
            auto_background_on_timeout: options.auto_background_on_timeout,
            signal: (!detached).then_some(options.signal).flatten(),
        };
        Self::new(
            task_id,
            ManagedTaskInfoProjection::registered(task),
            options,
            started_at,
            None,
            None,
        )
    }

    // Original: AgentTaskService.track() entry construction.
    pub fn tracked(task_id: String, options: AgentTaskTrackOptions, started_at: i64) -> Self {
        let AgentTaskTrackOptions {
            id_prefix: _,
            description,
            detached,
            timeout_ms,
            detach_timeout_ms,
            signal,
            force_stop,
            on_detach,
            to_info,
        } = options;
        let detached = detached.unwrap_or(true);
        Self::new(
            task_id,
            ManagedTaskInfoProjection::tracked(description, to_info),
            RegisterAgentTaskOptions {
                detached: Some(detached),
                timeout_ms,
                detach_timeout_ms,
                auto_background_on_timeout: None,
                signal: (!detached).then_some(signal).flatten(),
            },
            started_at,
            force_stop,
            on_detach,
        )
    }

    fn new(
        task_id: String,
        projection: ManagedTaskInfoProjection,
        options: RegisterAgentTaskOptions,
        started_at: i64,
        force_stop: Option<AgentTaskForceStop>,
        on_detach: Option<AgentTaskOnDetach>,
    ) -> Self {
        let detached = options.detached != Some(false);
        Self {
            task_id,
            projection,
            options,
            status: AgentTaskStatus::Running,
            started_at,
            ended_at: None,
            foreground_release: (!detached).then(ForegroundRelease::new),
            stop_reason: None,
            terminal_notification_suppressed: None,
            terminal_fired: false,
            timed_out: false,
            abort_controller: AbortController::new(),
            output: TaskOutputBuffer::new(detached),
            force_stop,
            on_detach,
        }
    }

    // Original: taskService.ts, startsDetached().
    pub fn starts_detached(&self) -> bool {
        self.options.detached != Some(false)
    }

    // Original: taskService.ts, isDetached().
    pub fn is_detached(&self) -> bool {
        self.foreground_release.is_none()
    }

    pub fn foreground_release_future(&self) -> ForegroundTaskReleaseFuture {
        self.foreground_release.as_ref().map_or_else(
            || {
                ready(ForegroundTaskReleaseReason::Terminal)
                    .boxed()
                    .shared()
            },
            ForegroundRelease::future,
        )
    }

    pub fn take_foreground_release(&mut self) -> Option<ForegroundRelease> {
        self.foreground_release.take()
    }

    // Original: taskService.ts, applyDetachTimeout() state portion.
    pub fn apply_detach_timeout(&mut self) -> Option<u64> {
        let timeout_ms = self.options.detach_timeout_ms?;
        self.options.timeout_ms = Some(timeout_ms);
        Some(timeout_ms)
    }

    // Original: taskService.ts, canAutoBackgroundOnTimeout().
    pub fn can_auto_background_on_timeout(&self) -> bool {
        self.options.auto_background_on_timeout == Some(true) && !self.is_detached()
    }

    // Original: taskService.ts, settleTask() state mutation.
    pub fn apply_settlement(&mut self, settlement: AgentTaskSettlement, now_ms: i64) -> bool {
        let mut base = AgentTaskInfoBase {
            task_id: self.task_id.clone(),
            description: String::new(),
            status: self.status,
            detached: None,
            started_at: self.started_at,
            ended_at: self.ended_at,
            stop_reason: self.stop_reason.clone(),
            terminal_notification_suppressed: self.terminal_notification_suppressed,
            timeout_ms: self.options.timeout_ms,
        };
        if !apply_task_settlement(&mut base, settlement, now_ms) {
            return false;
        }
        self.status = base.status;
        self.ended_at = base.ended_at;
        self.stop_reason = base.stop_reason;
        true
    }

    // Original: taskService.ts, toInfo().
    pub fn to_info(&self) -> AgentTaskInfo {
        self.projection.to_info(
            &self.task_id,
            self.status,
            self.is_detached(),
            self.started_at,
            self.ended_at,
            self.stop_reason.clone(),
            self.terminal_notification_suppressed,
            self.options.timeout_ms,
        )
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
        timeout_ms: Option<u64>,
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

        fn timeout_ms(&self) -> Option<u64> {
            self.timeout_ms
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
        let projection = ManagedTaskInfoProjection::registered(Arc::new(StubTask {
            kind: "process",
            timeout_ms: None,
        }));
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
        let projection = ManagedTaskInfoProjection::registered(Arc::new(StubTask {
            kind: "agent",
            timeout_ms: None,
        }));
        assert!(!projection.enforces_process_output_limit());
    }

    #[test]
    fn registered_state_applies_defaults_task_timeout_and_signal_rules() {
        let external = AbortController::new();
        let task: Arc<dyn AgentTask> = Arc::new(StubTask {
            kind: "process",
            timeout_ms: Some(50),
        });
        let detached = ManagedTaskState::registered(
            "bash-12345678".into(),
            Arc::clone(&task),
            RegisterAgentTaskOptions {
                signal: Some(external.signal()),
                ..RegisterAgentTaskOptions::default()
            },
            10,
        );
        assert!(detached.starts_detached());
        assert!(detached.is_detached());
        assert_eq!(detached.options.timeout_ms, Some(50));
        assert!(detached.options.signal.is_none());
        assert!(detached.output.output_persist_started);
        assert_eq!(detached.to_info().base.detached, Some(true));

        let foreground = ManagedTaskState::registered(
            "bash-abcdefgh".into(),
            task,
            RegisterAgentTaskOptions {
                detached: Some(false),
                timeout_ms: Some(20),
                signal: Some(external.signal()),
                ..RegisterAgentTaskOptions::default()
            },
            11,
        );
        assert!(!foreground.starts_detached());
        assert!(!foreground.is_detached());
        assert_eq!(foreground.options.timeout_ms, Some(20));
        assert!(foreground.options.signal.is_some());
        assert!(!foreground.output.output_persist_started);
    }

    #[tokio::test]
    async fn tracked_state_preserves_callbacks_detach_timeout_and_release_semantics() {
        let mut state = ManagedTaskState::tracked(
            "task-12345678".into(),
            AgentTaskTrackOptions {
                id_prefix: None,
                description: "tracked".into(),
                detached: Some(false),
                timeout_ms: Some(10),
                detach_timeout_ms: Some(30),
                signal: None,
                force_stop: Some(Arc::new(|| Box::pin(async { Ok(()) }))),
                on_detach: Some(Arc::new(|| {})),
                to_info: Arc::new(|base| AgentTaskInfo {
                    base,
                    kind: "process".into(),
                    details: Map::new(),
                }),
            },
            1,
        );
        assert!(!state.is_detached());
        assert!(!state.can_auto_background_on_timeout());
        assert_eq!(state.apply_detach_timeout(), Some(30));
        assert_eq!(state.options.timeout_ms, Some(30));
        let release_future = state.foreground_release_future();
        let release = state.take_foreground_release().unwrap();
        release.resolve(ForegroundTaskReleaseReason::TimeoutDetached);
        assert_eq!(
            release_future.await,
            ForegroundTaskReleaseReason::TimeoutDetached
        );
        assert!(state.is_detached());
        assert!(state.force_stop.is_some());
        assert!(state.on_detach.is_some());
        assert_eq!(state.to_info().base.detached, Some(true));

        let detached = ManagedTaskState::tracked(
            "task-abcdefgh".into(),
            AgentTaskTrackOptions {
                id_prefix: None,
                description: "tracked".into(),
                detached: Some(true),
                timeout_ms: None,
                detach_timeout_ms: None,
                signal: None,
                force_stop: None,
                on_detach: None,
                to_info: Arc::new(|base| AgentTaskInfo {
                    base,
                    kind: "process".into(),
                    details: Map::new(),
                }),
            },
            1,
        );
        assert_eq!(
            detached.foreground_release_future().await,
            ForegroundTaskReleaseReason::Terminal
        );
    }
}
