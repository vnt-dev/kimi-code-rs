//! Session-scoped cron scheduler.
//!
//! Original: `packages/agent-core-v2/src/session/cron/sessionCronServiceImpl.ts`.

use parking_lot::Mutex;
use std::sync::{
    Arc, OnceLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared, join_all},
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use ulid::Ulid;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::timer::{IntervalTimer, IntervalTimerOptions},
    },
    agent::{
        context_memory::{ContextMessage, PromptOrigin},
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopState, TurnHandle},
        prompt::AGENT_PROMPT_SERVICE_ID,
        tool_registry::{AGENT_TOOL_REGISTRY_SERVICE_ID, ToolRegistrationOptions},
    },
    app::{
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        cron::{
            CRON_ID_REGEX, CRON_SESSION_TAG, CRON_TASK_PERSISTENCE_SERVICE_ID, ClockSources,
            CronTask, CronTaskInit, CronTaskPersistenceHandle, CronTaskQuery,
            DEFAULT_CRON_JITTER_CONFIG, ParsedCronExpression, compute_next_cron_run,
            jittered_next_cron_run_ms, one_shot_jittered_next_cron_run_ms, parse_cron_expression,
            render_cron_fire_xml, resolve_clock_sources,
        },
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID},
        telemetry::{
            CronDeletedEvent, CronFiredEvent, CronMissedEvent, CronScheduledEvent,
            TELEMETRY_SERVICE_ID, TelemetryServiceEventExt, TelemetryServiceHandle,
        },
    },
    kosong::contract::message::{ContentPart, Message, Role},
    session::{
        agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceHandle, MAIN_AGENT_ID},
        cron::{
            CRON_MODEL, CronLoadOptions, MissedCronRenderer, SESSION_CRON_SERVICE_ID,
            SessionCronResult, SessionCronServiceContract, SessionCronServiceHandle, cron_add,
            cron_cursor, cron_delete,
            tools::{CronCreateTool, CronDeleteTool, CronListTool},
        },
        session_context::{SESSION_CONTEXT_ID, SessionContext},
    },
    tool::{ErasedExecutableTool, ToolSource},
    wire::contract::WIRE_SERVICE_ID,
};

pub const CRON_SCHEDULED: &str = "cron_scheduled";
pub const CRON_FIRED: &str = "cron_fired";
pub const CRON_MISSED: &str = "cron_missed";
pub const CRON_DELETED: &str = "cron_deleted";

const STALE_THRESHOLD_MS: f64 = 7.0 * 24.0 * 60.0 * 60.0 * 1_000.0;
const DEFAULT_POLL_INTERVAL_MS: u64 = 1_000;
const MAX_COALESCE_ITERATIONS: u64 = 10_000;
const MAX_ID_ATTEMPTS: usize = 8;

type PersistFuture = Shared<BoxFuture<'static, ()>>;
type PersistQueue = (u64, PersistFuture);

#[derive(Clone, Debug, Default)]
struct CronRuntimeConfig {
    debug: bool,
    no_jitter: bool,
    no_stale: bool,
    disabled: bool,
    manual_tick: bool,
    clock: Option<String>,
    poll_interval_ms: Option<Option<u64>>,
}

impl CronRuntimeConfig {
    fn from_value(value: Option<Value>) -> Self {
        let Some(Value::Object(value)) = value else {
            return Self::default();
        };
        let boolean = |name: &str| value.get(name).and_then(Value::as_bool).unwrap_or(false);
        Self {
            debug: boolean("debug"),
            no_jitter: boolean("noJitter"),
            no_stale: boolean("noStale"),
            disabled: boolean("disabled"),
            manual_tick: boolean("manualTick"),
            clock: value
                .get("clock")
                .and_then(Value::as_str)
                .map(str::to_owned),
            poll_interval_ms: match value.get("pollIntervalMs") {
                None => None,
                Some(Value::Null) => Some(None),
                Some(value) => value.as_u64().map(Some),
            },
        }
    }
}

struct RuntimeState {
    tasks: IndexMap<String, CronTask>,
    parsed_cache: HashMap<String, ParsedCronExpression>,
    last_seen_at: HashMap<String, f64>,
    seeded_from_store: HashSet<String>,
    in_flight: HashSet<String>,
    clocks: Arc<dyn ClockSources>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            tasks: IndexMap::new(),
            parsed_cache: HashMap::new(),
            last_seen_at: HashMap::new(),
            seeded_from_store: HashSet::new(),
            in_flight: HashSet::new(),
            clocks: resolve_clock_sources(None, false),
        }
    }
}

pub struct SessionCronService {
    ctx: SessionContext,
    store: CronTaskPersistenceHandle,
    agent_lifecycle: AgentLifecycleServiceHandle,
    telemetry: TelemetryServiceHandle,
    config: ConfigServiceHandle,
    runtime: Arc<Mutex<RuntimeState>>,
    timer: Mutex<IntervalTimer>,
    persist_queues: Arc<Mutex<HashMap<String, PersistQueue>>>,
    persist_generation: AtomicU64,
    started: AtomicBool,
    disposables: DisposableStore,
    self_weak: OnceLock<Weak<Self>>,
    #[cfg(unix)]
    sigusr1: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SessionCronService {
    pub fn new(
        ctx: SessionContext,
        store: CronTaskPersistenceHandle,
        agent_lifecycle: AgentLifecycleServiceHandle,
        telemetry: TelemetryServiceHandle,
        config: ConfigServiceHandle,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            ctx,
            store,
            agent_lifecycle,
            telemetry,
            config,
            runtime: Arc::new(Mutex::new(RuntimeState::default())),
            timer: Mutex::new(IntervalTimer::new(IntervalTimerOptions { unref: true })),
            persist_queues: Arc::new(Mutex::new(HashMap::new())),
            persist_generation: AtomicU64::new(1),
            started: AtomicBool::new(false),
            disposables: DisposableStore::new(),
            self_weak: OnceLock::new(),
            #[cfg(unix)]
            sigusr1: Mutex::new(None),
        });
        service
            .self_weak
            .set(Arc::downgrade(&service))
            .expect("cron service weak self is initialized once");
        service.initialize();
        service
    }

    fn initialize(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.disposables.add(
            self.agent_lifecycle
                .on_did_create()
                .subscribe(move |handle| {
                    if handle.id() == MAIN_AGENT_ID
                        && let Some(service) = weak.upgrade()
                    {
                        service.bind_main_agent(handle);
                    }
                }),
        );
        if let Some(main) = self.agent_lifecycle.get(MAIN_AGENT_ID) {
            self.bind_main_agent(&main);
        }
    }

    fn bind_main_agent(self: &Arc<Self>, handle: &crate::_base::di::scope::ScopeHandle) {
        if let Ok(wire) = handle.get(WIRE_SERVICE_ID) {
            let weak = Arc::downgrade(self);
            let hook = wire.hooks().on_did_restore.register(
                "cron",
                Arc::new(move |ctx, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.config.ready().await.map_err(|error| {
                                Box::new(error)
                                    as crate::_base::lifecycle::lifecycle_machine::BoxError
                            })?;
                            service.resolve_clocks();
                            if let Some(main) = service.agent_lifecycle.get(MAIN_AGENT_ID)
                                && let Ok(wire) = main.get(WIRE_SERVICE_ID)
                            {
                                service.runtime.lock().tasks = wire.get_model(&CRON_MODEL);
                            }
                            service
                                .load_from_store(CronLoadOptions {
                                    replace: Some(false),
                                })
                                .await?;
                            service.start().await?;
                        }
                        next(ctx).await
                    })
                }),
                Default::default(),
            );
            if let Ok(hook) = hook {
                self.disposables.add(hook);
            }
        }
        self.register_cron_tools(handle);
    }

    fn register_cron_tools(&self, handle: &crate::_base::di::scope::ScopeHandle) {
        let Ok(registry) = handle.get(AGENT_TOOL_REGISTRY_SERVICE_ID) else {
            return;
        };
        let Some(contract) = self.self_handle() else {
            return;
        };
        let options = ToolRegistrationOptions {
            source: Some(ToolSource::Builtin),
        };
        let tools: Vec<Arc<dyn ErasedExecutableTool>> = vec![
            Arc::new(CronCreateTool::new(contract.clone(), handle.id())),
            Arc::new(CronListTool::new(contract.clone())),
            Arc::new(CronDeleteTool::new(contract, handle.id())),
        ];
        for tool in tools {
            self.disposables.add(registry.register(tool, options));
        }
    }

    fn self_handle(&self) -> Option<SessionCronServiceHandle> {
        self.self_weak
            .get()
            .and_then(Weak::upgrade)
            .map(|service| SessionCronServiceHandle(service))
    }

    fn cron_config(&self) -> CronRuntimeConfig {
        CronRuntimeConfig::from_value(self.config.get("cron"))
    }

    fn resolve_clocks(&self) {
        let config = self.cron_config();
        self.runtime.lock().clocks = resolve_clock_sources(config.clock.as_deref(), config.debug);
    }

    fn compute_jittered_next(
        &self,
        task: &CronTask,
        parsed: &ParsedCronExpression,
        base_ms: f64,
    ) -> Option<f64> {
        let ideal = compute_next_cron_run(parsed, base_ms)?;
        self.compute_display_next_fire(task, parsed, ideal)
    }

    fn count_coalesced(
        &self,
        task: &CronTask,
        parsed: &ParsedCronExpression,
        first_fire_ms: f64,
        now_ms: f64,
    ) -> (u64, f64) {
        let mut count = 1;
        let mut cursor = first_fire_ms;
        let mut last_due = first_fire_ms;
        while count < MAX_COALESCE_ITERATIONS {
            let Some(next) = compute_next_cron_run(parsed, cursor) else {
                break;
            };
            if next > now_ms {
                break;
            }
            let Some(jittered) = self.compute_display_next_fire(task, parsed, next) else {
                break;
            };
            if jittered > now_ms {
                break;
            }
            count += 1;
            cursor = next;
            last_due = next;
        }
        (count, last_due)
    }

    async fn process_due(&self, task: CronTask, now: f64) {
        let parsed = {
            let mut runtime = self.runtime.lock();
            if let Some(parsed) = runtime.parsed_cache.get(&task.cron) {
                parsed.clone()
            } else {
                match parse_cron_expression(&task.cron) {
                    Ok(parsed) => {
                        runtime
                            .parsed_cache
                            .insert(task.cron.clone(), parsed.clone());
                        parsed
                    }
                    Err(error) => {
                        self.debug_log(&format!(
                            "tick failed to parse cron for task {}: {error}",
                            task.id
                        ));
                        return;
                    }
                }
            }
        };
        {
            let mut runtime = self.runtime.lock();
            if runtime.in_flight.contains(&task.id) {
                return;
            }
            if !runtime.seeded_from_store.contains(&task.id) {
                if task
                    .last_fired_at
                    .is_some_and(|cursor| cursor.is_finite() && cursor <= now)
                    && !runtime.last_seen_at.contains_key(&task.id)
                {
                    runtime
                        .last_seen_at
                        .insert(task.id.clone(), task.last_fired_at.unwrap());
                }
                runtime.seeded_from_store.insert(task.id.clone());
            }
        }
        let base = {
            let runtime = self.runtime.lock();
            runtime
                .last_seen_at
                .get(&task.id)
                .copied()
                .filter(|seen| *seen > task.created_at)
                .unwrap_or(task.created_at)
        };
        let Some(next_fire) = self.compute_jittered_next(&task, &parsed, base) else {
            return;
        };
        if now < next_fire {
            return;
        }
        let ideal = compute_next_cron_run(&parsed, base);
        let (coalesced, last_due) = if task.recurring != Some(false) {
            ideal.map_or((1, None), |ideal| {
                let (count, last_due) = self.count_coalesced(&task, &parsed, ideal, now);
                (count.max(1), Some(last_due))
            })
        } else {
            (1, None)
        };
        self.runtime.lock().in_flight.insert(task.id.clone());
        let delivered = self.deliver_due(&task, coalesced).await;
        self.runtime.lock().in_flight.remove(&task.id);
        if !delivered {
            return;
        }
        if task.recurring == Some(false) {
            let _ = self.remove_tasks(std::slice::from_ref(&task.id));
            let mut runtime = self.runtime.lock();
            runtime.last_seen_at.remove(&task.id);
            runtime.seeded_from_store.remove(&task.id);
        } else {
            let advanced = last_due.unwrap_or(now);
            self.runtime
                .lock()
                .last_seen_at
                .insert(task.id.clone(), advanced);
            self.advance_cursor(&task.id, advanced);
        }
    }

    async fn deliver_due(&self, task: &CronTask, coalesced_count: u64) -> bool {
        let fired_at = self.now();
        let stale = self.is_stale_at(task, fired_at);
        let delivered = self
            .deliver_fire(task, coalesced_count, fired_at, stale)
            .await;
        if delivered
            && stale
            && task.recurring != Some(false)
            && self
                .remove_tasks(std::slice::from_ref(&task.id))
                .is_ok_and(|removed| !removed.is_empty())
        {
            self.emit_deleted(&task.id, None);
        }
        delivered
    }

    async fn deliver_fire(
        &self,
        task: &CronTask,
        coalesced_count: u64,
        fired_at: f64,
        stale: bool,
    ) -> bool {
        let Some(main) = self.agent_lifecycle.get(MAIN_AGENT_ID) else {
            return false;
        };
        let Ok(prompt) = main.get(AGENT_PROMPT_SERVICE_ID) else {
            return false;
        };
        let buffered = main
            .get(AGENT_LOOP_SERVICE_ID)
            .is_ok_and(|service| service.status().state == AgentLoopState::Running);
        let origin = PromptOrigin::CronJob {
            job_id: task.id.clone(),
            cron: task.cron.clone(),
            recurring: task.recurring != Some(false),
            coalesced_count,
            stale: self.is_stale_at(task, fired_at),
        };
        let message = context_message(
            vec![ContentPart::Text {
                text: render_cron_fire_xml(&origin, &task.prompt),
            }],
            origin.clone(),
        );
        if let Err(error) = prompt.inject(message).await {
            self.debug_log(&format!(
                "steer launch rejected for task {}: {error}",
                task.id
            ));
            return false;
        }
        if let Ok(bus) = main.get(EVENT_BUS_SERVICE_ID)
            && let Value::Object(fields) = json!({
                "origin": origin,
                "prompt": task.prompt,
            })
        {
            bus.publish(DomainEvent::new("cron.fired", fields));
        }
        let _ = self.telemetry.track_event(&CronFiredEvent {
            recurring: task.recurring != Some(false),
            coalesced_count,
            stale,
            buffered,
        });
        true
    }

    fn advance_cursor(&self, id: &str, last_fired_at: f64) {
        let updated = {
            let mut runtime = self.runtime.lock();
            let Some(task) = runtime.tasks.get_mut(id) else {
                return;
            };
            task.last_fired_at = Some(last_fired_at);
            task.clone()
        };
        if let Ok(op) = cron_cursor(id, last_fired_at) {
            self.dispatch_cron(op);
        }
        self.enqueue_save(updated);
    }

    fn dispatch_cron(&self, op: crate::wire::op::Op) {
        if let Some(main) = self.agent_lifecycle.get(MAIN_AGENT_ID)
            && let Ok(wire) = main.get(WIRE_SERVICE_ID)
        {
            let _ = wire.dispatch([op]);
        }
    }

    fn enqueue_save(&self, task: CronTask) {
        let store = self.store.clone();
        let workspace_id = self.ctx.workspace_id.clone();
        let id = task.id.clone();
        self.persist_enqueue(id, async move {
            let _ = store.save(&workspace_id, &task).await;
        });
    }

    fn enqueue_delete(&self, id: String) {
        let store = self.store.clone();
        let workspace_id = self.ctx.workspace_id.clone();
        let queue_id = id.clone();
        self.persist_enqueue(queue_id, async move {
            let _ = store.delete(&workspace_id, &id).await;
        });
    }

    fn persist_enqueue(&self, id: String, work: impl Future<Output = ()> + Send + 'static) {
        let previous = self
            .persist_queues
            .lock()
            .get(&id)
            .map(|(_, future)| future.clone());
        let future = async move {
            if let Some(previous) = previous {
                previous.await;
            }
            work.await;
        }
        .boxed()
        .shared();
        let generation = self.persist_generation.fetch_add(1, Ordering::Relaxed);
        self.persist_queues
            .lock()
            .insert(id.clone(), (generation, future.clone()));
        let queues = Arc::clone(&self.persist_queues);
        tokio::spawn(async move {
            future.await;
            let mut queues = queues.lock();
            if queues
                .get(&id)
                .is_some_and(|(current_generation, _)| *current_generation == generation)
            {
                queues.remove(&id);
            }
        });
    }

    fn next_fire_for(&self, task: &CronTask) -> Option<f64> {
        let parsed = {
            let mut runtime = self.runtime.lock();
            if let Some(parsed) = runtime.parsed_cache.get(&task.cron) {
                parsed.clone()
            } else {
                match parse_cron_expression(&task.cron) {
                    Ok(parsed) => {
                        runtime
                            .parsed_cache
                            .insert(task.cron.clone(), parsed.clone());
                        parsed
                    }
                    Err(error) => {
                        drop(runtime);
                        self.debug_log(&format!("nextFireFor skipping task {}: {error}", task.id));
                        return None;
                    }
                }
            }
        };
        let runtime = self.runtime.lock();
        let persisted = task
            .last_fired_at
            .filter(|value| value.is_finite() && *value <= runtime.clocks.wall_now_ms());
        let cursor = runtime
            .last_seen_at
            .get(&task.id)
            .copied()
            .or(persisted)
            .filter(|cursor| *cursor > task.created_at)
            .unwrap_or(task.created_at);
        drop(runtime);
        self.compute_jittered_next(task, &parsed, cursor)
    }

    fn is_stale_at(&self, task: &CronTask, now: f64) -> bool {
        if self.cron_config().no_stale || task.recurring == Some(false) {
            return false;
        }
        let age = now - task.created_at;
        age.is_finite() && age >= STALE_THRESHOLD_MS
    }

    fn debug_log(&self, message: &str) {
        if self.cron_config().debug {
            eprintln!("[cron/session] {message}");
        }
    }

    #[cfg(unix)]
    fn bind_sigusr1(self: &Arc<Self>) {
        if !self.cron_config().manual_tick || self.sigusr1.lock().is_some() {
            return;
        }
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            let Ok(mut signal) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())
            else {
                return;
            };
            while signal.recv().await.is_some() {
                if let Some(service) = weak.upgrade() {
                    if let Err(error) = service.tick().await {
                        service.debug_log(&format!("SIGUSR1 tick threw: {error}"));
                    }
                } else {
                    break;
                }
            }
        });
        *self.sigusr1.lock() = Some(task);
    }

    #[cfg(unix)]
    fn unbind_sigusr1(&self) {
        if let Some(task) = self.sigusr1.lock().take() {
            task.abort();
        }
    }
}

#[async_trait]
impl SessionCronServiceContract for SessionCronService {
    fn is_enabled(&self) -> bool {
        true
    }

    fn is_disabled(&self) -> bool {
        self.cron_config().disabled
    }

    fn add_task(&self, init: CronTaskInit) -> SessionCronResult<CronTask> {
        let id = (0..MAX_ID_ATTEMPTS)
            .map(|_| Ulid::new().to_string())
            .find(|id| {
                CRON_ID_REGEX.is_match(id)
                    && !self.runtime.lock().tasks.contains_key(id)
            })
            .ok_or_else(|| {
                format!(
                    "SessionCronService: failed to generate a unique ULID after {MAX_ID_ATTEMPTS} attempts"
                )
            })?;
        let mut tags = init.tags.unwrap_or_default();
        tags.insert(CRON_SESSION_TAG.into(), self.ctx.session_id.clone());
        let task = CronTask {
            id,
            cron: init.cron,
            prompt: init.prompt,
            created_at: self.now(),
            recurring: init.recurring,
            last_fired_at: init.last_fired_at,
            tags: Some(tags),
        };
        self.runtime
            .lock()
            .tasks
            .insert(task.id.clone(), task.clone());
        if let Ok(op) = cron_add(task.clone()) {
            self.dispatch_cron(op);
        }
        self.enqueue_save(task.clone());
        Ok(task)
    }

    fn remove_tasks(&self, ids: &[String]) -> SessionCronResult<Vec<String>> {
        let removed = {
            let mut runtime = self.runtime.lock();
            ids.iter()
                .filter(|id| runtime.tasks.shift_remove(*id).is_some())
                .cloned()
                .collect::<Vec<_>>()
        };
        if removed.is_empty() {
            return Ok(removed);
        }
        if let Ok(op) = cron_delete(removed.clone()) {
            self.dispatch_cron(op);
        }
        for id in &removed {
            self.enqueue_delete(id.clone());
        }
        Ok(removed)
    }

    fn get_task(&self, id: &str) -> Option<CronTask> {
        self.runtime.lock().tasks.get(id).cloned()
    }

    fn list(&self) -> Vec<CronTask> {
        self.runtime.lock().tasks.values().cloned().collect()
    }

    fn now(&self) -> f64 {
        self.runtime.lock().clocks.wall_now_ms()
    }

    fn is_stale(&self, task: &CronTask) -> bool {
        self.is_stale_at(task, self.now())
    }

    fn get_next_fire_time(&self) -> Option<f64> {
        self.list()
            .iter()
            .filter_map(|task| self.next_fire_for(task))
            .min_by(f64::total_cmp)
    }

    fn get_next_fire_for_task(&self, task_id: &str) -> Option<f64> {
        self.get_task(task_id)
            .as_ref()
            .and_then(|task| self.next_fire_for(task))
    }

    fn compute_display_next_fire(
        &self,
        task: &CronTask,
        parsed: &ParsedCronExpression,
        ideal_ms: f64,
    ) -> Option<f64> {
        let no_jitter = Some(self.cron_config().no_jitter);
        Some(if task.recurring == Some(false) {
            one_shot_jittered_next_cron_run_ms(
                &task.id,
                Some(task.created_at),
                ideal_ms,
                DEFAULT_CRON_JITTER_CONFIG,
                no_jitter,
            )
        } else {
            jittered_next_cron_run_ms(
                &task.id,
                parsed,
                ideal_ms,
                DEFAULT_CRON_JITTER_CONFIG,
                no_jitter,
            )
        })
    }

    async fn load_from_store(&self, options: CronLoadOptions) -> SessionCronResult<()> {
        if options.replace != Some(false) {
            self.runtime.lock().tasks.clear();
        }
        let tasks = self
            .store
            .list(CronTaskQuery {
                workspace_id: self.ctx.workspace_id.clone(),
            })
            .await?;
        for mut task in tasks {
            let owner = task
                .tags
                .as_ref()
                .and_then(|tags| tags.get(CRON_SESSION_TAG));
            if owner.is_some_and(|owner| owner != &self.ctx.session_id) {
                continue;
            }
            if owner.is_none() {
                task.tags
                    .get_or_insert_with(IndexMap::new)
                    .insert(CRON_SESSION_TAG.into(), self.ctx.session_id.clone());
                self.enqueue_save(task.clone());
            }
            self.runtime.lock().tasks.insert(task.id.clone(), task);
        }
        Ok(())
    }

    async fn start(&self) -> SessionCronResult<()> {
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.config.ready().await?;
        let config = self.cron_config();
        let interval = if config.manual_tick {
            None
        } else {
            config
                .poll_interval_ms
                .unwrap_or(Some(DEFAULT_POLL_INTERVAL_MS))
        };
        if let Some(interval) = interval.filter(|interval| *interval != 0) {
            let weak = self.self_weak.get().cloned();
            self.timer.lock().cancel_and_set(
                move || {
                    if let Some(service) = weak.as_ref().and_then(std::sync::Weak::upgrade) {
                        tokio::spawn(async move {
                            let _ = service.tick().await;
                        });
                    }
                },
                Duration::from_millis(interval),
            );
        }
        // SIGUSR1 is a unix concept; Windows needs no signal binding.
        #[cfg(unix)]
        if let Some(service) = self.self_weak.get().and_then(Weak::upgrade) {
            service.bind_sigusr1();
        }
        Ok(())
    }

    async fn stop(&self) -> SessionCronResult<()> {
        #[cfg(unix)]
        self.unbind_sigusr1();
        self.timer.lock().cancel();
        {
            let mut runtime = self.runtime.lock();
            runtime.in_flight.clear();
            runtime.last_seen_at.clear();
            runtime.seeded_from_store.clear();
            runtime.parsed_cache.clear();
        }
        self.flush_persist().await;
        self.started.store(false, Ordering::Release);
        Ok(())
    }

    async fn tick(&self) -> SessionCronResult<()> {
        self.config.ready().await?;
        if self.is_disabled() {
            return Ok(());
        }
        let tasks = self.list();
        if tasks.is_empty() {
            return Ok(());
        }
        let Some(main) = self.agent_lifecycle.get(MAIN_AGENT_ID) else {
            return Ok(());
        };
        let loop_service = main.get(AGENT_LOOP_SERVICE_ID)?;
        if loop_service.status().state == AgentLoopState::Running {
            return Ok(());
        }
        let now = self.now();
        join_all(tasks.into_iter().map(|task| self.process_due(task, now))).await;
        Ok(())
    }

    async fn flush_persist(&self) {
        let pending = self
            .persist_queues
            .lock()
            .values()
            .map(|(_, future)| future.clone())
            .collect::<Vec<_>>();
        join_all(pending).await;
    }

    fn handle_missed(
        &self,
        tasks: &[CronTask],
        render_missed_notification: MissedCronRenderer,
    ) -> Option<TurnHandle> {
        if tasks.is_empty() {
            return None;
        }
        let main = self.agent_lifecycle.get(MAIN_AGENT_ID)?;
        let prompt = main.get(AGENT_PROMPT_SERVICE_ID).ok()?;
        let message = context_message(
            render_missed_notification(tasks),
            PromptOrigin::CronMissed {
                count: tasks.len() as u64,
            },
        );
        tokio::spawn(async move {
            let _ = prompt.inject(message).await;
        });
        let _ = self.telemetry.track_event(&CronMissedEvent {
            count: tasks.len() as u64,
        });
        None
    }

    fn emit_scheduled(&self, task: &CronTask, agent_id: Option<&str>) {
        let _ = self.telemetry.track_event(&CronScheduledEvent {
            recurring: task.recurring != Some(false),
            agent_id: agent_id.map(str::to_owned),
        });
    }

    fn emit_deleted(&self, task_id: &str, agent_id: Option<&str>) {
        let _ = self.telemetry.track_event(&CronDeletedEvent {
            task_id: task_id.into(),
            agent_id: agent_id.map(str::to_owned),
        });
    }
}

impl Disposable for SessionCronService {
    fn dispose(&self) -> DisposeResult {
        #[cfg(unix)]
        self.unbind_sigusr1();
        self.timer.lock().cancel();
        {
            let mut runtime = self.runtime.lock();
            runtime.in_flight.clear();
            runtime.last_seen_at.clear();
            runtime.seeded_from_store.clear();
            runtime.parsed_cache.clear();
        }
        self.started.store(false, Ordering::Release);
        let pending = self
            .persist_queues
            .lock()
            .values()
            .map(|(_, future)| future.clone())
            .collect::<Vec<_>>();
        if !pending.is_empty()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                join_all(pending).await;
            });
        }
        self.disposables.dispose()
    }
}

fn context_message(content: Vec<ContentPart>, origin: PromptOrigin) -> ContextMessage {
    ContextMessage {
        message: Message::new(Role::User, content, vec![]),
        id: None,
        provider_message_id: None,
        origin: Some(origin),
        is_error: None,
        note: None,
        attachments: Vec::new(),
    }
}

pub fn register_session_cron_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_CRON_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = SessionCronService::new(
                (*accessor.get(SESSION_CONTEXT_ID)?).clone(),
                (*accessor.get(CRON_TASK_PERSISTENCE_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_LIFECYCLE_SERVICE_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
            );
            let contract: Arc<dyn SessionCronServiceContract> = service;
            Ok(SessionCronServiceHandle(contract))
        })
        .disposable(),
        InstantiationType::Eager,
        "cron",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_poll_interval_states_match_typescript() {
        let default = CronRuntimeConfig::from_value(None);
        assert!(!default.disabled);
        assert!(!default.manual_tick);
        assert_eq!(default.poll_interval_ms, None);

        let configured = CronRuntimeConfig::from_value(Some(json!({
            "debug": true,
            "noJitter": true,
            "noStale": true,
            "disabled": true,
            "manualTick": true,
            "clock": "file:/tmp/clock",
            "pollIntervalMs": null
        })));
        assert!(configured.debug);
        assert!(configured.no_jitter);
        assert!(configured.no_stale);
        assert!(configured.disabled);
        assert!(configured.manual_tick);
        assert_eq!(configured.clock.as_deref(), Some("file:/tmp/clock"));
        assert_eq!(configured.poll_interval_ms, Some(None));

        let numeric = CronRuntimeConfig::from_value(Some(json!({"pollIntervalMs": 250})));
        assert_eq!(numeric.poll_interval_ms, Some(Some(250)));
    }

    #[test]
    fn public_event_names_match_telemetry_contract() {
        assert_eq!(CRON_SCHEDULED, "cron_scheduled");
        assert_eq!(CRON_FIRED, "cron_fired");
        assert_eq!(CRON_MISSED, "cron_missed");
        assert_eq!(CRON_DELETED, "cron_deleted");
    }

    #[test]
    fn registration_is_eager_session_scoped_and_uses_cron_domain() {
        crate::_base::di::scope::clear_scoped_registry_for_tests();
        register_session_cron_service();
        let entries =
            crate::_base::di::scope::get_scoped_service_descriptors(LifecycleScope::Session);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == SESSION_CRON_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "cron"
        }));
        crate::_base::di::scope::clear_scoped_registry_for_tests();
    }
}
