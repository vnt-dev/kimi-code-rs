//! Runtime task registry and persistence-backed query operations.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `getTask()`, `list()`, `persistOutput()`, `loadFromDisk()`,
//! `markLoadedTasksLost()`, `getOutputSnapshot()`, and `readOutput()`.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, MutexGuard},
};

use futures_util::{FutureExt, future::BoxFuture};
use indexmap::IndexMap;

use super::{
    AgentTaskInfo, AgentTaskLoadOptions, AgentTaskOutputSnapshot, AgentTaskPersistence,
    AgentTaskServiceResult, ManagedTaskState, RestoredTaskRegistry, TaskModelState,
    empty_output_snapshot, should_list_task,
};

const JAVASCRIPT_MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

type TaskWriteBarrier = futures_util::future::Shared<BoxFuture<'static, ()>>;

struct ManagedTaskRuntime {
    state: ManagedTaskState,
    persist_write_queue: TaskWriteBarrier,
    output_write_queue: TaskWriteBarrier,
}

impl ManagedTaskRuntime {
    fn new(state: ManagedTaskState) -> Self {
        Self {
            state,
            persist_write_queue: futures_util::future::ready(()).boxed().shared(),
            output_write_queue: futures_util::future::ready(()).boxed().shared(),
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
}

pub struct AgentTaskService {
    inner: Arc<AgentTaskServiceInner>,
}

struct AgentTaskServiceInner {
    persistence: Arc<AgentTaskPersistence>,
    state: Mutex<AgentTaskServiceState>,
}

impl AgentTaskService {
    pub fn new(persistence: Arc<AgentTaskPersistence>) -> Self {
        Self {
            inner: Arc::new(AgentTaskServiceInner {
                persistence,
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

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{Map, Value};

    use super::*;
    use crate::{
        agent::task::{
            AgentTask, AgentTaskError, AgentTaskInfoBase, AgentTaskSink, AgentTaskStatus,
            RegisterAgentTaskOptions, TaskOutputAction,
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

    fn service() -> (Arc<AgentTaskPersistence>, AgentTaskService) {
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
        let service = AgentTaskService::new(Arc::clone(&persistence));
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

    #[test]
    fn read_output_tail_matches_javascript_utf16_and_zero_semantics() {
        assert_eq!(utf16_tail("a😀b", 2.0), "�b");
        assert_eq!(utf16_tail("a😀b", 3.0), "😀b");
        assert_eq!(utf16_tail("abc", 0.0), "abc");
        assert_eq!(utf16_tail("abc", 0.9), "abc");
        assert_eq!(utf16_tail("abc", f64::NAN), "abc");
        assert_eq!(utf16_tail("abc", f64::INFINITY), "abc");
    }
}
