//! Runtime task registry and persistence-backed query operations.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `getTask()`, `list()`, `persistOutput()`, `loadFromDisk()`,
//! `markLoadedTasksLost()`, `getOutputSnapshot()`, and `readOutput()`.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use futures_util::{FutureExt, future::BoxFuture};
use indexmap::IndexMap;

use crate::{
    _base::{
        di::lifecycle::DisposableHandle,
        utils::abort::{AbortLink, AbortSignal, abortable},
    },
    agent::context_memory::{ContextMessage, PromptOrigin},
};

use super::{
    AgentTaskInfo, AgentTaskLifecycleRecorder, AgentTaskLoadOptions,
    AgentTaskNotificationBuildContext, AgentTaskNotificationEffects, AgentTaskOutputSnapshot,
    AgentTaskPersistence, AgentTaskServiceResult, AgentTaskSettlement, ForegroundTaskReleaseReason,
    ManagedTaskState, NOTIFICATION_FALLBACK_PREVIEW_BYTES, RestoredTaskRegistry,
    ScheduledTaskNotification, TaskModelState, TaskNotificationDelivery, empty_output_snapshot,
    finish_task_notification, needs_notification_fallback_preview, should_list_task,
};

const JAVASCRIPT_MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

type TaskWriteBarrier = futures_util::future::Shared<BoxFuture<'static, ()>>;

struct ManagedTaskRuntime {
    state: ManagedTaskState,
    persist_write_queue: TaskWriteBarrier,
    output_write_queue: TaskWriteBarrier,
    timeout_handle: Option<tokio::task::JoinHandle<()>>,
    foreground_signal_link: Option<AbortLink>,
    handle_subscription: Option<DisposableHandle>,
    settled: tokio::sync::watch::Sender<bool>,
}

impl ManagedTaskRuntime {
    fn new(state: ManagedTaskState) -> Self {
        let (settled, _) = tokio::sync::watch::channel(false);
        Self {
            state,
            persist_write_queue: futures_util::future::ready(()).boxed().shared(),
            output_write_queue: futures_util::future::ready(()).boxed().shared(),
            timeout_handle: None,
            foreground_signal_link: None,
            handle_subscription: None,
            settled,
        }
    }

    // Original: taskService.ts, persistLive(). State snapshots are captured
    // when queued and writes remain ordered even after an earlier write fails.
    fn persist_live(&mut self, persistence: Arc<AgentTaskPersistence>) -> TaskWriteBarrier {
        let previous = self.persist_write_queue.clone();
        let info = self.state.to_info();
        let next = async move {
            previous.await;
            let _ = persistence.write_task(&info).await;
        }
        .boxed()
        .shared();
        self.persist_write_queue = next.clone();
        tokio::spawn(next.clone());
        next
    }

    // Original: taskService.ts, appendTaskOutput(). Each future captures the
    // preceding barrier, preserving call order while swallowing storage errors.
    fn append_task_output(&mut self, persistence: Arc<AgentTaskPersistence>, chunk: String) {
        let previous = self.output_write_queue.clone();
        let task_id = self.state.task_id.clone();
        let next = async move {
            previous.await;
            let _ = persistence.append_task_output(&task_id, &chunk).await;
        }
        .boxed()
        .shared();
        self.output_write_queue = next.clone();
        tokio::spawn(next);
    }
}

#[derive(Default)]
struct AgentTaskServiceState {
    tasks: IndexMap<String, ManagedTaskRuntime>,
    ghosts: RestoredTaskRegistry,
    notifications: TaskNotificationDelivery,
}

#[derive(Clone)]
pub struct AgentTaskService {
    inner: Arc<AgentTaskServiceInner>,
}

pub trait AgentTaskRuntimeEffects: Send + Sync {
    fn context_snapshot(&self) -> Vec<ContextMessage>;
    fn record_task_started(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()>;
    fn record_task_terminated(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()>;
    fn enqueue_notification(
        &self,
        built: &AgentTaskNotificationBuildContext,
    ) -> AgentTaskServiceResult<()>;
    fn restore_notification(
        &self,
        built: &AgentTaskNotificationBuildContext,
    ) -> AgentTaskServiceResult<()>;
}

pub struct DefaultAgentTaskRuntimeEffects {
    context: Arc<dyn crate::agent::context_memory::AgentContextMemoryServiceContract>,
    lifecycle: AgentTaskLifecycleRecorder,
    notifications: AgentTaskNotificationEffects,
}

impl DefaultAgentTaskRuntimeEffects {
    pub fn new(
        context: Arc<dyn crate::agent::context_memory::AgentContextMemoryServiceContract>,
        lifecycle: AgentTaskLifecycleRecorder,
        notifications: AgentTaskNotificationEffects,
    ) -> Self {
        Self {
            context,
            lifecycle,
            notifications,
        }
    }
}

impl AgentTaskRuntimeEffects for DefaultAgentTaskRuntimeEffects {
    fn context_snapshot(&self) -> Vec<ContextMessage> {
        self.context.get()
    }

    fn record_task_started(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()> {
        self.lifecycle.record_task_started(info)?;
        Ok(())
    }

    fn record_task_terminated(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()> {
        self.lifecycle.record_task_terminated(info)?;
        Ok(())
    }

    fn enqueue_notification(
        &self,
        built: &AgentTaskNotificationBuildContext,
    ) -> AgentTaskServiceResult<()> {
        self.notifications.enqueue(built)?;
        Ok(())
    }

    fn restore_notification(
        &self,
        built: &AgentTaskNotificationBuildContext,
    ) -> AgentTaskServiceResult<()> {
        self.notifications.restore(built)?;
        Ok(())
    }
}

struct AgentTaskServiceInner {
    persistence: Arc<AgentTaskPersistence>,
    effects: Arc<dyn AgentTaskRuntimeEffects>,
    state: Mutex<AgentTaskServiceState>,
}

impl AgentTaskService {
    pub fn new(
        persistence: Arc<AgentTaskPersistence>,
        effects: Arc<dyn AgentTaskRuntimeEffects>,
    ) -> Self {
        Self {
            inner: Arc::new(AgentTaskServiceInner {
                persistence,
                effects,
                state: Mutex::new(AgentTaskServiceState::default()),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, AgentTaskServiceState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    // Wired into the public registerTask/track paths by the next service unit.
    #[allow(dead_code)]
    pub(crate) fn insert_managed_task(&self, task: ManagedTaskState) {
        let task_id = task.task_id.clone();
        let mut state = self.state();
        state
            .tasks
            .insert(task_id.clone(), ManagedTaskRuntime::new(task));
        state.ghosts.remove(&task_id);
    }

    // Original: AgentTaskService.restoreGhostsFromWire().
    pub fn restore_ghosts_from_wire(&self, wire_tasks: &TaskModelState) {
        let mut state = self.state();
        let live_task_ids = state.tasks.keys().cloned().collect::<HashSet<_>>();
        state.ghosts.restore_from_wire(wire_tasks, &live_task_ids);
    }

    pub fn restore_delivered_notifications(&self, keys: impl IntoIterator<Item = String>) {
        self.state().notifications.restore_delivered(keys);
    }

    pub fn mark_delivered_notification(&self, origin: &PromptOrigin) {
        self.state().notifications.mark_delivered(origin);
    }

    // Original: AgentTaskService.getTask().
    pub fn get_task(&self, task_id: &str) -> Option<AgentTaskInfo> {
        let state = self.state();
        state
            .tasks
            .get(task_id)
            .map(|entry| entry.state.to_info())
            .or_else(|| state.ghosts.get(task_id).cloned())
    }

    // Original: AgentTaskService.list(). Live entries always precede ghosts,
    // and ghosts are excluded from the default active-only view.
    pub fn list(&self, active_only: Option<bool>, limit: Option<usize>) -> Vec<AgentTaskInfo> {
        let active_only = active_only.unwrap_or(true);
        let state = self.state();
        let mut result = Vec::new();
        for entry in state.tasks.values() {
            let info = entry.state.to_info();
            if !should_list_task(&info, active_only) {
                continue;
            }
            result.push(info);
            if limit.is_some_and(|limit| result.len() >= limit) {
                return result;
            }
        }
        if !active_only {
            for ghost in state.ghosts.values() {
                if !should_list_task(ghost, active_only) {
                    continue;
                }
                result.push(ghost.clone());
                if limit.is_some_and(|limit| result.len() >= limit) {
                    return result;
                }
            }
        }
        result
    }

    // Original: AgentTaskService.persistOutput() and startOutputPersist().
    pub fn persist_output(&self, task_id: &str) {
        let persistence = Arc::clone(&self.inner.persistence);
        let mut state = self.state();
        let Some(entry) = state.tasks.get_mut(task_id) else {
            return;
        };
        if let Some(pending) = entry.state.output.start_output_persist() {
            entry.append_task_output(persistence, pending);
        }
    }

    // Original: AgentTaskService.loadFromDisk(). Storage is read without the
    // state lock; the merge then atomically observes the current live ids.
    pub async fn load_from_disk(
        &self,
        options: AgentTaskLoadOptions,
    ) -> AgentTaskServiceResult<()> {
        if options.replace != Some(false) {
            self.state().ghosts.clear();
        }
        let tasks = self.inner.persistence.list_tasks().await?;
        let mut state = self.state();
        let live_task_ids = state.tasks.keys().cloned().collect::<HashSet<_>>();
        state
            .ghosts
            .merge_loaded(tasks, Some(false), &live_task_ids);
        Ok(())
    }

    // Original: AgentTaskService.markLoadedTasksLost(). Records are persisted
    // sequentially in JavaScript Map order before callers emit terminal effects.
    pub async fn mark_loaded_tasks_lost(
        &self,
        now_ms: i64,
    ) -> AgentTaskServiceResult<Vec<AgentTaskInfo>> {
        let task_ids = self.state().ghosts.task_ids();
        let mut lost = Vec::new();
        for task_id in task_ids {
            let updated = self.state().ghosts.mark_active_lost_task(&task_id, now_ms);
            let Some(updated) = updated else {
                continue;
            };
            self.inner.persistence.write_task(&updated).await?;
            lost.push(updated);
        }
        Ok(lost)
    }

    // Original: AgentTaskService.getOutputSnapshot().
    pub async fn get_output_snapshot(
        &self,
        task_id: &str,
        max_preview_bytes: f64,
    ) -> AgentTaskServiceResult<AgentTaskOutputSnapshot> {
        let barrier = {
            let state = self.state();
            if !state.tasks.contains_key(task_id) && state.ghosts.get(task_id).is_none() {
                return Ok(empty_output_snapshot());
            }
            state
                .tasks
                .get(task_id)
                .map(|entry| entry.output_write_queue.clone())
        };
        if let Some(barrier) = barrier {
            barrier.await;
        }

        if let Some(persisted) = self
            .inner
            .persistence
            .read_task_output_snapshot(task_id, max_preview_bytes)
            .await?
        {
            return Ok(AgentTaskOutputSnapshot {
                output_path: Some(persisted.output_path.to_string_lossy().into_owned()),
                output_size_bytes: persisted.output_size_bytes,
                preview_bytes: persisted.preview_bytes,
                truncated: persisted.truncated,
                full_output_available: true,
                preview: persisted.preview,
            });
        }

        Ok(self
            .state()
            .tasks
            .get(task_id)
            .map_or_else(empty_output_snapshot, |entry| {
                entry.state.output.snapshot(max_preview_bytes)
            }))
    }

    // Original: AgentTaskService.readOutput(). `String.slice()` counts UTF-16
    // code units, so the Rust adaptation does the same for non-ASCII output.
    pub async fn read_output(
        &self,
        task_id: &str,
        tail: Option<f64>,
    ) -> AgentTaskServiceResult<String> {
        let output = self
            .get_output_snapshot(task_id, JAVASCRIPT_MAX_SAFE_INTEGER)
            .await?
            .preview;
        Ok(tail.map_or(output.clone(), |tail| utf16_tail(&output, tail)))
    }

    // Original: AgentTaskService.suppressTerminalNotification(). Unknown and
    // restored-only tasks are no-ops; live writes await the ordered queue.
    pub async fn suppress_terminal_notification(
        &self,
        task_id: &str,
    ) -> AgentTaskServiceResult<()> {
        let write = {
            let persistence = Arc::clone(&self.inner.persistence);
            let mut state = self.state();
            let Some(entry) = state.tasks.get_mut(task_id) else {
                return Ok(());
            };
            if entry.state.terminal_notification_suppressed == Some(true) {
                return Ok(());
            }
            entry.state.terminal_notification_suppressed = Some(true);
            entry.persist_live(persistence)
        };
        write.await;
        Ok(())
    }

    // Original: AgentTaskService.settleTask().
    pub async fn settle_task(
        &self,
        task_id: &str,
        settlement: AgentTaskSettlement,
        now_ms: i64,
    ) -> AgentTaskServiceResult<bool> {
        let (timeout, signal_link, subscription, output_persist_started) = {
            let mut state = self.state();
            let Some(entry) = state.tasks.get_mut(task_id) else {
                return Ok(false);
            };
            if !entry.state.apply_settlement(settlement, now_ms) {
                return Ok(false);
            }
            (
                entry.timeout_handle.take(),
                entry.foreground_signal_link.take(),
                entry.handle_subscription.take(),
                entry.state.output.output_persist_started,
            )
        };
        if let Some(timeout) = timeout {
            timeout.abort();
        }
        drop(signal_link);
        if let Some(subscription) = subscription {
            subscription.dispose()?;
        }

        let persist = {
            let persistence = Arc::clone(&self.inner.persistence);
            let mut state = self.state();
            let entry = state.tasks.get_mut(task_id).ok_or_else(|| {
                std::io::Error::other(format!("settling task disappeared: {task_id}"))
            })?;
            if output_persist_started {
                Some(entry.persist_live(persistence))
            } else {
                entry.state.output.discard_pending_output();
                None
            }
        };
        if let Some(persist) = persist {
            persist.await;
        }

        let terminal_info = {
            let mut state = self.state();
            let entry = state.tasks.get_mut(task_id).ok_or_else(|| {
                std::io::Error::other(format!("settling task disappeared: {task_id}"))
            })?;
            if !entry.state.terminal_fired && entry.state.is_detached() {
                entry.state.terminal_fired = true;
                Some(entry.state.to_info())
            } else {
                None
            }
        };
        if let Some(info) = terminal_info {
            let context = self.inner.effects.context_snapshot();
            let scheduled = self.schedule_task_notification(&info, &context);
            if let Some(scheduled) = scheduled {
                let service = self.clone();
                let notification_info = info.clone();
                tokio::spawn(async move {
                    let _ = service
                        .finish_and_enqueue_task_notification(scheduled, notification_info)
                        .await;
                });
            }
            self.inner.effects.record_task_terminated(&info)?;
        }

        let mut state = self.state();
        let entry = state.tasks.get_mut(task_id).ok_or_else(|| {
            std::io::Error::other(format!("settling task disappeared: {task_id}"))
        })?;
        if let Some(release) = &entry.state.foreground_release {
            release.resolve(ForegroundTaskReleaseReason::Terminal);
        }
        entry.settled.send_replace(true);
        Ok(true)
    }

    // Original: AgentTaskService.waitForForegroundRelease().
    pub async fn wait_for_foreground_release(
        &self,
        task_id: &str,
    ) -> AgentTaskServiceResult<Option<ForegroundTaskReleaseReason>> {
        enum ForegroundWait {
            Missing,
            Terminal(TaskWriteBarrier),
            Detached,
            Pending(
                super::ForegroundTaskReleaseFuture,
                tokio::sync::watch::Receiver<bool>,
            ),
        }
        let wait = {
            let state = self.state();
            match state.tasks.get(task_id) {
                None => ForegroundWait::Missing,
                Some(entry) if entry.state.status.is_terminal() => {
                    ForegroundWait::Terminal(entry.persist_write_queue.clone())
                }
                Some(entry) if entry.state.is_detached() => ForegroundWait::Detached,
                Some(entry) => ForegroundWait::Pending(
                    entry.state.foreground_release_future(),
                    entry.settled.subscribe(),
                ),
            }
        };
        let (release, mut settled) = match wait {
            ForegroundWait::Missing => return Ok(None),
            ForegroundWait::Terminal(barrier) => {
                barrier.await;
                return Ok(Some(ForegroundTaskReleaseReason::Terminal));
            }
            ForegroundWait::Detached => {
                return Ok(Some(ForegroundTaskReleaseReason::Detached));
            }
            ForegroundWait::Pending(release, settled) => (release, settled),
        };
        let reason = tokio::select! {
            biased;
            reason = release => reason,
            () = wait_until_settled(&mut settled) => ForegroundTaskReleaseReason::Terminal,
        };
        if reason == ForegroundTaskReleaseReason::Terminal {
            let barrier = self
                .state()
                .tasks
                .get(task_id)
                .map(|entry| entry.persist_write_queue.clone());
            if let Some(barrier) = barrier {
                barrier.await;
            }
        }
        Ok(Some(reason))
    }

    // Original: AgentTaskService.wait().
    pub async fn wait(
        &self,
        task_id: &str,
        timeout_ms: Option<f64>,
        signal: Option<AbortSignal>,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        let timeout_ms = timeout_ms.unwrap_or(30_000.0);
        let (mut settled, terminal_barrier) = {
            let state = self.state();
            let Some(entry) = state.tasks.get(task_id) else {
                return Ok(state.ghosts.get(task_id).cloned());
            };
            if entry.state.status.is_terminal() {
                (None, Some(entry.persist_write_queue.clone()))
            } else if timeout_ms <= 0.0 {
                return Ok(Some(entry.state.to_info()));
            } else {
                (Some(entry.settled.subscribe()), None)
            }
        };
        if let Some(barrier) = terminal_barrier {
            barrier.await;
            return Ok(self.get_task(task_id));
        }

        let mut settled = settled
            .take()
            .ok_or_else(|| std::io::Error::other("task waiter missing settlement receiver"))?;
        let pending = async {
            tokio::select! {
                biased;
                () = wait_until_settled(&mut settled) => {},
                () = tokio::time::sleep(javascript_timeout_duration(timeout_ms)) => {},
            }
        };
        if let Some(signal) = signal {
            abortable(pending, &signal).await.map_err(|error| {
                Box::new((*error).clone()) as crate::agent::task::AgentTaskServiceError
            })?;
        } else {
            pending.await;
        }

        let barrier = {
            let state = self.state();
            state.tasks.get(task_id).and_then(|entry| {
                entry
                    .state
                    .status
                    .is_terminal()
                    .then(|| entry.persist_write_queue.clone())
            })
        };
        if let Some(barrier) = barrier {
            barrier.await;
        }
        Ok(self.get_task(task_id))
    }

    // Original: AgentTaskService.buildAgentTaskNotificationContext(). The
    // context snapshot is supplied explicitly so no state lock crosses either
    // persistence read.
    pub async fn build_agent_task_notification_context(
        &self,
        info: &AgentTaskInfo,
        context: &[ContextMessage],
    ) -> AgentTaskServiceResult<Option<AgentTaskNotificationBuildContext>> {
        let scheduled = self.schedule_task_notification(info, context);
        let Some(scheduled) = scheduled else {
            return Ok(None);
        };

        self.finish_task_notification_context(scheduled, info).await
    }

    fn schedule_task_notification(
        &self,
        info: &AgentTaskInfo,
        context: &[ContextMessage],
    ) -> Option<ScheduledTaskNotification> {
        self.state().notifications.try_schedule(info, context)
    }

    async fn finish_task_notification_context(
        &self,
        scheduled: ScheduledTaskNotification,
        info: &AgentTaskInfo,
    ) -> AgentTaskServiceResult<Option<AgentTaskNotificationBuildContext>> {
        let mut output = self.get_output_snapshot(&info.base.task_id, 0.0).await?;
        if needs_notification_fallback_preview(&output) {
            output = self
                .get_output_snapshot(&info.base.task_id, NOTIFICATION_FALLBACK_PREVIEW_BYTES)
                .await?;
        }
        let currently_suppressed = self.is_terminal_notification_suppressed(&info.base.task_id);
        Ok(finish_task_notification(
            scheduled,
            info,
            &output,
            currently_suppressed,
        ))
    }

    async fn finish_and_enqueue_task_notification(
        &self,
        scheduled: ScheduledTaskNotification,
        info: AgentTaskInfo,
    ) -> AgentTaskServiceResult<()> {
        if let Some(built) = self
            .finish_task_notification_context(scheduled, &info)
            .await?
        {
            self.inner.effects.enqueue_notification(&built)?;
        }
        Ok(())
    }

    // Original: AgentTaskService.notifyAgentTask().
    pub async fn notify_agent_task(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()> {
        let context = self.inner.effects.context_snapshot();
        let Some(built) = self
            .build_agent_task_notification_context(info, &context)
            .await?
        else {
            return Ok(());
        };
        self.inner.effects.enqueue_notification(&built)
    }

    // Original: AgentTaskService.restoreAgentTaskNotifications(). Terminal
    // notifications are restored sequentially in list(false) order.
    pub async fn restore_agent_task_notifications(&self) -> AgentTaskServiceResult<()> {
        for info in self.list(Some(false), None) {
            if !info.base.status.is_terminal() {
                continue;
            }
            let context = self.inner.effects.context_snapshot();
            let Some(built) = self
                .build_agent_task_notification_context(&info, &context)
                .await?
            else {
                continue;
            };
            self.inner.effects.restore_notification(&built)?;
        }
        Ok(())
    }

    // Original: AgentTaskService.reconcile(). Lost task lifecycle events are
    // emitted before restored terminal notifications.
    pub async fn reconcile(&self, now_ms: i64) -> AgentTaskServiceResult<Vec<AgentTaskInfo>> {
        let lost = self.mark_loaded_tasks_lost(now_ms).await?;
        for info in &lost {
            self.inner.effects.record_task_terminated(info)?;
        }
        self.restore_agent_task_notifications().await?;
        Ok(lost)
    }

    fn is_terminal_notification_suppressed(&self, task_id: &str) -> bool {
        let state = self.state();
        state
            .tasks
            .get(task_id)
            .is_some_and(|entry| entry.state.terminal_notification_suppressed == Some(true))
            || state
                .ghosts
                .get(task_id)
                .is_some_and(|info| info.base.terminal_notification_suppressed == Some(true))
    }
}

fn utf16_tail(value: &str, tail: f64) -> String {
    let units = value.encode_utf16().collect::<Vec<_>>();
    let truncated = tail.trunc();
    let requested = if truncated.is_nan() || truncated <= 0.0 || truncated >= usize::MAX as f64 {
        units.len()
    } else {
        (truncated as usize).min(units.len())
    };
    String::from_utf16_lossy(&units[units.len() - requested..])
}

async fn wait_until_settled(receiver: &mut tokio::sync::watch::Receiver<bool>) {
    while !*receiver.borrow() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

fn javascript_timeout_duration(timeout_ms: f64) -> Duration {
    const MAX_NODE_TIMEOUT_MS: f64 = 2_147_483_647.0;
    let milliseconds = if !timeout_ms.is_finite() || timeout_ms > MAX_NODE_TIMEOUT_MS {
        1
    } else {
        (timeout_ms.trunc() as u64).max(1)
    };
    Duration::from_millis(milliseconds)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{Map, Value};

    use super::*;
    use crate::{
        agent::task::{
            AgentTask, AgentTaskError, AgentTaskInfoBase, AgentTaskSettlementStatus, AgentTaskSink,
            AgentTaskStatus, RegisterAgentTaskOptions, TaskOutputAction,
        },
        persistence::{
            backends::{
                memory::in_memory_storage_service::InMemoryStorageService,
                node_fs::atomic_document_store::JsonAtomicDocumentStore,
            },
            interface::{
                atomic_document_store::{AtomicDocumentStoreHandle, AtomicDocumentStoreService},
                storage::{FileSystemStorageService, FileSystemStorageServiceHandle},
            },
        },
    };

    struct StubTask;

    #[derive(Default)]
    struct RecordingEffects {
        context: Mutex<Vec<ContextMessage>>,
        started: Mutex<Vec<String>>,
        terminated: Mutex<Vec<String>>,
        enqueued: Mutex<Vec<String>>,
        restored: Mutex<Vec<String>>,
    }

    impl AgentTaskRuntimeEffects for RecordingEffects {
        fn context_snapshot(&self) -> Vec<ContextMessage> {
            self.context.lock().unwrap().clone()
        }

        fn record_task_started(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()> {
            self.started.lock().unwrap().push(info.base.task_id.clone());
            Ok(())
        }

        fn record_task_terminated(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()> {
            self.terminated
                .lock()
                .unwrap()
                .push(info.base.task_id.clone());
            Ok(())
        }

        fn enqueue_notification(
            &self,
            built: &AgentTaskNotificationBuildContext,
        ) -> AgentTaskServiceResult<()> {
            self.enqueued
                .lock()
                .unwrap()
                .push(built.hook_context.source_id.clone());
            Ok(())
        }

        fn restore_notification(
            &self,
            built: &AgentTaskNotificationBuildContext,
        ) -> AgentTaskServiceResult<()> {
            self.restored
                .lock()
                .unwrap()
                .push(built.hook_context.source_id.clone());
            Ok(())
        }
    }

    #[async_trait]
    impl AgentTask for StubTask {
        fn id_prefix(&self) -> &str {
            "bash"
        }

        fn kind(&self) -> &str {
            "process"
        }

        fn description(&self) -> &str {
            "live"
        }

        async fn start(&self, _sink: &dyn AgentTaskSink) -> Result<(), AgentTaskError> {
            Ok(())
        }

        fn to_info(&self, base: AgentTaskInfoBase) -> AgentTaskInfo {
            AgentTaskInfo {
                base,
                kind: "process".into(),
                details: Map::new(),
            }
        }
    }

    fn task(task_id: &str, status: AgentTaskStatus, detached: bool) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: task_id.into(),
                description: task_id.into(),
                status,
                detached: Some(detached),
                started_at: 1,
                ended_at: status.is_terminal().then_some(2),
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::from_iter([("command".into(), Value::String("pwd".into()))]),
        }
    }

    fn service_with_effects() -> (
        Arc<AgentTaskPersistence>,
        AgentTaskService,
        Arc<RecordingEffects>,
    ) {
        let storage = Arc::new(InMemoryStorageService::default());
        let bytes: Arc<dyn FileSystemStorageService> = storage;
        let docs: Arc<dyn AtomicDocumentStoreService> =
            Arc::new(JsonAtomicDocumentStore::new(Arc::clone(&bytes)));
        let persistence = Arc::new(AgentTaskPersistence::new(
            "/session/agents/main",
            "session/agents/main",
            AtomicDocumentStoreHandle(docs),
            FileSystemStorageServiceHandle(bytes),
            None,
        ));
        let effects = Arc::new(RecordingEffects::default());
        let service = AgentTaskService::new(
            Arc::clone(&persistence),
            effects.clone() as Arc<dyn AgentTaskRuntimeEffects>,
        );
        (persistence, service, effects)
    }

    fn service() -> (Arc<AgentTaskPersistence>, AgentTaskService) {
        let (persistence, service, _) = service_with_effects();
        (persistence, service)
    }

    fn insert_live(service: &AgentTaskService, task_id: &str, detached: bool) {
        service.insert_managed_task(ManagedTaskState::registered(
            task_id.into(),
            Arc::new(StubTask),
            RegisterAgentTaskOptions {
                detached: Some(detached),
                ..RegisterAgentTaskOptions::default()
            },
            10,
        ));
    }

    #[tokio::test]
    async fn queries_keep_live_order_filter_terminal_foreground_and_append_ghosts() {
        let (persistence, service) = service();
        for info in [
            task("bash-ghost001", AgentTaskStatus::Completed, true),
            task("bash-live0001", AgentTaskStatus::Failed, true),
        ] {
            persistence.write_task(&info).await.unwrap();
        }
        insert_live(&service, "bash-live0001", true);
        insert_live(&service, "bash-front001", false);
        {
            let mut state = service.state();
            state.tasks.get_mut("bash-front001").unwrap().state.status = AgentTaskStatus::Completed;
        }
        service
            .load_from_disk(AgentTaskLoadOptions::default())
            .await
            .unwrap();

        assert_eq!(
            service.get_task("bash-live0001").unwrap().base.status,
            AgentTaskStatus::Running
        );
        assert_eq!(
            service
                .list(None, None)
                .iter()
                .map(|info| info.base.task_id.as_str())
                .collect::<Vec<_>>(),
            ["bash-live0001"]
        );
        assert_eq!(
            service
                .list(Some(false), None)
                .iter()
                .map(|info| info.base.task_id.as_str())
                .collect::<Vec<_>>(),
            ["bash-live0001", "bash-ghost001"]
        );
        assert_eq!(service.list(Some(false), Some(1)).len(), 1);
    }

    #[test]
    fn wire_restore_skips_live_entries_and_projects_other_records() {
        let (_persistence, service) = service();
        insert_live(&service, "bash-live0003", true);
        service.restore_ghosts_from_wire(&TaskModelState::from([
            (
                "bash-live0003".into(),
                task("bash-live0003", AgentTaskStatus::Failed, true),
            ),
            (
                "bash-wire0001".into(),
                task("bash-wire0001", AgentTaskStatus::Completed, true),
            ),
        ]));

        assert_eq!(
            service.get_task("bash-live0003").unwrap().base.status,
            AgentTaskStatus::Running
        );
        assert_eq!(
            service.get_task("bash-wire0001").unwrap().base.status,
            AgentTaskStatus::Completed
        );
    }

    #[tokio::test]
    async fn output_snapshot_waits_for_flush_then_prefers_persisted_output() {
        let (_persistence, service) = service();
        insert_live(&service, "bash-output01", false);
        {
            let mut state = service.state();
            let output = &mut state.tasks.get_mut("bash-output01").unwrap().state.output;
            assert_eq!(
                output.append("hello € world".into(), false),
                TaskOutputAction::None
            );
        }
        let buffered = service
            .get_output_snapshot("bash-output01", 9.0)
            .await
            .unwrap();
        assert!(!buffered.full_output_available);
        assert_eq!(buffered.preview, "€ world");

        service.persist_output("bash-output01");
        let persisted = service
            .get_output_snapshot("bash-output01", 5.0)
            .await
            .unwrap();
        assert!(persisted.full_output_available);
        assert_eq!(persisted.preview, "world");
        assert!(persisted.output_path.unwrap().ends_with("output.log"));
        assert_eq!(
            service
                .read_output("bash-output01", Some(5.9))
                .await
                .unwrap(),
            "world"
        );
        assert_eq!(service.read_output("missing-task", None).await.unwrap(), "");
    }

    #[tokio::test]
    async fn loaded_running_tasks_become_lost_and_are_written_back() {
        let (persistence, service) = service();
        persistence
            .write_task(&task("bash-running1", AgentTaskStatus::Running, true))
            .await
            .unwrap();
        persistence
            .write_task(&task("bash-done0001", AgentTaskStatus::Completed, true))
            .await
            .unwrap();
        service
            .load_from_disk(AgentTaskLoadOptions::default())
            .await
            .unwrap();
        let lost = service.mark_loaded_tasks_lost(99).await.unwrap();
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].base.task_id, "bash-running1");
        assert_eq!(lost[0].base.status, AgentTaskStatus::Lost);
        assert_eq!(lost[0].base.ended_at, Some(99));
        assert_eq!(
            persistence
                .read_task("bash-running1")
                .await
                .unwrap()
                .unwrap()
                .base
                .status,
            AgentTaskStatus::Lost
        );
    }

    #[tokio::test]
    async fn suppress_terminal_notification_updates_and_persists_only_live_tasks() {
        let (persistence, service) = service();
        let ghost = task("bash-ghost002", AgentTaskStatus::Completed, true);
        persistence.write_task(&ghost).await.unwrap();
        service
            .load_from_disk(AgentTaskLoadOptions::default())
            .await
            .unwrap();
        insert_live(&service, "bash-live0002", true);

        service
            .suppress_terminal_notification("bash-live0002")
            .await
            .unwrap();
        service
            .suppress_terminal_notification("bash-live0002")
            .await
            .unwrap();
        service
            .suppress_terminal_notification("bash-ghost002")
            .await
            .unwrap();
        service
            .suppress_terminal_notification("bash-missing2")
            .await
            .unwrap();

        assert_eq!(
            service
                .get_task("bash-live0002")
                .unwrap()
                .base
                .terminal_notification_suppressed,
            Some(true)
        );
        assert_eq!(
            persistence
                .read_task("bash-live0002")
                .await
                .unwrap()
                .unwrap()
                .base
                .terminal_notification_suppressed,
            Some(true)
        );
        assert_eq!(
            service
                .get_task("bash-ghost002")
                .unwrap()
                .base
                .terminal_notification_suppressed,
            None
        );
    }

    #[tokio::test]
    async fn settlement_persists_detached_tasks_releases_foreground_and_is_idempotent() {
        let (persistence, service, effects) = service_with_effects();
        insert_live(&service, "bash-settle01", true);
        assert!(
            service
                .settle_task(
                    "bash-settle01",
                    AgentTaskSettlement {
                        status: AgentTaskSettlementStatus::Completed,
                        stop_reason: None,
                    },
                    20,
                )
                .await
                .unwrap()
        );
        assert_eq!(
            persistence
                .read_task("bash-settle01")
                .await
                .unwrap()
                .unwrap()
                .base
                .status,
            AgentTaskStatus::Completed
        );
        assert_eq!(
            *effects.terminated.lock().unwrap(),
            ["bash-settle01".to_owned()]
        );
        assert!(
            !service
                .settle_task(
                    "bash-settle01",
                    AgentTaskSettlement {
                        status: AgentTaskSettlementStatus::Failed,
                        stop_reason: Some("late".into()),
                    },
                    30,
                )
                .await
                .unwrap()
        );

        insert_live(&service, "bash-settle02", false);
        let release = service
            .state()
            .tasks
            .get("bash-settle02")
            .unwrap()
            .state
            .foreground_release_future();
        service
            .state()
            .tasks
            .get_mut("bash-settle02")
            .unwrap()
            .state
            .output
            .append("foreground only".into(), false);
        assert!(
            service
                .settle_task(
                    "bash-settle02",
                    AgentTaskSettlement {
                        status: AgentTaskSettlementStatus::Killed,
                        stop_reason: Some("stopped".into()),
                    },
                    21,
                )
                .await
                .unwrap()
        );
        assert_eq!(release.await, ForegroundTaskReleaseReason::Terminal);
        assert!(
            persistence
                .read_task("bash-settle02")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            service
                .get_task("bash-settle02")
                .unwrap()
                .base
                .stop_reason
                .as_deref(),
            Some("stopped")
        );
        assert_eq!(effects.terminated.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn foreground_release_wait_distinguishes_missing_detached_and_pending_tasks() {
        let (_persistence, service) = service();
        insert_live(&service, "bash-detached", true);
        insert_live(&service, "bash-fore0001", false);

        assert_eq!(
            service
                .wait_for_foreground_release("bash-missing3")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            service
                .wait_for_foreground_release("bash-detached")
                .await
                .unwrap(),
            Some(ForegroundTaskReleaseReason::Detached)
        );

        let waiting = service
            .state()
            .tasks
            .get("bash-fore0001")
            .unwrap()
            .state
            .foreground_release_future();
        let release = service
            .state()
            .tasks
            .get_mut("bash-fore0001")
            .unwrap()
            .state
            .take_foreground_release()
            .unwrap();
        release.resolve(ForegroundTaskReleaseReason::Detached);
        assert_eq!(waiting.await, ForegroundTaskReleaseReason::Detached);
        assert_eq!(
            service
                .wait_for_foreground_release("bash-fore0001")
                .await
                .unwrap(),
            Some(ForegroundTaskReleaseReason::Detached)
        );

        service
            .state()
            .tasks
            .get_mut("bash-detached")
            .unwrap()
            .state
            .status = AgentTaskStatus::Completed;
        assert_eq!(
            service
                .wait_for_foreground_release("bash-detached")
                .await
                .unwrap(),
            Some(ForegroundTaskReleaseReason::Terminal)
        );
    }

    #[tokio::test]
    async fn wait_handles_immediate_timeout_abort_and_settlement_broadcast() {
        use crate::_base::utils::abort::AbortController;

        let (_persistence, service) = service();
        insert_live(&service, "bash-wait0001", true);
        assert_eq!(
            service
                .wait("bash-wait0001", Some(0.0), None)
                .await
                .unwrap()
                .unwrap()
                .base
                .status,
            AgentTaskStatus::Running
        );

        let controller = AbortController::new();
        controller.abort(None);
        assert!(
            service
                .wait("bash-wait0001", Some(30_000.0), Some(controller.signal()))
                .await
                .is_err()
        );

        let waiting_service = service.clone();
        let waiter = tokio::spawn(async move {
            waiting_service
                .wait("bash-wait0001", Some(30_000.0), None)
                .await
        });
        tokio::task::yield_now().await;
        service
            .settle_task(
                "bash-wait0001",
                AgentTaskSettlement {
                    status: AgentTaskSettlementStatus::Completed,
                    stop_reason: None,
                },
                40,
            )
            .await
            .unwrap();
        assert_eq!(
            waiter.await.unwrap().unwrap().unwrap().base.status,
            AgentTaskStatus::Completed
        );
        assert_eq!(service.wait("missing", None, None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn notification_context_reserves_once_reads_fallback_and_honors_delivery_state() {
        let (persistence, service) = service();
        let completed = task("bash-notify01", AgentTaskStatus::Completed, true);
        persistence.write_task(&completed).await.unwrap();
        service
            .load_from_disk(AgentTaskLoadOptions::default())
            .await
            .unwrap();

        let built = service
            .build_agent_task_notification_context(&completed, &[])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(built.notification["type"], "task.completed");
        assert_eq!(built.notification["source_id"], "bash-notify01");
        assert!(
            service
                .build_agent_task_notification_context(&completed, &[])
                .await
                .unwrap()
                .is_none()
        );

        let delivered = task("bash-notify02", AgentTaskStatus::Failed, true);
        let delivered_origin = PromptOrigin::Task {
            task_id: delivered.base.task_id.clone(),
            status: delivered.base.status,
            notification_id: "task:bash-notify02:failed".into(),
        };
        service.mark_delivered_notification(&delivered_origin);
        assert!(
            service
                .build_agent_task_notification_context(&delivered, &[])
                .await
                .unwrap()
                .is_none()
        );

        let restored = task("bash-notify03", AgentTaskStatus::Lost, true);
        service.restore_delivered_notifications([format!(
            "{}\0lost\0task:{}:lost",
            restored.base.task_id, restored.base.task_id
        )]);
        assert!(
            service
                .build_agent_task_notification_context(&restored, &[])
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn reconcile_records_lost_tasks_then_restores_terminal_notifications_in_order() {
        let (persistence, service, effects) = service_with_effects();
        for info in [
            task("bash-running2", AgentTaskStatus::Running, true),
            task("bash-done0002", AgentTaskStatus::Completed, true),
        ] {
            persistence.write_task(&info).await.unwrap();
        }
        service
            .load_from_disk(AgentTaskLoadOptions::default())
            .await
            .unwrap();

        let lost = service.reconcile(100).await.unwrap();
        assert_eq!(lost.len(), 1);
        assert_eq!(
            *effects.terminated.lock().unwrap(),
            ["bash-running2".to_owned()]
        );
        assert_eq!(
            *effects.restored.lock().unwrap(),
            ["bash-done0002".to_owned(), "bash-running2".to_owned()]
        );
        assert!(effects.enqueued.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn live_notification_is_enqueued_instead_of_restored() {
        let (_persistence, service, effects) = service_with_effects();
        let completed = task("bash-notify04", AgentTaskStatus::Completed, true);
        service.notify_agent_task(&completed).await.unwrap();
        assert_eq!(
            *effects.enqueued.lock().unwrap(),
            ["bash-notify04".to_owned()]
        );
        assert!(effects.restored.lock().unwrap().is_empty());
    }

    #[test]
    fn read_output_tail_matches_javascript_utf16_and_zero_semantics() {
        assert_eq!(utf16_tail("a😀b", 2.0), "�b");
        assert_eq!(utf16_tail("a😀b", 3.0), "😀b");
        assert_eq!(utf16_tail("abc", 0.0), "abc");
        assert_eq!(utf16_tail("abc", 0.9), "abc");
        assert_eq!(utf16_tail("abc", f64::NAN), "abc");
        assert_eq!(utf16_tail("abc", f64::INFINITY), "abc");
        assert_eq!(
            javascript_timeout_duration(f64::NAN),
            Duration::from_millis(1)
        );
        assert_eq!(javascript_timeout_duration(1.9), Duration::from_millis(1));
        assert_eq!(
            javascript_timeout_duration(30_000.0),
            Duration::from_millis(30_000)
        );
    }
}
