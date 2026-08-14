//! Runtime task registry and persistence-backed query operations.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `getTask()`, `list()`, `persistOutput()`, `loadFromDisk()`,
//! `markLoadedTasksLost()`, `getOutputSnapshot()`, and `readOutput()`.

use std::{
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
    time::Duration,
};
use std::sync::{Arc};
use parking_lot::Mutex;
use parking_lot::MutexGuard;

use async_trait::async_trait;
use futures_util::{
    FutureExt,
    future::{BoxFuture, join_all},
};
use indexmap::IndexMap;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{
                Disposable, DisposableHandle, DisposableStore, DisposeResult, combined_disposable,
            },
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::error_message::to_error_message,
        errors::unexpected_error::on_unexpected_error,
        utils::abort::{AbortError, AbortSignal, abort_error, abortable, user_cancellation_reason},
    },
    agent::{
        context_injector::{
            AGENT_CONTEXT_INJECTOR_SERVICE_ID, AgentContextInjectorServiceContract,
            ContextInjectionContent, ContextInjectionProvider,
        },
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract,
            ContextMemorySnapshot, ContextMessage, PromptOrigin,
        },
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopServiceContract},
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
    },
    app::{
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle},
        task::contract::TaskState,
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryServiceHandle},
    },
    hooks::HookRegisterOptions,
    persistence::interface::{
        atomic_document_store::{ATOMIC_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreHandle},
        storage::{FILE_SYSTEM_STORAGE_SERVICE_ID, FileSystemStorageServiceHandle},
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    ACTIVE_BACKGROUND_TASK_INJECTION_VARIANT, AGENT_TASK_SERVICE_ID, AgentTask, AgentTaskEntry,
    AgentTaskError, AgentTaskInfo, AgentTaskLifecycleRecorder, AgentTaskLoadOptions,
    AgentTaskNotificationBuildContext, AgentTaskNotificationEffects, AgentTaskOutputSnapshot,
    AgentTaskPersistence, AgentTaskPersistenceRoot, AgentTaskServiceContract,
    AgentTaskServiceHandle, AgentTaskServiceResult, AgentTaskSettlement, AgentTaskSettlementStatus,
    AgentTaskSink, AgentTaskTrackOptions, AgentTrackedTaskHandle, ForegroundTaskReleaseReason,
    ManagedTaskState, NOTIFICATION_FALLBACK_PREVIEW_BYTES, RegisterAgentTaskOptions,
    RestoredTaskRegistry, ScheduledTaskNotification, TASK_MODEL, TASK_NOTIFICATION_DELIVERY_MODEL,
    TaskModelState, TaskNotificationDelivery, TaskOutputAction, active_background_task_reminder,
    check_task_registration, coerce_timeout_settlement, empty_output_snapshot,
    finish_task_notification, generate_task_id, is_compaction_splice,
    needs_notification_fallback_preview, normalize_reason, resolve_agent_task_config,
    should_list_task,
};

const SESSION_CLOSED_REASON: &str = "Session closed";

type TaskWriteBarrier = futures_util::future::Shared<BoxFuture<'static, ()>>;

struct ManagedTaskRuntime {
    state: ManagedTaskState,
    persist_write_queue: TaskWriteBarrier,
    output_write_queue: TaskWriteBarrier,
    timeout_handle: Option<tokio::task::JoinHandle<()>>,
    foreground_signal_task: Option<tokio::task::JoinHandle<()>>,
    handle_subscription: Option<DisposableHandle>,
    settled: tokio::sync::watch::Sender<bool>,
    lifecycle_done: tokio::sync::watch::Sender<bool>,
    tracked_handle: Option<Arc<dyn AgentTrackedTaskHandle>>,
}

impl ManagedTaskRuntime {
    fn new(state: ManagedTaskState) -> Self {
        let (settled, _) = tokio::sync::watch::channel(false);
        let (lifecycle_done, _) = tokio::sync::watch::channel(false);
        Self {
            state,
            persist_write_queue: futures_util::future::ready(()).boxed().shared(),
            output_write_queue: futures_util::future::ready(()).boxed().shared(),
            timeout_handle: None,
            foreground_signal_task: None,
            handle_subscription: None,
            settled,
            lifecycle_done,
            tracked_handle: None,
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
    active_task_reminder_pending: bool,
}

#[derive(Clone)]
pub struct AgentTaskService {
    inner: Arc<AgentTaskServiceInner>,
}

pub trait AgentTaskRuntimeEffects: Send + Sync {
    fn task_config(&self) -> Option<super::AgentTaskConfig>;
    fn context_snapshot(&self) -> ContextMemorySnapshot;
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
    config: crate::app::config::ConfigServiceHandle,
    context: Arc<dyn crate::agent::context_memory::AgentContextMemoryServiceContract>,
    lifecycle: AgentTaskLifecycleRecorder,
    notifications: AgentTaskNotificationEffects,
}

impl DefaultAgentTaskRuntimeEffects {
    pub fn new(
        config: crate::app::config::ConfigServiceHandle,
        context: Arc<dyn crate::agent::context_memory::AgentContextMemoryServiceContract>,
        lifecycle: AgentTaskLifecycleRecorder,
        notifications: AgentTaskNotificationEffects,
    ) -> Self {
        Self {
            config,
            context,
            lifecycle,
            notifications,
        }
    }
}

impl AgentTaskRuntimeEffects for DefaultAgentTaskRuntimeEffects {
    fn task_config(&self) -> Option<super::AgentTaskConfig> {
        resolve_agent_task_config(&self.config)
    }

    fn context_snapshot(&self) -> ContextMemorySnapshot {
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
    disposables: DisposableStore,
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
                disposables: DisposableStore::new(),
            }),
        }
    }

    // Original: AgentTaskService.constructor(). Rust passes explicit handles in
    // place of parameter decorators and retains the source dependency order.
    #[allow(clippy::too_many_arguments)]
    pub fn from_dependencies(
        telemetry: TelemetryServiceHandle,
        context: Arc<dyn AgentContextMemoryServiceContract>,
        config: ConfigServiceHandle,
        documents: AtomicDocumentStoreHandle,
        storage: FileSystemStorageServiceHandle,
        session: &SessionContext,
        scope: &AgentScopeContext,
        wire: WireServiceHandle,
        event_bus: EventBusHandle,
        injector: &dyn AgentContextInjectorServiceContract,
        loop_service: Arc<dyn AgentLoopServiceContract>,
    ) -> AgentTaskServiceResult<Self> {
        let (agent_dir, agent_scope, fallback_root) = task_persistence_coordinates(session, scope);
        let persistence = Arc::new(AgentTaskPersistence::new(
            agent_dir,
            agent_scope,
            documents,
            storage,
            fallback_root,
        ));
        let lifecycle = AgentTaskLifecycleRecorder::new(wire.clone(), telemetry);
        let notifications = AgentTaskNotificationEffects::new(
            Arc::clone(&context),
            event_bus.clone(),
            loop_service,
        );
        let effects: Arc<dyn AgentTaskRuntimeEffects> = Arc::new(
            DefaultAgentTaskRuntimeEffects::new(config, context, lifecycle, notifications),
        );
        let service = Self::new(persistence, effects);
        service.install_restore_hook(&wire)?;
        service.install_context_hooks(&event_bus, injector);
        Ok(service)
    }

    fn state(&self) -> MutexGuard<'_, AgentTaskServiceState> {
        self.inner
            .state
            .lock()
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

    // Original: AgentTaskService.registerTask().
    pub fn register_task(
        &self,
        task: Arc<dyn AgentTask>,
        options: RegisterAgentTaskOptions,
    ) -> AgentTaskServiceResult<String> {
        let detached = options.detached.unwrap_or(true);
        self.check_registration(detached)?;
        let task_id = generate_task_id(task.id_prefix())?;
        let managed = ManagedTaskState::registered(
            task_id.clone(),
            Arc::clone(&task),
            options,
            current_time_millis(),
        );
        let timeout_ms = managed.options.timeout_ms;
        self.insert_managed_task(managed);
        if let Some(timeout_ms) = timeout_ms
            && timeout_ms > 0
        {
            self.arm_manager_timeout(&task_id, timeout_ms);
        }

        let task_signal = self
            .state()
            .tasks
            .get(&task_id)
            .map(|entry| entry.state.abort_controller.signal())
            .ok_or_else(|| std::io::Error::other("registered task state disappeared"))?;
        let (start_sender, start_receiver) = tokio::sync::oneshot::channel();

        let service = self.clone();
        let lifecycle_task_id = task_id.clone();
        tokio::spawn(async move {
            let _ = start_receiver.await;
            let sink = ManagedAgentTaskSink {
                service: service.clone(),
                task_id: lifecycle_task_id.clone(),
                signal: task_signal,
            };
            let start = catch_unwind(AssertUnwindSafe(|| task.start(&sink)));
            let failure = match start {
                Ok(start) => match AssertUnwindSafe(start).catch_unwind().await {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(to_error_message(error.as_ref(), false)),
                    Err(panic) => Some(panic_payload_message(panic)),
                },
                Err(panic) => Some(panic_payload_message(panic)),
            };
            if let Some(failure) = failure {
                let (timed_out, aborted) = {
                    let state = service.state();
                    state
                        .tasks
                        .get(&lifecycle_task_id)
                        .map(|entry| {
                            (
                                entry.state.timed_out,
                                entry.state.abort_controller.signal().aborted(),
                            )
                        })
                        .unwrap_or((false, false))
                };
                let status = if timed_out {
                    AgentTaskSettlementStatus::TimedOut
                } else if aborted {
                    AgentTaskSettlementStatus::Killed
                } else {
                    AgentTaskSettlementStatus::Failed
                };
                let _ = service
                    .settle_task(
                        &lifecycle_task_id,
                        AgentTaskSettlement {
                            status,
                            stop_reason: (status == AgentTaskSettlementStatus::Failed)
                                .then_some(failure),
                        },
                        current_time_millis(),
                    )
                    .await;
            }
            service.mark_lifecycle_done(&lifecycle_task_id);
        });
        self.install_foreground_signal(&task_id);
        let initial_record = self.record_initial_detached_task(&task_id);
        let _ = start_sender.send(());
        initial_record?;
        Ok(task_id)
    }

    // Original: AgentTaskService.track().
    pub fn track(
        &self,
        handle: Arc<dyn AgentTrackedTaskHandle>,
        options: AgentTaskTrackOptions,
    ) -> AgentTaskServiceResult<AgentTaskEntry> {
        let detached = options.detached.unwrap_or(true);
        self.check_registration(detached)?;
        let task_id = generate_task_id(options.id_prefix.as_deref().unwrap_or("task"))?;
        let timeout_ms = options.timeout_ms;
        let managed = ManagedTaskState::tracked(task_id.clone(), options, current_time_millis());
        let on_did_detach = managed.foreground_release_future();
        self.insert_managed_task(managed);
        if let Some(entry) = self.state().tasks.get_mut(&task_id) {
            entry.tracked_handle = Some(Arc::clone(&handle));
        }
        if let Some(timeout_ms) = timeout_ms
            && timeout_ms > 0
        {
            self.arm_manager_timeout(&task_id, timeout_ms);
        }

        let output_service = self.clone();
        let output_task_id = task_id.clone();
        let output_subscription = handle.on_did_output().subscribe(move |chunk| {
            output_service.append_output(&output_task_id, chunk.clone());
        });
        let state_service = self.clone();
        let state_task_id = task_id.clone();
        let state_subscription = handle.on_did_change_state().subscribe(move |task_state| {
            if !task_state.is_terminal() {
                return;
            }
            let service = state_service.clone();
            let task_id = state_task_id.clone();
            let task_state = *task_state;
            tokio::spawn(async move {
                service.settle_tracked_state(&task_id, task_state).await;
            });
        });
        let subscription = combined_disposable(vec![output_subscription, state_subscription]);
        let dispose_immediately = {
            let mut state = self.state();
            match state.tasks.get_mut(&task_id) {
                None => true,
                Some(entry) if entry.state.status.is_terminal() => true,
                Some(entry) => {
                    entry.handle_subscription = Some(Arc::clone(&subscription));
                    false
                }
            }
        };
        if dispose_immediately {
            subscription.dispose()?;
        }

        let lifecycle_service = self.clone();
        let lifecycle_task_id = task_id.clone();
        let lifecycle_handle = Arc::clone(&handle);
        tokio::spawn(async move {
            lifecycle_handle.settled().await;
            lifecycle_service.mark_lifecycle_done(&lifecycle_task_id);
        });
        self.install_foreground_signal(&task_id);
        self.record_initial_detached_task(&task_id)?;
        Ok(AgentTaskEntry {
            task_id,
            on_did_detach,
        })
    }

    fn check_registration(&self, detached: bool) -> AgentTaskServiceResult<()> {
        let state = self.state();
        let active = state
            .tasks
            .values()
            .filter(|entry| !entry.state.status.is_terminal() && entry.state.starts_detached())
            .count();
        drop(state);
        let maximum = self
            .inner
            .effects
            .task_config()
            .and_then(|config| config.max_running_tasks);
        check_task_registration(detached, active, maximum)?;
        Ok(())
    }

    fn record_initial_detached_task(&self, task_id: &str) -> AgentTaskServiceResult<()> {
        let info = {
            let persistence = Arc::clone(&self.inner.persistence);
            let mut state = self.state();
            let Some(entry) = state.tasks.get_mut(task_id) else {
                return Ok(());
            };
            if !entry.state.is_detached() {
                return Ok(());
            }
            drop(entry.persist_live(persistence));
            entry.state.to_info()
        };
        self.inner.effects.record_task_started(&info)
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

    // Original: AgentTaskService.restoreAfterReplay(). Restore-derived state is
    // installed before disk merge, reconciliation, and terminal notification
    // restoration; `replace: false` retains Wire-only entries while matching
    // disk entries still use the original `newerRestoredTask()` arbitration.
    pub async fn restore_after_replay(
        &self,
        wire_tasks: &TaskModelState,
        delivered_keys: &[String],
        now_ms: i64,
    ) -> AgentTaskServiceResult<Vec<AgentTaskInfo>> {
        self.restore_delivered_notifications(delivered_keys.iter().cloned());
        self.restore_ghosts_from_wire(wire_tasks);
        self.load_from_disk(AgentTaskLoadOptions {
            replace: Some(false),
        })
        .await?;
        self.reconcile(now_ms).await
    }

    // Original: AgentTaskService constructor's `context.spliced` subscription.
    // Compaction arms the reminder before every inserted task-origin message is
    // recorded as delivered.
    pub fn handle_context_splice(&self, delete_count: usize, messages: &[ContextMessage]) {
        let mut state = self.state();
        if is_compaction_splice(delete_count, messages) {
            state.active_task_reminder_pending = true;
        }
        for message in messages {
            if let Some(origin) = &message.origin {
                state.notifications.mark_delivered(origin);
            }
        }
    }

    // Original: AgentTaskService constructor's EventBus subscription and
    // AgentContextInjector registration.
    pub fn install_context_hooks(
        &self,
        event_bus: &EventBusHandle,
        injector: &dyn AgentContextInjectorServiceContract,
    ) {
        let service = self.clone();
        self.inner.disposables.add(event_bus.subscribe_type(
            "context.spliced",
            Arc::new(move |event| {
                if let Some((delete_count, messages)) = context_splice_event(event) {
                    service.handle_context_splice(delete_count, &messages);
                }
            }),
        ));

        let service = self.clone();
        let provider: ContextInjectionProvider = Arc::new(move |_| {
            let service = service.clone();
            async move {
                Ok(service
                    .active_background_task_reminder()
                    .map(ContextInjectionContent::Text))
            }
            .boxed()
        });
        self.inner
            .disposables
            .add(injector.register(ACTIVE_BACKGROUND_TASK_INJECTION_VARIANT.into(), provider));
    }

    // Original: AgentTaskService constructor's Wire onDidRestore hook. Models
    // must be registered before replay begins; the hook itself runs restoration
    // before forwarding to the next ordered handler.
    pub fn install_restore_hook(&self, wire: &WireServiceHandle) -> AgentTaskServiceResult<()> {
        std::sync::LazyLock::force(&TASK_MODEL);
        std::sync::LazyLock::force(&TASK_NOTIFICATION_DELIVERY_MODEL);
        let service = self.clone();
        let wire_for_hook = wire.clone();
        let disposable = wire.hooks().on_did_restore.register(
            "task",
            Arc::new(move |context, next| {
                let service = service.clone();
                let wire = wire_for_hook.clone();
                async move {
                    let wire_tasks = wire.get_model(&TASK_MODEL);
                    let delivered = wire.get_model(&TASK_NOTIFICATION_DELIVERY_MODEL);
                    service
                        .restore_after_replay(&wire_tasks, &delivered, current_time_millis())
                        .await?;
                    next(context).await
                }
                .boxed()
            }),
            HookRegisterOptions::default(),
        )?;
        self.inner.disposables.add(disposable);
        Ok(())
    }

    // Original: AgentTaskService.activeBackgroundTaskReminder(). The pending
    // flag is consumed even when no active task remains.
    pub fn active_background_task_reminder(&self) -> Option<String> {
        let mut state = self.state();
        let active_tasks = state
            .tasks
            .values()
            .map(|entry| entry.state.to_info())
            .filter(|info| should_list_task(info, true))
            .collect::<Vec<_>>();
        active_background_task_reminder(&mut state.active_task_reminder_pending, &active_tasks)
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
        max_preview_bytes: u64,
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
        tail: Option<u64>,
    ) -> AgentTaskServiceResult<String> {
        let output = self.get_output_snapshot(task_id, u64::MAX).await?.preview;
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
        let (timeout, signal_task, subscription, output_persist_started) = {
            let mut state = self.state();
            let Some(entry) = state.tasks.get_mut(task_id) else {
                return Ok(false);
            };
            if !entry.state.apply_settlement(settlement, now_ms) {
                return Ok(false);
            }
            (
                entry.timeout_handle.take(),
                entry.foreground_signal_task.take(),
                entry.handle_subscription.take(),
                entry.state.output.output_persist_started,
            )
        };
        if let Some(timeout) = timeout {
            timeout.abort();
        }
        if let Some(signal_task) = signal_task {
            signal_task.abort();
        }
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

    // Original: AgentTaskService.stop().
    pub async fn stop(
        &self,
        task_id: &str,
        reason: Option<&str>,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        if !self.state().tasks.contains_key(task_id) {
            return Ok(None);
        }
        let reason = normalize_reason(reason).map(str::to_owned);
        self.terminate_with_grace(
            task_id,
            reason.clone(),
            abort_error(reason.as_deref()),
            AgentTaskSettlementStatus::Killed,
        )
        .await
    }

    // Original: AgentTaskService.stopByUser().
    pub async fn stop_by_user(
        &self,
        task_id: &str,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        if !self.state().tasks.contains_key(task_id) {
            return Ok(None);
        }
        let reason = user_cancellation_reason();
        self.terminate_with_grace(
            task_id,
            Some(reason.to_string()),
            reason,
            AgentTaskSettlementStatus::Killed,
        )
        .await
    }

    // Original: AgentTaskService.stopAll(). Calls begin together like
    // Promise.all and results retain task Map order.
    pub async fn stop_all(
        &self,
        reason: Option<&str>,
    ) -> AgentTaskServiceResult<Vec<AgentTaskInfo>> {
        let task_ids = self.state().tasks.keys().cloned().collect::<Vec<_>>();
        let reason = reason.map(str::to_owned);
        let results = join_all(task_ids.iter().map(|task_id| {
            let reason = reason.clone();
            async move { self.stop(task_id, reason.as_deref()).await }
        }))
        .await;
        let mut stopped = Vec::new();
        for result in results {
            if let Some(info) = result? {
                stopped.push(info);
            }
        }
        Ok(stopped)
    }

    // Original: AgentTaskService.stopAllOnExit().
    pub async fn stop_all_on_exit(
        &self,
        reason: &str,
    ) -> AgentTaskServiceResult<Vec<AgentTaskInfo>> {
        if self.keep_alive_on_exit() {
            return Ok(Vec::new());
        }
        let detached_ids = self
            .list(None, None)
            .into_iter()
            .filter(|info| info.base.detached == Some(true))
            .map(|info| info.base.task_id)
            .collect::<Vec<_>>();
        let suppressions = join_all(
            detached_ids
                .iter()
                .map(|task_id| self.suppress_terminal_notification(task_id)),
        )
        .await;
        for suppression in suppressions {
            suppression?;
        }
        self.stop_all(Some(reason)).await
    }

    fn keep_alive_on_exit(&self) -> bool {
        self.inner
            .effects
            .task_config()
            .is_some_and(|config| config.keep_alive_on_exit == Some(true))
    }

    // Original: AgentTaskService.dispose() and forceStopOnDispose().
    fn dispose_tasks(&self) {
        if self.keep_alive_on_exit() {
            return;
        }
        let actions = {
            let mut state = self.state();
            state
                .tasks
                .values_mut()
                .filter(|entry| !entry.state.status.is_terminal())
                .map(|entry| {
                    (
                        entry.timeout_handle.take(),
                        entry.tracked_handle.clone(),
                        entry.state.abort_controller.clone(),
                        entry.state.force_stop.clone(),
                        entry.state.projection.registered_task(),
                    )
                })
                .collect::<Vec<_>>()
        };
        for (timeout, handle, abort_controller, callback, task) in actions {
            if let Some(timeout) = timeout {
                timeout.abort();
            }
            if let Some(handle) = handle {
                handle.cancel();
            } else {
                abort_controller.abort(Some(AbortError::new(SESSION_CLOSED_REASON)));
            }
            let force_stop = catch_unwind(AssertUnwindSafe(|| {
                callback
                    .map(|callback| callback())
                    .or_else(|| task.map(|task| async move { task.force_stop().await }.boxed()))
            }));
            if let Ok(Some(force_stop)) = force_stop {
                tokio::spawn(async move {
                    let _ = AssertUnwindSafe(force_stop).catch_unwind().await;
                });
            }
        }
    }

    async fn terminate_with_grace(
        &self,
        task_id: &str,
        stop_reason: Option<String>,
        abort_reason: AbortError,
        final_status: AgentTaskSettlementStatus,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        if let Some(info) = self.terminal_info(task_id).await {
            return Ok(Some(info));
        }

        let (timeout, tracked_handle, abort_controller, mut lifecycle_done) = {
            let mut state = self.state();
            let Some(entry) = state.tasks.get_mut(task_id) else {
                return Ok(None);
            };
            if final_status == AgentTaskSettlementStatus::TimedOut {
                entry.state.timed_out = true;
            }
            entry.state.stop_reason = stop_reason.clone();
            (
                entry.timeout_handle.take(),
                entry.tracked_handle.clone(),
                entry.state.abort_controller.clone(),
                entry.lifecycle_done.subscribe(),
            )
        };
        if let Some(timeout) = timeout {
            timeout.abort();
        }
        if let Some(handle) = tracked_handle {
            handle.cancel();
        } else {
            abort_controller.abort(Some(abort_reason));
        }

        let grace_ms = self
            .inner
            .effects
            .task_config()
            .and_then(|config| config.kill_grace_period_ms)
            .unwrap_or(5_000);
        let graceful = if *lifecycle_done.borrow() {
            true
        } else {
            tokio::select! {
                biased;
                () = wait_until_settled(&mut lifecycle_done) => true,
                () = tokio::time::sleep(Duration::from_millis(grace_ms)) => false,
            }
        };

        if let Some(info) = self.terminal_info(task_id).await {
            return Ok(Some(info));
        }
        if !graceful {
            let (callback, task) = {
                let state = self.state();
                let Some(entry) = state.tasks.get(task_id) else {
                    return Ok(None);
                };
                (
                    entry.state.force_stop.clone(),
                    entry.state.projection.registered_task(),
                )
            };
            if let Some(callback) = callback {
                if let Ok(force_stop) = catch_unwind(AssertUnwindSafe(|| callback())) {
                    let _ = AssertUnwindSafe(force_stop).catch_unwind().await;
                }
            } else if let Some(task) = task
                && let Ok(force_stop) = catch_unwind(AssertUnwindSafe(|| task.force_stop()))
            {
                let _ = AssertUnwindSafe(force_stop).catch_unwind().await;
            }
        }

        if let Some(info) = self.terminal_info(task_id).await {
            return Ok(Some(info));
        }
        self.settle_task(
            task_id,
            AgentTaskSettlement {
                status: final_status,
                stop_reason,
            },
            current_time_millis(),
        )
        .await?;
        Ok(self.terminal_info(task_id).await)
    }

    // Original: AgentTaskService.detach() and detachEntry().
    pub fn detach(&self, task_id: &str) -> Option<AgentTaskInfo> {
        self.detach_entry(task_id, false)
    }

    fn detach_entry(&self, task_id: &str, via_timeout: bool) -> Option<AgentTaskInfo> {
        let (release, signal_task, old_timeout, detach_timeout, callback, task) = {
            let mut state = self.state();
            let entry = state.tasks.get_mut(task_id)?;
            if entry.state.status.is_terminal() || entry.state.is_detached() {
                return Some(entry.state.to_info());
            }
            let release = entry.state.take_foreground_release()?;
            let detach_timeout = entry.state.apply_detach_timeout();
            let old_timeout = detach_timeout.and_then(|_| entry.timeout_handle.take());
            (
                release,
                entry.foreground_signal_task.take(),
                old_timeout,
                detach_timeout,
                entry.state.on_detach.clone(),
                entry.state.projection.registered_task(),
            )
        };
        if let Some(signal_task) = signal_task {
            signal_task.abort();
        }
        if let Some(timeout) = old_timeout {
            timeout.abort();
        }
        if let Some(timeout_ms) = detach_timeout
            && timeout_ms > 0
        {
            self.arm_manager_timeout(task_id, timeout_ms);
        }
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Some(callback) = callback {
                callback();
            } else if let Some(task) = task {
                task.on_detach();
            }
        }));

        let info = {
            let persistence = Arc::clone(&self.inner.persistence);
            let mut state = self.state();
            let entry = state.tasks.get_mut(task_id)?;
            if let Some(pending) = entry.state.output.start_output_persist() {
                entry.append_task_output(Arc::clone(&persistence), pending);
            }
            drop(entry.persist_live(persistence));
            entry.state.to_info()
        };
        if let Err(error) = self.inner.effects.record_task_started(&info) {
            on_unexpected_error(error.as_ref());
        }
        release.resolve(if via_timeout {
            ForegroundTaskReleaseReason::TimeoutDetached
        } else {
            ForegroundTaskReleaseReason::Detached
        });
        Some(self.get_task(task_id).unwrap_or(info))
    }

    fn append_output(&self, task_id: &str, chunk: String) {
        let stop_reason = {
            let persistence = Arc::clone(&self.inner.persistence);
            let mut state = self.state();
            let Some(entry) = state.tasks.get_mut(task_id) else {
                return;
            };
            let is_process = entry.state.projection.enforces_process_output_limit();
            match entry.state.output.append(chunk, is_process) {
                TaskOutputAction::None => None,
                TaskOutputAction::AppendPersisted(chunk) => {
                    entry.append_task_output(persistence, chunk);
                    None
                }
                TaskOutputAction::StartPersisting(chunk) => {
                    entry.append_task_output(persistence, chunk);
                    None
                }
                TaskOutputAction::StopProcess(reason) => Some(reason),
            }
        };
        if let Some(reason) = stop_reason {
            let service = self.clone();
            let task_id = task_id.to_owned();
            tokio::spawn(async move {
                let _ = service.stop(&task_id, Some(&reason)).await;
            });
        }
    }

    async fn settle_tracked_state(&self, task_id: &str, task_state: TaskState) {
        let (timed_out, stop_reason) = {
            let state = self.state();
            let Some(entry) = state.tasks.get(task_id) else {
                return;
            };
            (entry.state.timed_out, entry.state.stop_reason.clone())
        };
        let status = if timed_out {
            AgentTaskSettlementStatus::TimedOut
        } else {
            match task_state {
                TaskState::Cancelled => AgentTaskSettlementStatus::Killed,
                TaskState::Failed => AgentTaskSettlementStatus::Failed,
                TaskState::Completed => AgentTaskSettlementStatus::Completed,
                TaskState::Pending | TaskState::Running => return,
            }
        };
        let _ = self
            .settle_task(
                task_id,
                AgentTaskSettlement {
                    status,
                    stop_reason,
                },
                current_time_millis(),
            )
            .await;
    }

    fn mark_lifecycle_done(&self, task_id: &str) {
        if let Some(entry) = self.state().tasks.get(task_id) {
            entry.lifecycle_done.send_replace(true);
        }
    }

    // Original: AgentTaskService.installForegroundSignal().
    fn install_foreground_signal(&self, task_id: &str) {
        let signal = {
            let state = self.state();
            let Some(entry) = state.tasks.get(task_id) else {
                return;
            };
            if entry.state.is_detached() {
                return;
            }
            entry.state.options.signal.clone()
        };
        let Some(signal) = signal else {
            return;
        };
        let service = self.clone();
        let task_id_owned = task_id.to_owned();
        let signal_for_task = signal.clone();
        let task = tokio::spawn(async move {
            let reason = signal_for_task.cancelled().await;
            let should_stop = service
                .state()
                .tasks
                .get(&task_id_owned)
                .is_some_and(|entry| {
                    !entry.state.is_detached() && !entry.state.status.is_terminal()
                });
            if !should_stop {
                return;
            }
            let user_reason = user_cancellation_reason();
            let _ = service
                .terminate_with_grace(
                    &task_id_owned,
                    Some(user_reason.to_string()),
                    (*reason).clone(),
                    AgentTaskSettlementStatus::Killed,
                )
                .await;
        });
        let previous = {
            let mut state = self.state();
            let Some(entry) = state.tasks.get_mut(task_id) else {
                task.abort();
                return;
            };
            entry.foreground_signal_task.replace(task)
        };
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    // Original: AgentTaskService.armManagerTimeout().
    fn arm_manager_timeout(&self, task_id: &str, timeout_ms: u64) {
        let service = self.clone();
        let task_id_owned = task_id.to_owned();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
            let auto_background = {
                let mut state = service.state();
                let Some(entry) = state.tasks.get_mut(&task_id_owned) else {
                    return;
                };
                entry.timeout_handle = None;
                entry.state.can_auto_background_on_timeout()
            };
            if auto_background {
                service.detach_entry(&task_id_owned, true);
            } else {
                let _ = service
                    .terminate_with_grace(
                        &task_id_owned,
                        None,
                        AbortError::new("Timed out"),
                        AgentTaskSettlementStatus::TimedOut,
                    )
                    .await;
            }
        });
        let previous = {
            let mut state = self.state();
            let Some(entry) = state.tasks.get_mut(task_id) else {
                handle.abort();
                return;
            };
            entry.timeout_handle.replace(handle)
        };
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    async fn terminal_info(&self, task_id: &str) -> Option<AgentTaskInfo> {
        let barrier = {
            let state = self.state();
            let entry = state.tasks.get(task_id)?;
            if !entry.state.status.is_terminal() {
                return None;
            }
            entry.persist_write_queue.clone()
        };
        barrier.await;
        self.get_task(task_id)
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
                    entry.lifecycle_done.subscribe(),
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
        timeout_ms: Option<u64>,
        signal: Option<AbortSignal>,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        let timeout_ms = timeout_ms.unwrap_or(30_000);
        let (mut settled, terminal_barrier) = {
            let state = self.state();
            let Some(entry) = state.tasks.get(task_id) else {
                return Ok(state.ghosts.get(task_id).cloned());
            };
            if entry.state.status.is_terminal() {
                (None, Some(entry.persist_write_queue.clone()))
            } else if timeout_ms == 0 {
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
        let mut output = self.get_output_snapshot(&info.base.task_id, 0).await?;
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

struct ManagedAgentTaskSink {
    service: AgentTaskService,
    task_id: String,
    signal: AbortSignal,
}

#[async_trait]
impl AgentTaskSink for ManagedAgentTaskSink {
    fn signal(&self) -> AbortSignal {
        self.signal.clone()
    }

    fn append_output(&self, chunk: &str) {
        self.service.append_output(&self.task_id, chunk.into());
    }

    async fn settle(&self, settlement: AgentTaskSettlement) -> Result<bool, AgentTaskError> {
        let timed_out = self
            .service
            .state()
            .tasks
            .get(&self.task_id)
            .is_some_and(|entry| entry.state.timed_out);
        self.service
            .settle_task(
                &self.task_id,
                coerce_timeout_settlement(timed_out, settlement),
                current_time_millis(),
            )
            .await
    }
}

impl Disposable for AgentTaskService {
    fn dispose(&self) -> DisposeResult {
        self.dispose_tasks();
        self.inner.disposables.dispose()
    }
}

// Original: `AgentTaskService implements IAgentTaskService`. Keeping this as a
// direct delegation layer preserves the method-level mapping while allowing the
// concrete service to be stored behind the DI contract handle.
#[async_trait]
impl AgentTaskServiceContract for AgentTaskService {
    fn track(
        &self,
        handle: Arc<dyn AgentTrackedTaskHandle>,
        options: AgentTaskTrackOptions,
    ) -> AgentTaskServiceResult<AgentTaskEntry> {
        AgentTaskService::track(self, handle, options)
    }

    fn register_task(
        &self,
        task: Arc<dyn AgentTask>,
        options: RegisterAgentTaskOptions,
    ) -> AgentTaskServiceResult<String> {
        AgentTaskService::register_task(self, task, options)
    }

    fn get_task(&self, task_id: &str) -> Option<AgentTaskInfo> {
        AgentTaskService::get_task(self, task_id)
    }

    fn list(&self, active_only: Option<bool>, limit: Option<usize>) -> Vec<AgentTaskInfo> {
        AgentTaskService::list(self, active_only, limit)
    }

    fn persist_output(&self, task_id: &str) {
        AgentTaskService::persist_output(self, task_id);
    }

    async fn get_output_snapshot(
        &self,
        task_id: &str,
        max_preview_bytes: u64,
    ) -> AgentTaskServiceResult<AgentTaskOutputSnapshot> {
        AgentTaskService::get_output_snapshot(self, task_id, max_preview_bytes).await
    }

    async fn read_output(
        &self,
        task_id: &str,
        tail: Option<u64>,
    ) -> AgentTaskServiceResult<String> {
        AgentTaskService::read_output(self, task_id, tail).await
    }

    async fn suppress_terminal_notification(&self, task_id: &str) -> AgentTaskServiceResult<()> {
        AgentTaskService::suppress_terminal_notification(self, task_id).await
    }

    fn detach(&self, task_id: &str) -> Option<AgentTaskInfo> {
        AgentTaskService::detach(self, task_id)
    }

    async fn stop(
        &self,
        task_id: &str,
        reason: Option<&str>,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        AgentTaskService::stop(self, task_id, reason).await
    }

    async fn stop_by_user(&self, task_id: &str) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        AgentTaskService::stop_by_user(self, task_id).await
    }

    async fn stop_all(&self, reason: Option<&str>) -> AgentTaskServiceResult<Vec<AgentTaskInfo>> {
        AgentTaskService::stop_all(self, reason).await
    }

    async fn stop_all_on_exit(&self, reason: &str) -> AgentTaskServiceResult<Vec<AgentTaskInfo>> {
        AgentTaskService::stop_all_on_exit(self, reason).await
    }

    async fn wait(
        &self,
        task_id: &str,
        timeout_ms: Option<u64>,
        signal: Option<AbortSignal>,
    ) -> AgentTaskServiceResult<Option<AgentTaskInfo>> {
        AgentTaskService::wait(self, task_id, timeout_ms, signal).await
    }

    async fn wait_for_foreground_release(
        &self,
        task_id: &str,
    ) -> AgentTaskServiceResult<Option<ForegroundTaskReleaseReason>> {
        AgentTaskService::wait_for_foreground_release(self, task_id).await
    }
}

// Original: registerScopedService(LifecycleScope.Agent, IAgentTaskService,
// AgentTaskService, InstantiationType.Eager, "task").
pub fn register_agent_task_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TASK_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let telemetry = accessor.get(TELEMETRY_SERVICE_ID)?;
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let documents = accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?;
            let storage = accessor.get(FILE_SYSTEM_STORAGE_SERVICE_ID)?;
            let session = accessor.get(SESSION_CONTEXT_ID)?;
            let scope = accessor.get(AGENT_SCOPE_CONTEXT_ID)?;
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let injector = accessor.get(AGENT_CONTEXT_INJECTOR_SERVICE_ID)?;
            let loop_service = accessor.get(AGENT_LOOP_SERVICE_ID)?;
            let service = AgentTaskService::from_dependencies(
                (*telemetry).clone(),
                Arc::clone(&context.0),
                (*config).clone(),
                (*documents).clone(),
                (*storage).clone(),
                &session,
                &scope,
                (*wire).clone(),
                (*event_bus).clone(),
                injector.0.as_ref(),
                Arc::clone(&loop_service.0),
            )
            .map_err(|error| crate::_base::di::errors::DiError::Factory(error.to_string()))?;
            let service: Arc<dyn AgentTaskServiceContract> = Arc::new(service);
            Ok(AgentTaskServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "task",
    );
}

fn utf16_tail(value: &str, tail: u64) -> String {
    let units = value.encode_utf16().collect::<Vec<_>>();
    let requested = if tail == 0 {
        units.len()
    } else {
        (tail as usize).min(units.len())
    };
    String::from_utf16_lossy(&units[units.len() - requested..])
}

fn context_splice_event(event: &DomainEvent) -> Option<(usize, Vec<ContextMessage>)> {
    let delete_count = usize::try_from(event.fields.get("deleteCount")?.as_u64()?).ok()?;
    let messages = serde_json::from_value(event.fields.get("messages")?.clone()).ok()?;
    Some((delete_count, messages))
}

fn task_persistence_coordinates(
    session: &SessionContext,
    scope: &AgentScopeContext,
) -> (std::path::PathBuf, String, Option<AgentTaskPersistenceRoot>) {
    let session_dir = std::path::PathBuf::from(&session.session_dir);
    let fallback_root = (scope.agent_id == "main").then(|| AgentTaskPersistenceRoot {
        dir: session_dir.clone(),
        scope: session.scope(None),
    });
    (
        session_dir.join("agents").join(&scope.agent_id),
        scope.scope(None),
        fallback_root,
    )
}

async fn wait_until_settled(receiver: &mut tokio::sync::watch::Receiver<bool>) {
    while !*receiver.borrow() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

fn javascript_timeout_duration(timeout_ms: u64) -> Duration {
    const MAX_NODE_TIMEOUT_MS: u64 = 2_147_483_647;
    Duration::from_millis(timeout_ms.clamp(1, MAX_NODE_TIMEOUT_MS))
}

fn current_time_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("task panicked")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use crate::_base::event::{Emitter, Event};
    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::{Map, Value};

    use super::*;
    use crate::{
        _base::di::lifecycle::{DisposableHandle, disposable_none, to_disposable},
        agent::context_injector::{ContextInjectionContext, ContextInjectionError},
        agent::task::{
            AgentTask, AgentTaskConfig, AgentTaskError, AgentTaskInfoBase,
            AgentTaskSettlementStatus, AgentTaskSink, AgentTaskStatus, AgentTaskTrackOptions,
            RegisterAgentTaskOptions, TaskOutputAction, task_started,
        },
        app::event::{event_bus::EventBusContract, event_bus_service::EventBusService},
        kosong::contract::message::{Message, Role},
        persistence::{
            backends::{
                memory::in_memory_storage_service::InMemoryStorageService,
                node_fs::atomic_document_store::JsonAtomicDocumentStore,
            },
            interface::{
                append_log_store::{
                    AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
                    AppendLogValueStream,
                },
                atomic_document_store::{AtomicDocumentStoreHandle, AtomicDocumentStoreService},
                storage::{FileSystemStorageService, FileSystemStorageServiceHandle},
            },
        },
        wire::wire_service::{DomainEventPublisher, WireBlobService, WireService},
    };

    struct StubTask;

    #[derive(Default)]
    struct EmptyAppendLog;

    #[async_trait]
    impl AppendLogStoreService for EmptyAppendLog {
        fn append_value(&self, _: &str, _: &str, _: Value, _: AppendLogOptions) {}

        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::empty())
        }

        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            _: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn flush(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        fn acquire(&self, _: &str, _: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    struct IdentityBlobs;

    #[async_trait]
    impl WireBlobService for IdentityBlobs {
        async fn offload_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }

        async fn load_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    struct NoopDomainEvents;

    impl DomainEventPublisher for NoopDomainEvents {
        fn publish(&self, _: Value) {}
    }

    #[derive(Default)]
    struct TestContextInjector {
        name: Mutex<Option<String>>,
        provider: Arc<Mutex<Option<ContextInjectionProvider>>>,
    }

    #[async_trait]
    impl AgentContextInjectorServiceContract for TestContextInjector {
        fn register(&self, name: String, provider: ContextInjectionProvider) -> DisposableHandle {
            *self.name.lock() = Some(name);
            *self.provider.lock() = Some(provider);
            let providers = Arc::clone(&self.provider);
            to_disposable(move || {
                *providers.lock() = None;
            })
        }

        async fn inject_after_compaction(&self) -> Result<(), ContextInjectionError> {
            Ok(())
        }
    }

    impl Disposable for TestContextInjector {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    struct CompletingTask;

    struct TestTrackedHandle {
        id: String,
        state: Mutex<TaskState>,
        state_events: Emitter<TaskState>,
        output_events: Emitter<String>,
        lifecycle_done: tokio::sync::watch::Sender<bool>,
    }

    impl TestTrackedHandle {
        fn new(id: &str) -> Self {
            let (lifecycle_done, _) = tokio::sync::watch::channel(false);
            Self {
                id: id.into(),
                state: Mutex::new(TaskState::Running),
                state_events: Emitter::new(),
                output_events: Emitter::new(),
                lifecycle_done,
            }
        }

        fn emit_output(&self, output: &str) {
            self.output_events.fire(&output.to_owned());
        }

        fn transition(&self, state: TaskState) {
            *self.state.lock() = state;
            self.state_events.fire(&state);
            if state.is_terminal() {
                self.lifecycle_done.send_replace(true);
            }
        }
    }

    #[async_trait]
    impl AgentTrackedTaskHandle for TestTrackedHandle {
        fn id(&self) -> &str {
            &self.id
        }

        fn state(&self) -> TaskState {
            *self.state.lock()
        }

        async fn settled(&self) {
            let mut receiver = self.lifecycle_done.subscribe();
            wait_until_settled(&mut receiver).await;
        }

        fn on_did_change_state(&self) -> Event<TaskState> {
            self.state_events.event()
        }

        fn on_did_output(&self) -> Event<String> {
            self.output_events.event()
        }

        fn cancel(&self) {
            self.transition(TaskState::Cancelled);
        }
    }

    #[async_trait]
    impl AgentTask for CompletingTask {
        fn id_prefix(&self) -> &str {
            "bash"
        }

        fn kind(&self) -> &str {
            "process"
        }

        fn description(&self) -> &str {
            "completing"
        }

        async fn start(&self, sink: &dyn AgentTaskSink) -> Result<(), AgentTaskError> {
            assert!(!sink.signal().aborted());
            sink.append_output("registered output");
            sink.settle(AgentTaskSettlement {
                status: AgentTaskSettlementStatus::Completed,
                stop_reason: None,
            })
            .await?;
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

    #[derive(Default)]
    struct RecordingEffects {
        context: Mutex<Vec<ContextMessage>>,
        keep_alive_on_exit: Mutex<Option<bool>>,
        max_running_tasks: Mutex<Option<u64>>,
        started: Mutex<Vec<String>>,
        terminated: Mutex<Vec<String>>,
        enqueued: Mutex<Vec<String>>,
        restored: Mutex<Vec<String>>,
    }

    impl AgentTaskRuntimeEffects for RecordingEffects {
        fn task_config(&self) -> Option<AgentTaskConfig> {
            Some(AgentTaskConfig {
                kill_grace_period_ms: Some(0),
                keep_alive_on_exit: *self.keep_alive_on_exit.lock(),
                max_running_tasks: *self.max_running_tasks.lock(),
                ..AgentTaskConfig::default()
            })
        }

        fn context_snapshot(&self) -> ContextMemorySnapshot {
            self.context.lock().clone().into()
        }

        fn record_task_started(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()> {
            self.started.lock().push(info.base.task_id.clone());
            Ok(())
        }

        fn record_task_terminated(&self, info: &AgentTaskInfo) -> AgentTaskServiceResult<()> {
            self.terminated
                .lock()
                .push(info.base.task_id.clone());
            Ok(())
        }

        fn enqueue_notification(
            &self,
            built: &AgentTaskNotificationBuildContext,
        ) -> AgentTaskServiceResult<()> {
            self.enqueued
                .lock()
                .push(built.hook_context.source_id.clone());
            Ok(())
        }

        fn restore_notification(
            &self,
            built: &AgentTaskNotificationBuildContext,
        ) -> AgentTaskServiceResult<()> {
            self.restored
                .lock()
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

    fn context_message(origin: PromptOrigin) -> ContextMessage {
        ContextMessage {
            message: Message::new(Role::User, vec![], vec![]),
            id: None,
            provider_message_id: None,
            origin: Some(origin),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
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

    #[tokio::test]
    async fn register_starts_task_routes_sink_and_records_detached_lifecycle() {
        let (_persistence, service, effects) = service_with_effects();
        let task_id = service
            .register_task(
                Arc::new(CompletingTask),
                RegisterAgentTaskOptions::default(),
            )
            .unwrap();
        assert!(task_id.starts_with("bash-"));
        let completed = service
            .wait(&task_id, Some(30_000), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.base.status, AgentTaskStatus::Completed);
        assert_eq!(
            service
                .get_output_snapshot(&task_id, 100)
                .await
                .unwrap()
                .preview,
            "registered output"
        );
        assert_eq!(
            effects.started.lock().as_slice(),
            [task_id.as_str()]
        );
        assert_eq!(
            effects.terminated.lock().as_slice(),
            [task_id.as_str()]
        );
    }

    #[tokio::test]
    async fn registration_quota_counts_only_tasks_that_started_detached() {
        let (_persistence, service, effects) = service_with_effects();
        *effects.max_running_tasks.lock() = Some(1);
        service
            .register_task(Arc::new(StubTask), RegisterAgentTaskOptions::default())
            .unwrap();
        assert!(
            service
                .register_task(Arc::new(StubTask), RegisterAgentTaskOptions::default())
                .is_err()
        );
        assert!(
            service
                .register_task(
                    Arc::new(StubTask),
                    RegisterAgentTaskOptions {
                        detached: Some(false),
                        ..RegisterAgentTaskOptions::default()
                    }
                )
                .is_ok()
        );
    }

    #[tokio::test]
    async fn track_projects_handle_output_and_terminal_state_then_disposes_subscriptions() {
        let (_persistence, service, effects) = service_with_effects();
        let handle = Arc::new(TestTrackedHandle::new("app-task-1"));
        let entry = service
            .track(
                handle.clone() as Arc<dyn AgentTrackedTaskHandle>,
                AgentTaskTrackOptions {
                    id_prefix: Some("job".into()),
                    description: "tracked job".into(),
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
            )
            .unwrap();
        assert!(entry.task_id.starts_with("job-"));
        assert_eq!(
            entry.on_did_detach.await,
            ForegroundTaskReleaseReason::Terminal
        );
        handle.emit_output("tracked output");
        handle.transition(TaskState::Completed);
        let completed = service
            .wait(&entry.task_id, Some(30_000), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.base.status, AgentTaskStatus::Completed);
        assert_eq!(
            service
                .get_output_snapshot(&entry.task_id, 100)
                .await
                .unwrap()
                .preview,
            "tracked output"
        );
        handle.emit_output("ignored after settlement");
        assert_eq!(
            service
                .get_output_snapshot(&entry.task_id, 100)
                .await
                .unwrap()
                .preview,
            "tracked output"
        );
        assert_eq!(
            effects.started.lock().as_slice(),
            [entry.task_id.as_str()]
        );
        assert_eq!(
            effects.terminated.lock().as_slice(),
            [entry.task_id.as_str()]
        );
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
            .get_output_snapshot("bash-output01", 9)
            .await
            .unwrap();
        assert!(!buffered.full_output_available);
        assert_eq!(buffered.preview, "€ world");

        service.persist_output("bash-output01");
        let persisted = service
            .get_output_snapshot("bash-output01", 5)
            .await
            .unwrap();
        assert!(persisted.full_output_available);
        assert_eq!(persisted.preview, "world");
        assert!(persisted.output_path.unwrap().ends_with("output.log"));
        assert_eq!(
            service.read_output("bash-output01", Some(5)).await.unwrap(),
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
            *effects.terminated.lock(),
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
        assert_eq!(effects.terminated.lock().len(), 1);
    }

    #[tokio::test]
    async fn stop_normalizes_reason_aborts_and_settles_after_grace() {
        let (_persistence, service, effects) = service_with_effects();
        insert_live(&service, "bash-stop0001", true);
        let signal = service
            .state()
            .tasks
            .get("bash-stop0001")
            .unwrap()
            .state
            .abort_controller
            .signal();
        let stopped = service
            .stop("bash-stop0001", Some("  requested  "))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stopped.base.status, AgentTaskStatus::Killed);
        assert_eq!(stopped.base.stop_reason.as_deref(), Some("requested"));
        assert!(signal.aborted());
        assert_eq!(
            *effects.terminated.lock(),
            ["bash-stop0001".to_owned()]
        );
        assert_eq!(
            service
                .stop("bash-stop0001", Some("ignored"))
                .await
                .unwrap()
                .unwrap()
                .base
                .stop_reason
                .as_deref(),
            Some("requested")
        );
        assert!(service.stop("missing", None).await.unwrap().is_none());

        insert_live(&service, "bash-stop0002", true);
        let by_user = service
            .stop_by_user("bash-stop0002")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            by_user.base.stop_reason,
            Some(user_cancellation_reason().to_string())
        );
    }

    #[tokio::test]
    async fn stop_all_and_exit_policy_preserve_order_suppression_and_keep_alive() {
        let (_persistence, service, effects) = service_with_effects();
        insert_live(&service, "bash-all00001", true);
        insert_live(&service, "bash-all00002", true);
        let stopped = service.stop_all(Some("shutdown")).await.unwrap();
        assert_eq!(
            stopped
                .iter()
                .map(|info| info.base.task_id.as_str())
                .collect::<Vec<_>>(),
            ["bash-all00001", "bash-all00002"]
        );
        assert!(
            stopped
                .iter()
                .all(|info| info.base.status == AgentTaskStatus::Killed)
        );

        insert_live(&service, "bash-exit0001", true);
        let exited = service.stop_all_on_exit("exit").await.unwrap();
        let exited_info = exited
            .iter()
            .find(|info| info.base.task_id == "bash-exit0001")
            .unwrap();
        assert_eq!(
            exited_info.base.terminal_notification_suppressed,
            Some(true)
        );

        insert_live(&service, "bash-keep0001", true);
        *effects.keep_alive_on_exit.lock() = Some(true);
        assert!(
            service
                .stop_all_on_exit("ignored")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            service.get_task("bash-keep0001").unwrap().base.status,
            AgentTaskStatus::Running
        );
    }

    #[tokio::test]
    async fn dispose_aborts_active_tasks_without_synthesizing_settlement() {
        let (_persistence, service) = service();
        insert_live(&service, "bash-dispose1", true);
        let signal = service
            .state()
            .tasks
            .get("bash-dispose1")
            .unwrap()
            .state
            .abort_controller
            .signal();
        service.dispose().unwrap();
        tokio::task::yield_now().await;
        assert!(signal.aborted());
        assert_eq!(signal.reason().unwrap().to_string(), SESSION_CLOSED_REASON);
        assert_eq!(
            service.get_task("bash-dispose1").unwrap().base.status,
            AgentTaskStatus::Running
        );
    }

    #[tokio::test]
    async fn service_contract_delegates_to_the_concrete_task_service() {
        let (_persistence, service) = service();
        insert_live(&service, "bash-contract1", true);
        let handle = crate::agent::task::AgentTaskServiceHandle(Arc::new(service));

        assert_eq!(handle.list(Some(true), None).len(), 1);
        let stopped = handle
            .stop("bash-contract1", Some("contract"))
            .await
            .unwrap();
        assert_eq!(stopped.unwrap().base.status, AgentTaskStatus::Killed);
        assert_eq!(handle.list(Some(true), None).len(), 0);
    }

    #[test]
    fn persistence_coordinates_preserve_main_fallback_and_agent_scope() {
        let session = crate::session::session_context::make_session_context(
            crate::session::session_context::SessionContextInput {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
                session_dir: "/sessions/workspace/session".into(),
                session_scope: "sessions/workspace/session".into(),
                cwd: "/workspace".into(),
                meta_scope: None,
            },
        );
        let main = crate::agent::scope_context::make_agent_scope_context(
            crate::agent::scope_context::AgentScopeContextInput {
                agent_id: "main".into(),
                agent_scope: "sessions/workspace/session/agents/main".into(),
            },
        );
        let (dir, scope, fallback) = task_persistence_coordinates(&session, &main);
        assert_eq!(
            dir,
            std::path::Path::new("/sessions/workspace/session/agents/main")
        );
        assert_eq!(scope, "sessions/workspace/session/agents/main");
        assert_eq!(
            fallback,
            Some(AgentTaskPersistenceRoot {
                dir: "/sessions/workspace/session".into(),
                scope: "sessions/workspace/session".into(),
            })
        );

        let child = crate::agent::scope_context::make_agent_scope_context(
            crate::agent::scope_context::AgentScopeContextInput {
                agent_id: "worker".into(),
                agent_scope: "sessions/workspace/session/agents/worker".into(),
            },
        );
        assert!(task_persistence_coordinates(&session, &child).2.is_none());
    }

    #[tokio::test]
    async fn restore_after_replay_merges_wire_and_disk_then_reconciles_in_order() {
        let (persistence, service, effects) = service_with_effects();
        let mut disk_conflict = task("bash-restore1", AgentTaskStatus::Completed, true);
        disk_conflict.base.description = "disk".into();
        persistence.write_task(&disk_conflict).await.unwrap();
        persistence
            .write_task(&task("bash-restore2", AgentTaskStatus::Running, true))
            .await
            .unwrap();
        let mut wire_task = task("bash-restore1", AgentTaskStatus::Running, true);
        wire_task.base.description = "wire".into();
        let delivered = vec!["task\0completed\0notice".into()];

        let lost = service
            .restore_after_replay(
                &TaskModelState::from([(wire_task.base.task_id.clone(), wire_task)]),
                &delivered,
                50,
            )
            .await
            .unwrap();

        assert_eq!(
            lost.iter()
                .map(|info| info.base.task_id.as_str())
                .collect::<Vec<_>>(),
            ["bash-restore2"]
        );
        assert_eq!(
            service.get_task("bash-restore1").unwrap().base.description,
            "disk"
        );
        assert!(lost.iter().all(|info| {
            info.base.status == AgentTaskStatus::Lost && info.base.ended_at == Some(50)
        }));
        assert!(service.state().notifications.is_delivered(&delivered[0]));
        assert_eq!(
            effects.terminated.lock().as_slice(),
            ["bash-restore2"]
        );
    }

    #[tokio::test]
    async fn restore_hook_reconciles_wire_models_before_next_and_is_disposable() {
        let (_persistence, service, effects) = service_with_effects();
        let wire = WireServiceHandle(Arc::new(WireService::new(
            "agents/main",
            AppendLogStoreHandle(Arc::new(EmptyAppendLog)),
            Arc::new(IdentityBlobs),
            Arc::new(NoopDomainEvents),
        )));
        service.install_restore_hook(&wire).unwrap();
        wire.dispatch([
            task_started(task("bash-wire0001", AgentTaskStatus::Running, true)).unwrap(),
        ])
        .unwrap();

        wire.hooks()
            .on_did_restore
            .run(&mut (), None)
            .await
            .unwrap();
        assert_eq!(
            service.get_task("bash-wire0001").unwrap().base.status,
            AgentTaskStatus::Lost
        );
        assert_eq!(
            effects.terminated.lock().as_slice(),
            ["bash-wire0001"]
        );

        service.dispose().unwrap();
        wire.dispatch([
            task_started(task("bash-wire0002", AgentTaskStatus::Running, true)).unwrap(),
        ])
        .unwrap();
        wire.hooks()
            .on_did_restore
            .run(&mut (), None)
            .await
            .unwrap();
        assert!(service.get_task("bash-wire0002").is_none());
    }

    #[test]
    fn context_splice_arms_and_consumes_active_task_reminder_and_delivery() {
        let (_persistence, service) = service();
        insert_live(&service, "bash-remind01", true);
        let task_origin = PromptOrigin::Task {
            task_id: "bash-finished1".into(),
            status: AgentTaskStatus::Completed,
            notification_id: "task:bash-finished1:completed".into(),
        };
        service.handle_context_splice(
            3,
            &[
                context_message(PromptOrigin::CompactionSummary),
                context_message(task_origin),
            ],
        );

        let key = "bash-finished1\0completed\0task:bash-finished1:completed";
        assert!(service.state().notifications.is_delivered(key));
        let reminder = service.active_background_task_reminder().unwrap();
        assert!(reminder.contains("task_id: bash-remind01"));
        assert_eq!(service.active_background_task_reminder(), None);

        service.handle_context_splice(2, &[context_message(PromptOrigin::CompactionSummary)]);
        service.state().tasks.clear();
        assert_eq!(service.active_background_task_reminder(), None);
        insert_live(&service, "bash-later001", true);
        assert_eq!(service.active_background_task_reminder(), None);
    }

    #[tokio::test]
    async fn context_hooks_route_splices_to_the_injector_and_dispose_together() {
        let (_persistence, service) = service();
        insert_live(&service, "bash-hook0001", true);
        let bus = Arc::new(EventBusService::new());
        let bus_handle = EventBusHandle(bus.clone());
        let injector = TestContextInjector::default();
        service.install_context_hooks(&bus_handle, &injector);
        assert_eq!(
            injector.name.lock().as_deref(),
            Some(ACTIVE_BACKGROUND_TASK_INJECTION_VARIANT)
        );

        EventBusContract::publish(
            &*bus,
            DomainEvent::new(
                "context.spliced",
                Map::from_iter([
                    ("deleteCount".into(), Value::from(1)),
                    (
                        "messages".into(),
                        serde_json::to_value([context_message(PromptOrigin::CompactionSummary)])
                            .unwrap(),
                    ),
                ]),
            ),
        );
        let provider = injector.provider.lock().clone().unwrap();
        let injected = provider(ContextInjectionContext {
            injected_positions: vec![],
            last_injected_at: None,
            is_new_turn: true,
        })
        .await
        .unwrap();
        let Some(ContextInjectionContent::Text(reminder)) = injected else {
            panic!("expected text reminder")
        };
        assert!(reminder.contains("task_id: bash-hook0001"));

        service.dispose().unwrap();
        assert!(injector.provider.lock().is_none());
    }

    #[tokio::test]
    async fn detach_runs_callback_persists_pending_output_and_records_started_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (_persistence, service, effects) = service_with_effects();
        let detach_calls = Arc::new(AtomicUsize::new(0));
        let detach_calls_for_callback = Arc::clone(&detach_calls);
        service.insert_managed_task(ManagedTaskState::tracked(
            "task-detach01".into(),
            AgentTaskTrackOptions {
                id_prefix: None,
                description: "tracked".into(),
                detached: Some(false),
                timeout_ms: None,
                detach_timeout_ms: Some(0),
                signal: None,
                force_stop: None,
                on_detach: Some(Arc::new(move || {
                    detach_calls_for_callback.fetch_add(1, Ordering::SeqCst);
                })),
                to_info: Arc::new(|base| AgentTaskInfo {
                    base,
                    kind: "process".into(),
                    details: Map::new(),
                }),
            },
            10,
        ));
        let release = service
            .state()
            .tasks
            .get("task-detach01")
            .unwrap()
            .state
            .foreground_release_future();
        service
            .state()
            .tasks
            .get_mut("task-detach01")
            .unwrap()
            .state
            .output
            .append("pending output".into(), false);

        let detached = service.detach("task-detach01").unwrap();
        assert_eq!(detached.base.detached, Some(true));
        assert_eq!(detached.base.timeout_ms, Some(0));
        assert_eq!(release.await, ForegroundTaskReleaseReason::Detached);
        assert_eq!(detach_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *effects.started.lock(),
            ["task-detach01".to_owned()]
        );
        assert_eq!(
            service
                .get_output_snapshot("task-detach01", 100)
                .await
                .unwrap()
                .preview,
            "pending output"
        );
        service.detach("task-detach01");
        assert_eq!(detach_calls.load(Ordering::SeqCst), 1);
        assert_eq!(effects.started.lock().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn manager_timeout_auto_backgrounds_eligible_foreground_task() {
        let (_persistence, service) = service();
        service.insert_managed_task(ManagedTaskState::registered(
            "bash-timeout1".into(),
            Arc::new(StubTask),
            RegisterAgentTaskOptions {
                detached: Some(false),
                auto_background_on_timeout: Some(true),
                ..RegisterAgentTaskOptions::default()
            },
            10,
        ));
        let release = service
            .state()
            .tasks
            .get("bash-timeout1")
            .unwrap()
            .state
            .foreground_release_future();
        service.arm_manager_timeout("bash-timeout1", 25);
        tokio::time::advance(Duration::from_millis(25)).await;
        tokio::task::yield_now().await;
        assert_eq!(release.await, ForegroundTaskReleaseReason::TimeoutDetached);
        assert_eq!(
            service.get_task("bash-timeout1").unwrap().base.detached,
            Some(true)
        );
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
                .wait("bash-wait0001", Some(0), None)
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
                .wait("bash-wait0001", Some(30_000), Some(controller.signal()))
                .await
                .is_err()
        );

        let waiting_service = service.clone();
        let waiter = tokio::spawn(async move {
            waiting_service
                .wait("bash-wait0001", Some(30_000), None)
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
            *effects.terminated.lock(),
            ["bash-running2".to_owned()]
        );
        assert_eq!(
            *effects.restored.lock(),
            ["bash-done0002".to_owned(), "bash-running2".to_owned()]
        );
        assert!(effects.enqueued.lock().is_empty());
    }

    #[tokio::test]
    async fn live_notification_is_enqueued_instead_of_restored() {
        let (_persistence, service, effects) = service_with_effects();
        let completed = task("bash-notify04", AgentTaskStatus::Completed, true);
        service.notify_agent_task(&completed).await.unwrap();
        assert_eq!(
            *effects.enqueued.lock(),
            ["bash-notify04".to_owned()]
        );
        assert!(effects.restored.lock().is_empty());
    }

    #[test]
    fn read_output_tail_matches_javascript_utf16_and_zero_semantics() {
        assert_eq!(utf16_tail("a😀b", 2), "�b");
        assert_eq!(utf16_tail("a😀b", 3), "😀b");
        assert_eq!(utf16_tail("abc", 0), "abc");
        assert_eq!(javascript_timeout_duration(0), Duration::from_millis(1));
        assert_eq!(javascript_timeout_duration(1), Duration::from_millis(1));
        assert_eq!(
            javascript_timeout_duration(30_000),
            Duration::from_millis(30_000)
        );
    }
}
