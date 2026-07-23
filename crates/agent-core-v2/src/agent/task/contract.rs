//! Agent-scoped task manager contract.
//!
//! Original: `packages/agent-core-v2/src/agent/task/task.ts`.

use std::{
    error::Error,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};

use crate::{
    _base::{di::instantiation::ServiceIdentifier, event::Event, utils::abort::AbortSignal},
    app::task::contract::{TaskHandle, TaskState},
};

use super::types::{AgentTask, AgentTaskInfo, AgentTaskInfoBase};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AgentTaskLoadOptions {
    pub replace: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentTaskOutputSnapshot {
    pub output_path: Option<String>,
    pub output_size_bytes: usize,
    pub preview_bytes: usize,
    pub truncated: bool,
    pub full_output_available: bool,
    pub preview: String,
}

#[derive(Clone, Default)]
pub struct RegisterAgentTaskOptions {
    pub detached: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub detach_timeout_ms: Option<u64>,
    pub auto_background_on_timeout: Option<bool>,
    pub signal: Option<AbortSignal>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForegroundTaskReleaseReason {
    Detached,
    TimeoutDetached,
    Terminal,
}

pub type ForegroundTaskReleaseFuture = Shared<BoxFuture<'static, ForegroundTaskReleaseReason>>;

// Original: taskService.ts, ForegroundRelease and createForegroundRelease().
pub struct ForegroundRelease {
    sender: tokio::sync::watch::Sender<Option<ForegroundTaskReleaseReason>>,
    resolved: AtomicBool,
    future: ForegroundTaskReleaseFuture,
}

impl ForegroundRelease {
    pub fn new() -> Self {
        let (sender, mut receiver) = tokio::sync::watch::channel(None);
        let keep_alive = sender.clone();
        let future = async move {
            let _keep_alive = keep_alive;
            loop {
                if let Some(reason) = *receiver.borrow() {
                    return reason;
                }
                let _ = receiver.changed().await;
            }
        }
        .boxed()
        .shared();
        Self {
            sender,
            resolved: AtomicBool::new(false),
            future,
        }
    }

    pub fn future(&self) -> ForegroundTaskReleaseFuture {
        self.future.clone()
    }

    pub fn resolve(&self, reason: ForegroundTaskReleaseReason) {
        if self
            .resolved
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.sender.send_replace(Some(reason));
        }
    }
}

impl Default for ForegroundRelease {
    fn default() -> Self {
        Self::new()
    }
}
pub type AgentTaskCallbackFuture = BoxFuture<'static, Result<(), AgentTaskServiceError>>;
pub type AgentTaskForceStop = Arc<dyn Fn() -> AgentTaskCallbackFuture + Send + Sync>;
pub type AgentTaskOnDetach = Arc<dyn Fn() + Send + Sync>;
pub type AgentTaskToInfo = Arc<dyn Fn(AgentTaskInfoBase) -> AgentTaskInfo + Send + Sync>;

#[derive(Clone)]
pub struct AgentTaskTrackOptions {
    pub id_prefix: Option<String>,
    pub description: String,
    pub detached: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub detach_timeout_ms: Option<u64>,
    pub signal: Option<AbortSignal>,
    pub force_stop: Option<AgentTaskForceStop>,
    pub on_detach: Option<AgentTaskOnDetach>,
    pub to_info: AgentTaskToInfo,
}

#[derive(Clone)]
pub struct AgentTaskEntry {
    pub task_id: String,
    pub on_did_detach: ForegroundTaskReleaseFuture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentTaskNotificationSeverity {
    Info,
    Warning,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentTaskNotificationContext {
    pub notification_type: String,
    pub title: String,
    pub body: String,
    pub severity: AgentTaskNotificationSeverity,
    pub source_kind: String,
    pub source_id: String,
}

pub type AgentTaskServiceError = Box<dyn Error + Send + Sync>;
pub type AgentTaskServiceResult<T> = Result<T, AgentTaskServiceError>;

/// Object-safe projection of the generic app task handle. Agent task tracking
/// observes settlement but intentionally discards the underlying result value.
#[async_trait]
pub trait AgentTrackedTaskHandle: Send + Sync {
    fn id(&self) -> &str;
    fn state(&self) -> TaskState;
    async fn settled(&self);
    fn on_did_change_state(&self) -> Event<TaskState>;
    fn on_did_output(&self) -> Event<String>;
    fn cancel(&self);
}

/// Rust adaptation of passing an `ITaskHandle<T>` to `AgentTaskService.track()`.
/// The agent task layer observes settlement but deliberately does not expose the
/// generic result value, matching the original `handle.result.then(() => {},
/// () => {})` lifecycle promise.
pub struct AgentTrackedTaskHandleAdapter<T> {
    handle: Arc<dyn TaskHandle<T>>,
}

impl<T> AgentTrackedTaskHandleAdapter<T>
where
    T: Send + Sync + 'static,
{
    pub fn new<H>(handle: Arc<H>) -> Self
    where
        H: TaskHandle<T> + 'static,
    {
        Self { handle }
    }

    pub fn from_handle(handle: Arc<dyn TaskHandle<T>>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl<T> AgentTrackedTaskHandle for AgentTrackedTaskHandleAdapter<T>
where
    T: Send + Sync + 'static,
{
    fn id(&self) -> &str {
        self.handle.id()
    }

    fn state(&self) -> TaskState {
        self.handle.state()
    }

    async fn settled(&self) {
        let _ = self.handle.result().await;
    }

    fn on_did_change_state(&self) -> Event<TaskState> {
        self.handle.on_did_change_state()
    }

    fn on_did_output(&self) -> Event<String> {
        self.handle.on_did_output()
    }

    fn cancel(&self) {
        self.handle.cancel();
    }
}

#[async_trait]
pub trait AgentTaskServiceContract: Send + Sync {
    fn track(
        &self,
        handle: Arc<dyn AgentTrackedTaskHandle>,
        options: AgentTaskTrackOptions,
    ) -> AgentTaskServiceResult<AgentTaskEntry>;

    fn register_task(
        &self,
        task: Arc<dyn AgentTask>,
        options: RegisterAgentTaskOptions,
    ) -> AgentTaskServiceResult<String>;

    fn get_task(&self, task_id: &str) -> Option<AgentTaskInfo>;
    fn list(&self, active_only: Option<bool>, limit: Option<usize>) -> Vec<AgentTaskInfo>;
    fn persist_output(&self, task_id: &str);

    async fn get_output_snapshot(
        &self,
        task_id: &str,
        max_preview_bytes: f64,
    ) -> AgentTaskServiceResult<AgentTaskOutputSnapshot>;

    async fn read_output(&self, task_id: &str, tail: Option<f64>)
    -> AgentTaskServiceResult<String>;

    async fn suppress_terminal_notification(&self, task_id: &str) -> AgentTaskServiceResult<()>;
    fn detach(&self, task_id: &str) -> Option<AgentTaskInfo>;

    async fn stop(
        &self,
        task_id: &str,
        reason: Option<&str>,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>>;

    async fn stop_by_user(&self, task_id: &str) -> AgentTaskServiceResult<Option<AgentTaskInfo>>;

    async fn stop_all(&self, reason: Option<&str>) -> AgentTaskServiceResult<Vec<AgentTaskInfo>>;

    async fn stop_all_on_exit(&self, reason: &str) -> AgentTaskServiceResult<Vec<AgentTaskInfo>>;

    async fn wait(
        &self,
        task_id: &str,
        timeout_ms: Option<f64>,
        signal: Option<AbortSignal>,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>>;

    async fn wait_for_foreground_release(
        &self,
        task_id: &str,
    ) -> AgentTaskServiceResult<Option<ForegroundTaskReleaseReason>>;
}

#[derive(Clone)]
pub struct AgentTaskServiceHandle(pub Arc<dyn AgentTaskServiceContract>);

impl Deref for AgentTaskServiceHandle {
    type Target = dyn AgentTaskServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_TASK_SERVICE_ID: ServiceIdentifier<AgentTaskServiceHandle> =
    ServiceIdentifier::new("agentTaskService");

#[cfg(test)]
mod tests {
    use futures_util::{FutureExt, future::ready};

    use super::*;

    #[test]
    fn defaults_and_service_identifier_match_source_contract() {
        assert_eq!(AGENT_TASK_SERVICE_ID.to_string(), "agentTaskService");
        assert_eq!(AgentTaskLoadOptions::default().replace, None);
        assert_eq!(
            AgentTaskOutputSnapshot::default(),
            AgentTaskOutputSnapshot {
                output_path: None,
                output_size_bytes: 0,
                preview_bytes: 0,
                truncated: false,
                full_output_available: false,
                preview: String::new(),
            }
        );
        let options = RegisterAgentTaskOptions::default();
        assert_eq!(options.detached, None);
        assert_eq!(options.timeout_ms, None);
    }

    #[tokio::test]
    async fn release_future_is_cloneable_like_a_javascript_promise() {
        let release: ForegroundTaskReleaseFuture =
            ready(ForegroundTaskReleaseReason::TimeoutDetached)
                .boxed()
                .shared();
        assert_eq!(
            release.clone().await,
            ForegroundTaskReleaseReason::TimeoutDetached
        );
        assert_eq!(release.await, ForegroundTaskReleaseReason::TimeoutDetached);
    }

    #[tokio::test]
    async fn foreground_release_is_first_wins_and_shared() {
        let release = ForegroundRelease::new();
        let first = release.future();
        let second = release.future();
        release.resolve(ForegroundTaskReleaseReason::Detached);
        release.resolve(ForegroundTaskReleaseReason::Terminal);
        assert_eq!(first.await, ForegroundTaskReleaseReason::Detached);
        assert_eq!(second.await, ForegroundTaskReleaseReason::Detached);
    }

    #[tokio::test(start_paused = true)]
    async fn dropped_release_keeps_the_promise_pending() {
        let future = {
            let release = ForegroundRelease::new();
            release.future()
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), future)
                .await
                .is_err()
        );
    }

    #[test]
    fn notification_context_keeps_info_and_warning_distinct() {
        let mut context = AgentTaskNotificationContext {
            notification_type: "task".into(),
            title: "Completed".into(),
            body: "done".into(),
            severity: AgentTaskNotificationSeverity::Info,
            source_kind: "process".into(),
            source_id: "bash-12345678".into(),
        };
        assert_eq!(context.severity, AgentTaskNotificationSeverity::Info);
        context.severity = AgentTaskNotificationSeverity::Warning;
        assert_eq!(context.severity, AgentTaskNotificationSeverity::Warning);
    }

    #[tokio::test]
    async fn tracked_handle_adapter_erases_results_and_preserves_cancellation() {
        use crate::app::task::task_service::TaskService;

        let handle = TaskService::new().defer::<String>();
        let tracked = AgentTrackedTaskHandleAdapter::new(Arc::clone(&handle));
        assert_eq!(tracked.id(), "task-0");
        assert_eq!(tracked.state(), TaskState::Pending);

        tracked.cancel();
        tracked.settled().await;
        assert_eq!(tracked.state(), TaskState::Cancelled);
        assert!(handle.result().await.is_err());
    }
}
