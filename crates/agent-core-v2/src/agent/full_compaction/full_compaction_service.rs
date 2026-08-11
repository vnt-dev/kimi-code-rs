//! Agent-scoped full-context compaction orchestration.
//!
//! Original: `packages/agent-core-v2/src/agent/fullCompaction/fullCompactionService.ts`.

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use async_trait::async_trait;
use futures_util::{FutureExt, future::Shared};
use serde_json::{Map, Value};
use tokio::sync::oneshot;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::{INSTANTIATION_SERVICE_ID, ServicesAccessorExt},
            instantiation_service::InstantiationService,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::{
            errors::{Error2, Error2Options, ErrorCause},
            serialize::to_error_payload,
        },
        event::{Emitter, Event},
        lifecycle::lifecycle_machine::BoxError,
        log::{LOG_SERVICE_ID, LogServiceHandle},
        utils::{
            abort::{AbortController, AbortError, AbortSignal, is_abort_error},
            render_prompt::render_prompt,
            retry::{retry_backoff_delays, sleep_for_retry},
        },
    },
    agent::{
        context_injector::{AGENT_CONTEXT_INJECTOR_SERVICE_ID, AgentContextInjectorServiceHandle},
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceHandle,
            ContextCompactionInput, ContextMessage, build_compaction_summary_text,
            is_real_user_input,
        },
        context_size::{AGENT_CONTEXT_SIZE_SERVICE_ID, AgentContextSizeServiceHandle},
        llm_requester::{
            AGENT_LLM_REQUESTER_SERVICE_ID, AgentLlmRequestError, AgentLlmRequestFinish,
            AgentLlmRequestOverrides, AgentLlmRequestSource, AgentLlmRequesterServiceHandle,
        },
        loop_::{
            AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle, AgentLoopState, LoopErrorContext,
            LoopErrorHandler, LoopErrorHandlerRegistrationOptions, LoopValue, StepEnqueueOptions,
            StepRequestQueuePosition,
        },
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle, ProfileModelContext},
        tool_registry::{AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle},
        tool_select::{
            AGENT_TOOL_SELECT_SERVICE_ID, AgentToolSelectServiceHandle, strip_dynamic_tool_context,
        },
    },
    app::{
        auth::AUTH_LOGIN_REQUIRED,
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle},
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryProperties, TelemetryServiceHandle},
    },
    hooks::{HookRegisterOptions, OrderedHookSlot},
    kosong::{
        contract::{
            errors::{ChatProviderError, is_retryable_generate_error},
            message::{ContentPart, Message, Role, create_user_message},
            provider::{FinishReason, ThinkingEffort},
            tokens::{
                estimate_tokens, estimate_tokens_for_message, estimate_tokens_for_messages,
                estimate_tokens_for_tools,
            },
            tool::Tool,
            usage::{TokenUsage, input_total},
        },
        protocol::errors::{CONTEXT_OVERFLOW, PROVIDER_AUTH_ERROR},
    },
    session::todo::{SESSION_TODO_SERVICE_ID, SessionTodoServiceHandle, render_todo_list},
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    AGENT_FULL_COMPACTION_SERVICE_ID, AgentFullCompactionHooks, AgentFullCompactionServiceContract,
    AgentFullCompactionServiceHandle, COMPACTION_MODEL, COMPACTION_UNABLE, CompactionBeginData,
    CompactionPhase, CompactionResult, CompactionSource, CompactionStrategy,
    DEFAULT_COMPACTION_CONFIG, FullCompactionError, FullCompactionInput, FullCompactionTask,
    RuntimeCompactionStrategy, ensure_full_compaction_errors_registered,
    ensure_full_compaction_ops_registered, full_compaction_begin, full_compaction_cancel,
    full_compaction_complete,
};

pub const MAX_COMPACTION_RETRY_ATTEMPTS: usize = 5;
const DEFAULT_COMPACTION_MAX_COMPLETION_TOKENS: u64 = 128 * 1024;
const OVERFLOW_CONTEXT_SAFETY_RATIO: f64 = 0.85;
const OVERFLOW_STATUS_RECOVERY_RATIO: f64 = 0.5;
const MAX_COMPACTION_OVERFLOW_SHRINK_ATTEMPTS: usize = 3;
const COMPACTION_OVERFLOW_SHRINK_RATIOS: [f64; 3] = [0.7, 0.5, 0.35];
const COMPACTION_INSTRUCTION_TEMPLATE: &str = include_str!("compaction-instruction.md");

#[derive(Debug)]
struct CompactionTruncatedError;

impl fmt::Display for CompactionTruncatedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("Compaction response was truncated before producing a complete summary.")
    }
}

impl Error for CompactionTruncatedError {}

#[derive(Debug)]
struct SharedError(FullCompactionError);

impl fmt::Display for SharedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for SharedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Clone, Debug)]
struct CompactionAttemptResult {
    summary: String,
    usage: Option<TokenUsage>,
    trace_id: Option<String>,
}

struct ActiveCompaction {
    task: FullCompactionTask,
    origin_turn_id: Option<crate::agent::TurnId>,
    blocked_by_turn: AtomicBool,
    settlement: Mutex<Option<oneshot::Sender<Result<CompactionResult, FullCompactionError>>>>,
}

impl ActiveCompaction {
    fn settle(&self, result: Result<CompactionResult, FullCompactionError>) {
        if let Some(sender) = self.settlement.lock().unwrap().take() {
            let _ = sender.send(result);
        }
    }
}

#[derive(Default)]
struct CompactionState {
    compaction_count_in_turn: u64,
    compacting: Option<Arc<ActiveCompaction>>,
    observed_max_context_tokens_by_model: HashMap<String, u64>,
    last_compacted_token_count: Option<f64>,
    consecutive_overflow_compactions: u64,
    active_turn_id: Option<crate::agent::TurnId>,
}

pub struct AgentFullCompactionService {
    context: AgentContextMemoryServiceHandle,
    context_size: AgentContextSizeServiceHandle,
    llm_requester: AgentLlmRequesterServiceHandle,
    profile: AgentProfileServiceHandle,
    tool_registry: AgentToolRegistryServiceHandle,
    tool_select: AgentToolSelectServiceHandle,
    instantiation: Arc<InstantiationService>,
    todo: SessionTodoServiceHandle,
    telemetry: TelemetryServiceHandle,
    wire: WireServiceHandle,
    event_bus: EventBusHandle,
    _log: LogServiceHandle,
    loop_service: AgentLoopServiceHandle,
    hooks: AgentFullCompactionHooks,
    on_did_finish_compaction: Arc<Emitter<FullCompactionTask>>,
    context_injector: Mutex<Option<AgentContextInjectorServiceHandle>>,
    state: Arc<Mutex<CompactionState>>,
    begin_lock: Mutex<()>,
    self_weak: OnceLock<Weak<Self>>,
    disposables: DisposableStore,
    shutdown: CancellationToken,
    tasks: TaskTracker,
}

impl AgentFullCompactionService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: AgentContextMemoryServiceHandle,
        context_size: AgentContextSizeServiceHandle,
        llm_requester: AgentLlmRequesterServiceHandle,
        profile: AgentProfileServiceHandle,
        tool_registry: AgentToolRegistryServiceHandle,
        tool_select: AgentToolSelectServiceHandle,
        instantiation: Arc<InstantiationService>,
        todo: SessionTodoServiceHandle,
        telemetry: TelemetryServiceHandle,
        wire: WireServiceHandle,
        event_bus: EventBusHandle,
        log: LogServiceHandle,
        loop_service: AgentLoopServiceHandle,
    ) -> Result<Arc<Self>, FullCompactionError> {
        ensure_full_compaction_errors_registered();
        ensure_full_compaction_ops_registered();
        let service = Arc::new(Self {
            context,
            context_size,
            llm_requester,
            profile,
            tool_registry,
            tool_select,
            instantiation,
            todo,
            telemetry,
            wire,
            event_bus,
            _log: log,
            loop_service,
            hooks: AgentFullCompactionHooks {
                on_will_compact: OrderedHookSlot::new(),
            },
            on_did_finish_compaction: Arc::new(Emitter::new()),
            context_injector: Mutex::new(None),
            state: Arc::new(Mutex::new(CompactionState::default())),
            begin_lock: Mutex::new(()),
            self_weak: OnceLock::new(),
            disposables: DisposableStore::new(),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
        });
        let _ = service.self_weak.set(Arc::downgrade(&service));
        service.install()?;
        Ok(service)
    }

    fn install(self: &Arc<Self>) -> Result<(), FullCompactionError> {
        let weak = Arc::downgrade(self);
        self.disposables.add(
            self.wire
                .hooks()
                .on_did_restore
                .register(
                    "full-compaction",
                    Arc::new(move |context, next| {
                        let weak = Weak::clone(&weak);
                        Box::pin(async move {
                            if let Some(service) = weak.upgrade() {
                                service
                                    .normalize_after_replay()
                                    .map_err(|error| Box::new(SharedError(error)) as BoxError)?;
                            }
                            next(context).await
                        })
                    }),
                    HookRegisterOptions::default(),
                )
                .map_err(arc_error)?,
        );

        let weak = Arc::downgrade(self);
        self.disposables.add(self.event_bus.subscribe_type(
            "turn.started",
            Arc::new(move |_| {
                if let Some(service) = weak.upgrade() {
                    service.reset_for_turn();
                }
            }),
        ));
        let weak = Arc::downgrade(self);
        self.disposables.add(self.event_bus.subscribe_type(
            "turn.ended",
            Arc::new(move |_| {
                if let Some(service) = weak.upgrade() {
                    service.state.lock().unwrap().active_turn_id = None;
                }
            }),
        ));

        let weak = Arc::downgrade(self);
        self.disposables.add(
            self.loop_service
                .hooks()
                .on_will_begin_step
                .register(
                    "full-compaction",
                    Arc::new(move |context, next| {
                        let weak = Weak::clone(&weak);
                        Box::pin(async move {
                            if let Some(service) = weak.upgrade() {
                                service
                                    .before_step(context.signal.clone(), Some(context.turn_id))
                                    .await
                                    .map_err(|error| Box::new(SharedError(error)) as BoxError)?;
                            }
                            next(context).await
                        })
                    }),
                    HookRegisterOptions::default(),
                )
                .map_err(arc_error)?,
        );
        let weak = Arc::downgrade(self);
        self.disposables.add(
            self.loop_service
                .hooks()
                .on_did_finish_step
                .register(
                    "full-compaction",
                    Arc::new(move |context, next| {
                        let weak = Weak::clone(&weak);
                        Box::pin(async move {
                            if let Some(service) = weak.upgrade() {
                                service
                                    .after_step()
                                    .map_err(|error| Box::new(SharedError(error)) as BoxError)?;
                            }
                            next(context).await
                        })
                    }),
                    HookRegisterOptions::default(),
                )
                .map_err(arc_error)?,
        );

        let handler: Arc<dyn LoopErrorHandler> = Arc::new(CompactionLoopErrorHandler {
            service: Arc::downgrade(self),
        });
        self.disposables.add(
            self.loop_service
                .register_loop_error_handler(
                    handler,
                    LoopErrorHandlerRegistrationOptions::default(),
                )
                .map_err(loop_value_error)?,
        );
        Ok(())
    }

    fn strategy(&self) -> Result<RuntimeCompactionStrategy, FullCompactionError> {
        let model = self.resolve_model_context_with_effective_max()?;
        Ok(RuntimeCompactionStrategy::new(Arc::new(move || {
            model.clone()
        })))
    }

    fn get_effective_max_context_tokens(&self) -> Result<u64, FullCompactionError> {
        let data = self.profile.data().map_err(FullCompactionError::from)?;
        let configured = data.config.model_capabilities.max_context_tokens;
        let observed = data.config.model_alias.as_ref().and_then(|model_alias| {
            self.state
                .lock()
                .unwrap()
                .observed_max_context_tokens_by_model
                .get(model_alias)
                .copied()
        });
        Ok(match observed {
            None => configured,
            Some(observed) if configured == 0 => observed,
            Some(observed) => configured.min(observed),
        })
    }

    fn resolve_model_context_with_effective_max(
        &self,
    ) -> Result<ProfileModelContext, FullCompactionError> {
        let mut resolved = self
            .profile
            .resolve_model_context()
            .map_err(FullCompactionError::from)?;
        resolved.model_capabilities.max_context_tokens = self.get_effective_max_context_tokens()?;
        Ok(resolved)
    }

    fn estimate_current_request_tokens(&self) -> f64 {
        let context = self.context.get();
        self.estimate_request_tokens(context.iter().map(|message| &message.message))
    }

    fn estimate_request_tokens<'a>(&self, messages: impl IntoIterator<Item = &'a Message>) -> f64 {
        (estimate_tokens(&self.profile.get_system_prompt())
            + estimate_tokens_for_tools(
                &self
                    .default_tools()
                    .into_iter()
                    .filter(|tool| tool.deferred != Some(true))
                    .collect::<Vec<_>>(),
            )
            + estimate_tokens_for_messages(messages)) as f64
    }

    fn default_tools(&self) -> Vec<Tool> {
        self.tool_select
            .shape_tools(&self.tool_registry.list())
            .into_iter()
            .map(|entry| Tool {
                name: entry.info.name,
                description: entry.info.description,
                parameters: entry.info.parameters.unwrap_or_else(empty_tool_parameters),
                deferred: entry.deferred.then_some(true),
            })
            .collect()
    }

    fn should_recover_from_context_overflow(
        &self,
        error: &(dyn Error + 'static),
        estimated_request_tokens: Option<f64>,
    ) -> bool {
        if error
            .downcast_ref::<Error2>()
            .is_some_and(|error| error.code == CONTEXT_OVERFLOW)
        {
            return true;
        }
        let Some(status_error) = find_provider_error(error) else {
            return false;
        };
        if matches!(status_error, ChatProviderError::ApiContextOverflow { .. }) {
            return true;
        }
        if status_error.status_code() != Some(413) {
            return false;
        }
        let Ok(effective_max) = self.get_effective_max_context_tokens() else {
            return false;
        };
        effective_max > 0
            && estimated_request_tokens.unwrap_or_else(|| self.estimate_current_request_tokens())
                >= effective_max as f64 * OVERFLOW_STATUS_RECOVERY_RATIO
    }

    fn observe_context_overflow(&self, estimated_request_tokens: f64) {
        if !estimated_request_tokens.is_finite() || estimated_request_tokens <= 0.0 {
            return;
        }
        let Ok(data) = self.profile.data() else {
            return;
        };
        let Some(model_alias) = data.config.model_alias else {
            return;
        };
        let observed = (estimated_request_tokens * OVERFLOW_CONTEXT_SAFETY_RATIO)
            .floor()
            .max(1.0) as u64;
        let Ok(current) = self.get_effective_max_context_tokens() else {
            return;
        };
        if current > 0 && observed >= current {
            return;
        }
        self.state
            .lock()
            .unwrap()
            .observed_max_context_tokens_by_model
            .insert(model_alias, observed);
    }

    fn reserve_compaction_slot(&self, source: CompactionSource) -> bool {
        let mut state = self.state.lock().unwrap();
        if source == CompactionSource::Manual {
            state.compaction_count_in_turn = 0;
        } else {
            state.compaction_count_in_turn += 1;
        }
        state.compaction_count_in_turn as f64 <= DEFAULT_COMPACTION_CONFIG.max_compaction_per_turn
    }

    fn validate_compaction_start(
        &self,
        source: CompactionSource,
    ) -> Result<f64, FullCompactionError> {
        let history = self.context.get();
        if history.is_empty() {
            return Err(Arc::new(Error2::new(
                COMPACTION_UNABLE,
                "No messages to compact in current history.",
            )));
        }
        if source == CompactionSource::Manual
            && self.loop_service.status().state != AgentLoopState::Idle
        {
            return Err(Arc::new(Error2::new(
                COMPACTION_UNABLE,
                "Cannot compact while a turn is active. Wait for it to finish, then retry.",
            )));
        }
        Ok(estimate_tokens_for_messages(history.iter().map(|message| &message.message)) as f64)
    }

    fn create_active_compaction(
        &self,
        trigger: CompactionSource,
        token_count: f64,
        origin_turn_id: Option<crate::agent::TurnId>,
    ) -> Arc<ActiveCompaction> {
        let abort_controller = AbortController::new();
        let trace = Arc::new(Mutex::new(None));
        let (sender, receiver) = oneshot::channel();
        let promise: Shared<_> = async move {
            receiver.await.unwrap_or_else(|_| {
                Err(Arc::new(AbortError::new(
                    "Compaction task ended without a settlement.",
                )) as FullCompactionError)
            })
        }
        .boxed()
        .shared();
        Arc::new(ActiveCompaction {
            task: FullCompactionTask::new(abort_controller, promise, trigger, token_count, trace),
            origin_turn_id,
            blocked_by_turn: AtomicBool::new(false),
            settlement: Mutex::new(Some(sender)),
        })
    }

    fn cancel_active(&self, active: &Arc<ActiveCompaction>) -> bool {
        {
            let mut state = self.state.lock().unwrap();
            if state
                .compacting
                .as_ref()
                .is_none_or(|current| !Arc::ptr_eq(current, active))
            {
                return false;
            }
            if let Ok(operation) = full_compaction_cancel() {
                let _ = self.wire.dispatch([operation]);
            }
            state.compacting = None;
        }
        if !active.task.abort_controller.signal().aborted() {
            active.task.abort_controller.abort(None);
        }
        self.event_bus
            .publish(DomainEvent::new("compaction.cancelled", Map::new()));
        true
    }

    fn mark_completed(&self, active: &Arc<ActiveCompaction>) -> Result<bool, FullCompactionError> {
        let mut state = self.state.lock().unwrap();
        if state
            .compacting
            .as_ref()
            .is_none_or(|current| !Arc::ptr_eq(current, active))
        {
            return Ok(false);
        }
        self.wire
            .dispatch([full_compaction_complete().map_err(arc_error)?])
            .map_err(arc_error)?;
        state.compacting = None;
        Ok(true)
    }

    fn normalize_after_replay(&self) -> Result<(), FullCompactionError> {
        if self.wire.get_model(&COMPACTION_MODEL).phase != CompactionPhase::Running {
            return Ok(());
        }
        self.wire
            .dispatch([full_compaction_cancel().map_err(arc_error)?])
            .map_err(arc_error)
    }

    fn reset_for_turn(&self) {
        let mut state = self.state.lock().unwrap();
        state.compaction_count_in_turn = 0;
        state.last_compacted_token_count = None;
        state.consecutive_overflow_compactions = 0;
    }

    async fn recover_from_context_overflow(
        &self,
        context: &mut LoopErrorContext,
    ) -> Result<Option<bool>, LoopValue> {
        self.record_overflow_recovery(&context.error)
            .map_err(LoopValue::Error)?;
        let did_start = self.begin_auto_compaction(true).map_err(LoopValue::Error)?;
        if !did_start && self.compacting().is_none() {
            return Ok(Some(false));
        }
        self.block(Some(context.signal.clone()), Some(context.turn_id))
            .await
            .map_err(LoopValue::Error)?;
        let Some(driver) = context.failed_driver.clone() else {
            return Ok(Some(false));
        };
        if context
            .current_step
            .as_ref()
            .is_some_and(|step| step.0.signal().aborted())
        {
            return Ok(Some(false));
        }
        (context.retry)(
            driver,
            Some(StepEnqueueOptions {
                at: Some(StepRequestQueuePosition::Head),
            }),
        );
        Ok(Some(true))
    }

    fn record_overflow_recovery(&self, error: &LoopValue) -> Result<(), FullCompactionError> {
        self.observe_context_overflow(self.estimate_current_request_tokens());
        let max_attempts = self.strategy()?.max_overflow_compaction_attempts() as u64;
        let attempts = {
            let mut state = self.state.lock().unwrap();
            state.consecutive_overflow_compactions += 1;
            state.consecutive_overflow_compactions
        };
        if attempts <= max_attempts {
            return Ok(());
        }
        let cause = match error {
            LoopValue::Error(error) => Some(ErrorCause::Error(Arc::clone(error))),
            LoopValue::Value(value) => Some(ErrorCause::Value(value.clone())),
        };
        Err(Arc::new(Error2::with_options(
            CONTEXT_OVERFLOW,
            format!(
                "Compaction failed to bring the context under the model window after {max_attempts} attempts."
            ),
            Error2Options {
                cause,
                ..Error2Options::default()
            },
        )))
    }

    async fn before_step(
        &self,
        signal: AbortSignal,
        turn_id: Option<crate::agent::TurnId>,
    ) -> Result<(), FullCompactionError> {
        self.state.lock().unwrap().active_turn_id = turn_id;
        self.check_auto_compaction(true)?;
        if self
            .strategy()?
            .should_block(self.token_count_with_pending())
        {
            self.block(Some(signal), turn_id).await?;
        }
        Ok(())
    }

    fn after_step(&self) -> Result<(), FullCompactionError> {
        self.state.lock().unwrap().consecutive_overflow_compactions = 0;
        if self.strategy()?.check_after_step() {
            self.check_auto_compaction(false)?;
        }
        Ok(())
    }

    fn check_auto_compaction(&self, throw_on_limit: bool) -> Result<bool, FullCompactionError> {
        let (active, last_compacted) = {
            let state = self.state.lock().unwrap();
            (state.compacting.is_some(), state.last_compacted_token_count)
        };
        if active {
            return Ok(true);
        }
        let token_count = self.token_count_with_pending();
        if last_compacted.is_some_and(|last| token_count <= last) {
            return Ok(false);
        }
        if !self.strategy()?.should_compact(token_count) {
            return Ok(false);
        }
        self.begin_auto_compaction(throw_on_limit)
    }

    fn begin_auto_compaction(&self, throw_on_limit: bool) -> Result<bool, FullCompactionError> {
        if self.state.lock().unwrap().compacting.is_some() {
            return Ok(true);
        }
        let max_compactions = self.strategy()?.max_compaction_per_turn();
        if self.state.lock().unwrap().compaction_count_in_turn as f64 >= max_compactions {
            if throw_on_limit {
                return Err(Arc::new(Error2::with_options(
                    CONTEXT_OVERFLOW,
                    format!("Compaction limit exceeded ({max_compactions})"),
                    Error2Options {
                        details: Some(Map::from_iter([(
                            "maxCompactions".into(),
                            Value::from(max_compactions),
                        )])),
                        ..Error2Options::default()
                    },
                )));
            }
            return Ok(false);
        }
        self.begin(FullCompactionInput {
            source: CompactionSource::Auto,
            instruction: None,
        })
    }

    async fn block(
        &self,
        signal: Option<AbortSignal>,
        turn_id: Option<crate::agent::TurnId>,
    ) -> Result<(), FullCompactionError> {
        let Some(active) = self.state.lock().unwrap().compacting.clone() else {
            return Ok(());
        };
        active.blocked_by_turn.store(true, Ordering::Release);
        let mut fields = Map::new();
        if let Some(turn_id) = turn_id {
            fields.insert("turnId".into(), Value::from(turn_id.get()));
        }
        self.event_bus
            .publish(DomainEvent::new("compaction.blocked", fields));
        let completion = active.task.promise.clone();
        let caller_signal = signal.clone();
        let result = match signal {
            Some(signal) => {
                tokio::select! {
                    result = completion.clone() => result,
                    reason = signal.cancelled() => {
                        active.task.abort_controller.abort(Some((*reason).clone()));
                        completion.await
                    }
                }
            }
            None => completion.await,
        };
        match result {
            Ok(_) => Ok(()),
            Err(error)
                if caller_signal.as_ref().is_some_and(AbortSignal::aborted)
                    && (active.task.abort_controller.signal().aborted()
                        || is_compaction_abort(error.as_ref())) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn compaction_worker(
        self: Arc<Self>,
        active: Arc<ActiveCompaction>,
        data: CompactionBeginData,
    ) {
        let result = match AssertUnwindSafe(self.compaction_worker_result(&active, &data))
            .catch_unwind()
            .await
        {
            Ok(result) => result,
            Err(payload) => {
                self.cancel_active(&active);
                Err(Arc::new(Error2::new(
                    crate::_base::errors::codes::CORE_INTERNAL,
                    format!(
                        "Full compaction task panicked: {}",
                        panic_payload_message(payload)
                    ),
                )) as FullCompactionError)
            }
        };
        self.on_did_finish_compaction.fire(&active.task);
        active.settle(result);
    }

    fn request_shutdown(&self) {
        let _begin_guard = self.begin_lock.lock().unwrap();
        self.shutdown.cancel();
        self.tasks.close();
        if let Some(active) = self.state.lock().unwrap().compacting.clone()
            && !active.task.abort_controller.signal().aborted()
        {
            active
                .task
                .abort_controller
                .abort(Some(AbortError::new("Full compaction service shut down.")));
        }
    }

    async fn compaction_worker_result(
        &self,
        active: &Arc<ActiveCompaction>,
        data: &CompactionBeginData,
    ) -> Result<CompactionResult, FullCompactionError> {
        let result = async {
            let result = self.compaction_round(active, data).await?;
            if !self.is_active(active) {
                return Err(compaction_cancelled_reason(active));
            }
            self.profile.refresh_system_prompt().await;
            self.state.lock().unwrap().last_compacted_token_count = Some(result.tokens_after);
            self.context_injector()?
                .inject_after_compaction()
                .await
                .map_err(FullCompactionError::from)?;
            self.state.lock().unwrap().last_compacted_token_count =
                Some(self.token_count_with_pending());
            if !self.mark_completed(active)? {
                return Err(compaction_cancelled_reason(active));
            }
            self.publish_completed(&result)?;
            Ok(result)
        }
        .await;

        match result {
            Ok(result) => Ok(result),
            Err(error)
                if active.task.abort_controller.signal().aborted()
                    || is_compaction_abort(error.as_ref()) =>
            {
                self.cancel_active(active);
                Err(error)
            }
            Err(error) => {
                let blocked =
                    self.is_active(active) && active.blocked_by_turn.load(Ordering::Acquire);
                if self.is_active(active) {
                    self.cancel_active(active);
                }
                if !blocked {
                    self.publish_error(error.as_ref());
                }
                Err(error)
            }
        }
    }

    async fn compaction_round(
        &self,
        active: &Arc<ActiveCompaction>,
        data: &CompactionBeginData,
    ) -> Result<CompactionResult, FullCompactionError> {
        let started_at = Instant::now();
        let original_history = self.context.get();
        let tokens_before =
            estimate_tokens_for_messages(original_history.iter().map(|message| &message.message))
                as f64;
        let mut retry_count = 0usize;
        let mut thinking_effort = self
            .profile
            .data()
            .map(|data| ThinkingEffort::new(data.config.thinking_level))
            .unwrap_or_else(|_| ThinkingEffort::new("off"));

        let round = async {
            active
                .task
                .abort_controller
                .signal()
                .throw_if_aborted()
                .map_err(|error| error as FullCompactionError)?;
            let mut hook_task = active.task.clone();
            self.hooks
                .on_will_compact
                .run(&mut hook_task, None)
                .await
                .map_err(|error| Arc::from(error) as FullCompactionError)?;

            let resolved = self
                .profile
                .resolve_model_context()
                .map_err(FullCompactionError::from)?;
            thinking_effort = resolved.thinking_level.clone();
            let default_cap = (resolved.model_capabilities.max_context_tokens > 0).then_some(
                resolved
                    .model_capabilities
                    .max_context_tokens
                    .min(DEFAULT_COMPACTION_MAX_COMPLETION_TOKENS),
            );
            let max_output_size = resolved.max_output_size.or(default_cap);
            let custom_instruction = data
                .instruction
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            let custom_block = if custom_instruction.is_empty() {
                String::new()
            } else {
                format!("\nOptional user instruction:\n{custom_instruction}\n")
            };
            let instruction = render_prompt(
                COMPACTION_INSTRUCTION_TEMPLATE,
                &HashMap::from([(
                    "custom_instruction_block".into(),
                    Value::String(custom_block),
                )]),
            )
            .trim_end()
            .to_owned();

            let delays = retry_backoff_delays(MAX_COMPACTION_RETRY_ATTEMPTS);
            let mut history_for_model = strip_dynamic_tool_context(&original_history);
            let mut dropped_count = 0usize;
            let mut overflow_shrink_count = 0usize;
            let mut empty_or_truncated_shrink_count = 0usize;
            let attempt = loop {
                let messages_to_compact = history_for_model.clone();
                let mut messages = messages_to_compact
                    .iter()
                    .map(|message| message.message.clone())
                    .collect::<Vec<_>>();
                messages.push(create_user_message(&instruction));
                let estimated_tokens = self.estimate_request_tokens(&messages);
                let request = self.llm_requester.start(
                    Some(AgentLlmRequestOverrides {
                        messages: Some(messages),
                        max_output_size,
                        source: Some(AgentLlmRequestSource::Operation {
                            turn_id: active.origin_turn_id,
                            request_kind: Some("full_compaction".into()),
                            log_fields: Some(Map::from_iter([(
                                "droppedCount".into(),
                                Value::from(dropped_count as u64),
                            )])),
                        }),
                        ..AgentLlmRequestOverrides::default()
                    }),
                    None,
                    Some(active.task.abort_controller.signal()),
                );
                active.task.set_trace(request.trace.clone());
                match request.result.await.and_then(collect_summary) {
                    Ok(attempt) => break attempt,
                    Err(error) => {
                        if self.should_recover_from_context_overflow(
                            error.as_ref(),
                            Some(estimated_tokens),
                        ) {
                            self.observe_context_overflow(estimated_tokens);
                            overflow_shrink_count += 1;
                            if overflow_shrink_count > MAX_COMPACTION_OVERFLOW_SHRINK_ATTEMPTS
                                || messages_to_compact.len() <= 1
                            {
                                return Err(error);
                            }
                            let before = messages_to_compact.len();
                            history_for_model = shrink_compaction_history_after_overflow(
                                &messages_to_compact,
                                overflow_shrink_count,
                            );
                            dropped_count += before - history_for_model.len();
                            retry_count = 0;
                            continue;
                        }
                        if (error.downcast_ref::<CompactionTruncatedError>().is_some()
                            || matches!(
                                find_provider_error(error.as_ref()),
                                Some(ChatProviderError::ApiEmptyResponse { .. })
                            ))
                            && messages_to_compact.len() > 1
                        {
                            empty_or_truncated_shrink_count += 1;
                            if empty_or_truncated_shrink_count > MAX_COMPACTION_RETRY_ATTEMPTS {
                                return Err(error);
                            }
                            let reduced =
                                drop_oldest_message_and_leading_tool_results(&messages_to_compact);
                            dropped_count += messages_to_compact.len() - reduced.len();
                            history_for_model = reduced;
                            retry_count = 0;
                            continue;
                        }
                        let retryable = find_provider_error(error.as_ref())
                            .is_some_and(is_retryable_generate_error);
                        if !retryable || retry_count + 1 >= MAX_COMPACTION_RETRY_ATTEMPTS {
                            return Err(error);
                        }
                        sleep_for_retry(
                            delays[retry_count],
                            Some(&active.task.abort_controller.signal()),
                        )
                        .await
                        .map_err(|error| error as FullCompactionError)?;
                        retry_count += 1;
                    }
                }
            };

            if !history_safe_to_compact(&self.context.get(), &original_history) {
                self.cancel_active(active);
                return Err(compaction_cancelled_reason(active));
            }
            active.task.set_trace_id(attempt.trace_id.clone());
            let summary = self.post_process_summary(&attempt.summary);
            let result = self
                .context
                .apply_compaction(ContextCompactionInput {
                    summary: summary.clone(),
                    context_summary: Some(build_compaction_summary_text(&summary)),
                    compacted_count: original_history.len() as f64,
                    tokens_before,
                    tokens_after: None,
                    kept_user_message_count: None,
                    kept_head_user_message_count: None,
                    dropped_count: (dropped_count > 0).then_some(dropped_count as f64),
                })
                .map_err(arc_error)?;
            let result = CompactionResult {
                summary: result.summary,
                context_summary: Some(result.context_summary),
                compacted_count: result.compacted_count,
                tokens_before: result.tokens_before,
                tokens_after: result.tokens_after,
                kept_user_message_count: Some(result.kept_user_message_count),
                kept_head_user_message_count: result.kept_head_user_message_count,
                dropped_count: result.dropped_count,
            };
            self.track_finished(
                active,
                data,
                &result,
                started_at,
                retry_count,
                &thinking_effort,
                &attempt,
            );
            Ok(result)
        }
        .await;

        match round {
            Ok(result) => Ok(result),
            Err(error) if is_compaction_abort(error.as_ref()) => Err(error),
            Err(error) => {
                self.track_failed(
                    active,
                    data,
                    tokens_before,
                    started_at,
                    retry_count,
                    &thinking_effort,
                    error.as_ref(),
                );
                if error.downcast_ref::<Error2>().is_some_and(|error| {
                    error.code == AUTH_LOGIN_REQUIRED || error.code == PROVIDER_AUTH_ERROR
                }) {
                    return Err(error);
                }
                Err(Arc::new(Error2::with_options(
                    super::COMPACTION_FAILED,
                    error.to_string(),
                    Error2Options {
                        cause: Some(ErrorCause::Error(error)),
                        ..Error2Options::default()
                    },
                )))
            }
        }
    }

    fn post_process_summary(&self, summary: &str) -> String {
        let todos = self.todo.get_todos();
        if todos.is_empty() {
            return summary.to_owned();
        }
        format!(
            "{}\n\n{}",
            summary.trim(),
            render_todo_list(&todos, Some("## TODO List"))
        )
    }

    fn token_count_with_pending(&self) -> f64 {
        self.context_size.get(None, None).size
    }

    fn context_injector(&self) -> Result<AgentContextInjectorServiceHandle, FullCompactionError> {
        if let Some(service) = self.context_injector.lock().unwrap().clone() {
            return Ok(service);
        }
        let service = self
            .instantiation
            .get(AGENT_CONTEXT_INJECTOR_SERVICE_ID)
            .map_err(arc_error)?;
        let service = (*service).clone();
        *self.context_injector.lock().unwrap() = Some(service.clone());
        Ok(service)
    }

    fn is_active(&self, active: &Arc<ActiveCompaction>) -> bool {
        self.state
            .lock()
            .unwrap()
            .compacting
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, active))
    }

    fn publish_completed(&self, result: &CompactionResult) -> Result<(), FullCompactionError> {
        let mut value = serde_json::to_value(result).map_err(arc_error)?;
        if let Value::Object(result) = &mut value {
            result.remove("contextSummary");
        }
        self.event_bus.publish(DomainEvent::new(
            "compaction.completed",
            Map::from_iter([("result".into(), value)]),
        ));
        Ok(())
    }

    fn publish_error(&self, error: &(dyn Error + 'static)) {
        let fields = match serde_json::to_value(to_error_payload(error)) {
            Ok(Value::Object(fields)) => fields,
            _ => Map::new(),
        };
        self.event_bus.publish(DomainEvent::new("error", fields));
    }

    #[allow(clippy::too_many_arguments)]
    fn track_finished(
        &self,
        active: &ActiveCompaction,
        data: &CompactionBeginData,
        result: &CompactionResult,
        started_at: Instant,
        retry_count: usize,
        thinking_effort: &ThinkingEffort,
        attempt: &CompactionAttemptResult,
    ) {
        let mut properties = TelemetryProperties::new();
        insert_value(
            &mut properties,
            "turn_id",
            active
                .origin_turn_id
                .map(|turn_id| Value::from(turn_id.get())),
        );
        insert_json(&mut properties, "source", data.source);
        insert_value(
            &mut properties,
            "tokens_before",
            Some(Value::from(result.tokens_before)),
        );
        insert_value(
            &mut properties,
            "tokens_after",
            Some(Value::from(result.tokens_after)),
        );
        insert_value(
            &mut properties,
            "duration_ms",
            Some(Value::from(started_at.elapsed().as_millis() as u64)),
        );
        insert_value(
            &mut properties,
            "compacted_count",
            Some(Value::from(result.compacted_count)),
        );
        insert_value(
            &mut properties,
            "dropped_count",
            result.dropped_count.map(Value::from),
        );
        insert_value(
            &mut properties,
            "retry_count",
            Some(Value::from(retry_count as u64)),
        );
        insert_value(&mut properties, "round", Some(Value::from(1)));
        insert_value(
            &mut properties,
            "thinking_effort",
            Some(Value::String(thinking_effort.to_string())),
        );
        insert_value(
            &mut properties,
            "trace_id",
            attempt.trace_id.clone().map(Value::String),
        );
        if let Some(usage) = attempt.usage {
            insert_value(
                &mut properties,
                "input_tokens",
                Some(Value::from(input_total(&usage))),
            );
            insert_value(
                &mut properties,
                "output_tokens",
                Some(Value::from(usage.output)),
            );
            insert_value(
                &mut properties,
                "input_cache_read",
                Some(Value::from(usage.input_cache_read)),
            );
            insert_value(
                &mut properties,
                "input_cache_creation",
                Some(Value::from(usage.input_cache_creation)),
            );
        }
        self.telemetry
            .track("compaction_finished", Some(&properties));
    }

    #[allow(clippy::too_many_arguments)]
    fn track_failed(
        &self,
        active: &ActiveCompaction,
        data: &CompactionBeginData,
        tokens_before: f64,
        started_at: Instant,
        retry_count: usize,
        thinking_effort: &ThinkingEffort,
        error: &(dyn Error + 'static),
    ) {
        let mut properties = TelemetryProperties::new();
        insert_value(
            &mut properties,
            "turn_id",
            active
                .origin_turn_id
                .map(|turn_id| Value::from(turn_id.get())),
        );
        insert_json(&mut properties, "source", data.source);
        insert_value(
            &mut properties,
            "tokens_before",
            Some(Value::from(tokens_before)),
        );
        insert_value(
            &mut properties,
            "duration_ms",
            Some(Value::from(started_at.elapsed().as_millis() as u64)),
        );
        insert_value(&mut properties, "round", Some(Value::from(1)));
        insert_value(
            &mut properties,
            "retry_count",
            Some(Value::from(retry_count as u64)),
        );
        insert_value(
            &mut properties,
            "thinking_effort",
            Some(Value::String(thinking_effort.to_string())),
        );
        insert_value(
            &mut properties,
            "error_type",
            Some(Value::String(error_name(error))),
        );
        insert_value(
            &mut properties,
            "trace_id",
            find_provider_error(error)
                .and_then(provider_trace_id)
                .or_else(|| active.task.trace_id())
                .map(Value::String),
        );
        self.telemetry.track("compaction_failed", Some(&properties));
    }
}

#[async_trait]
impl AgentFullCompactionServiceContract for AgentFullCompactionService {
    fn compacting(&self) -> Option<FullCompactionTask> {
        self.state
            .lock()
            .unwrap()
            .compacting
            .as_ref()
            .map(|active| active.task.clone())
    }

    fn begin(&self, input: FullCompactionInput) -> Result<bool, FullCompactionError> {
        let _begin_guard = self.begin_lock.lock().unwrap();
        if self.shutdown.is_cancelled() {
            return Err(Arc::new(AbortError::new(
                "Full compaction service was disposed.",
            )));
        }
        if self.state.lock().unwrap().compacting.is_some() {
            return Ok(false);
        }
        if !self.reserve_compaction_slot(input.source) {
            return Ok(false);
        }
        let token_count = self.validate_compaction_start(input.source)?;
        let data = CompactionBeginData {
            source: input.source,
            instruction: input.instruction,
        };
        self.wire
            .dispatch([full_compaction_begin(data.clone()).map_err(arc_error)?])
            .map_err(arc_error)?;
        let origin_turn_id = (input.source == CompactionSource::Auto)
            .then(|| self.state.lock().unwrap().active_turn_id)
            .flatten();
        let active = self.create_active_compaction(input.source, token_count, origin_turn_id);
        self.state.lock().unwrap().compacting = Some(Arc::clone(&active));

        let weak = self.self_weak.get().cloned().unwrap_or_default();
        let abort_active = Arc::clone(&active);
        self.tasks.spawn(async move {
            let signal = abort_active.task.abort_controller.signal();
            tokio::select! {
                _ = signal.cancelled() => {
                    if let Some(service) = weak.upgrade() {
                        service.cancel_active(&abort_active);
                    }
                }
                _ = abort_active.task.promise.clone() => {}
            }
        });
        let service = self
            .self_weak
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| {
                Arc::new(AbortError::new("Full compaction service was disposed."))
                    as FullCompactionError
            })?;
        self.tasks.spawn(service.compaction_worker(active, data));
        Ok(true)
    }

    async fn shutdown(&self) {
        self.request_shutdown();
        self.tasks.wait().await;
    }

    fn hooks(&self) -> &AgentFullCompactionHooks {
        &self.hooks
    }

    fn on_did_finish_compaction(&self) -> Event<FullCompactionTask> {
        self.on_did_finish_compaction.event()
    }
}

impl Disposable for AgentFullCompactionService {
    fn dispose(&self) -> DisposeResult {
        self.request_shutdown();
        let result = self.disposables.dispose();
        let emitter_result = self.on_did_finish_compaction.dispose();
        result.and(emitter_result)
    }
}

struct CompactionLoopErrorHandler {
    service: Weak<AgentFullCompactionService>,
}

#[async_trait]
impl LoopErrorHandler for CompactionLoopErrorHandler {
    fn id(&self) -> &str {
        "full-compaction"
    }

    fn matches(&self, context: &LoopErrorContext) -> bool {
        let Some(service) = self.service.upgrade() else {
            return false;
        };
        match &context.error {
            LoopValue::Error(error) => {
                service.should_recover_from_context_overflow(error.as_ref(), None)
            }
            LoopValue::Value(_) => false,
        }
    }

    async fn handle(&self, context: &mut LoopErrorContext) -> Result<Option<bool>, LoopValue> {
        match self.service.upgrade() {
            Some(service) => service.recover_from_context_overflow(context).await,
            None => Ok(Some(false)),
        }
    }
}

fn empty_tool_parameters() -> Map<String, Value> {
    Map::from_iter([
        ("type".into(), Value::String("object".into())),
        ("properties".into(), Value::Object(Map::new())),
    ])
}

fn collect_summary(
    finish: AgentLlmRequestFinish,
) -> Result<CompactionAttemptResult, AgentLlmRequestError> {
    if finish.provider_finish_reason == Some(FinishReason::Truncated) {
        return Err(Arc::new(CompactionTruncatedError));
    }
    let summary = finish
        .message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_owned();
    if summary.is_empty() {
        return Err(Arc::new(ChatProviderError::empty_response(
            "The compaction response did not contain a non-empty summary.",
            None,
            finish.raw_finish_reason.clone(),
        )));
    }
    Ok(CompactionAttemptResult {
        summary,
        usage: Some(finish.usage),
        trace_id: finish.trace_id,
    })
}

fn history_safe_to_compact(current: &[ContextMessage], original: &[ContextMessage]) -> bool {
    current.len() >= original.len()
        && current[..original.len()] == *original
        && current[original.len()..].iter().all(is_real_user_input)
}

fn shrink_compaction_history_after_overflow(
    messages: &[ContextMessage],
    attempt: usize,
) -> Vec<ContextMessage> {
    if messages.len() <= 1 {
        return messages.to_vec();
    }
    let ratio = COMPACTION_OVERFLOW_SHRINK_RATIOS
        [(attempt - 1).min(COMPACTION_OVERFLOW_SHRINK_RATIOS.len() - 1)];
    let token_budget =
        (estimate_tokens_for_messages(messages.iter().map(|message| &message.message)) as f64
            * ratio)
            .floor() as usize;
    take_recent_messages_within_token_budget(messages, token_budget)
}

fn take_recent_messages_within_token_budget(
    messages: &[ContextMessage],
    token_budget: usize,
) -> Vec<ContextMessage> {
    let mut start = messages.len();
    let mut tokens = 0usize;
    for index in (0..messages.len()).rev() {
        let message_tokens = estimate_tokens_for_message(&messages[index].message);
        if tokens + message_tokens > token_budget {
            break;
        }
        tokens += message_tokens;
        start = index;
    }
    if start == 0 {
        start = 1;
    }
    drop_leading_tool_results(&messages[start..])
}

fn drop_oldest_message_and_leading_tool_results(
    messages: &[ContextMessage],
) -> Vec<ContextMessage> {
    if messages.len() <= 1 {
        return messages.to_vec();
    }
    drop_leading_tool_results(&messages[1..])
}

fn drop_leading_tool_results(messages: &[ContextMessage]) -> Vec<ContextMessage> {
    let start = messages
        .iter()
        .position(|message| message.message.role != Role::Tool)
        .unwrap_or(messages.len());
    messages[start..].to_vec()
}

fn find_provider_error<'a>(mut error: &'a (dyn Error + 'static)) -> Option<&'a ChatProviderError> {
    loop {
        if let Some(error) = error.downcast_ref::<ChatProviderError>() {
            return Some(error);
        }
        error = error.source()?;
    }
}

fn provider_trace_id(error: &ChatProviderError) -> Option<String> {
    error.status_data().and_then(|data| data.trace_id.clone())
}

fn is_compaction_abort(error: &(dyn Error + 'static)) -> bool {
    is_abort_error(error) || matches!(find_provider_error(error), Some(ChatProviderError::Abort))
}

fn compaction_cancelled_reason(active: &ActiveCompaction) -> FullCompactionError {
    active
        .task
        .abort_controller
        .signal()
        .reason()
        .map(|reason| reason as FullCompactionError)
        .unwrap_or_else(|| Arc::new(AbortError::new("Compaction cancelled.")))
}

fn error_name(error: &(dyn Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<Error2>() {
        return error.name.clone();
    }
    if let Some(error) = error.downcast_ref::<ChatProviderError>() {
        return error.name().into();
    }
    if let Some(error) = error.downcast_ref::<AbortError>() {
        return error.name().into();
    }
    if error.downcast_ref::<CompactionTruncatedError>().is_some() {
        return "CompactionTruncatedError".into();
    }
    "Error".into()
}

fn insert_value(properties: &mut TelemetryProperties, key: &str, value: Option<Value>) {
    properties.insert(key.into(), value);
}

fn insert_json(properties: &mut TelemetryProperties, key: &str, value: impl serde::Serialize) {
    insert_value(properties, key, serde_json::to_value(value).ok());
}

fn arc_error(error: impl Error + Send + Sync + 'static) -> FullCompactionError {
    Arc::new(error)
}

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("task panicked")
        .to_owned()
}

fn loop_value_error(error: LoopValue) -> FullCompactionError {
    match error {
        LoopValue::Error(error) => error,
        LoopValue::Value(value) => Arc::new(std::io::Error::other(value.to_string())),
    }
}

pub fn register_agent_full_compaction_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_FULL_COMPACTION_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let context_size = accessor.get(AGENT_CONTEXT_SIZE_SERVICE_ID)?;
            let requester = accessor.get(AGENT_LLM_REQUESTER_SERVICE_ID)?;
            let profile = accessor.get(AGENT_PROFILE_SERVICE_ID)?;
            let registry = accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?;
            let tool_select = accessor.get(AGENT_TOOL_SELECT_SERVICE_ID)?;
            let instantiation = accessor.get(INSTANTIATION_SERVICE_ID)?;
            let todo = accessor.get(SESSION_TODO_SERVICE_ID)?;
            let telemetry = accessor.get(TELEMETRY_SERVICE_ID)?;
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let loop_service = accessor.get(AGENT_LOOP_SERVICE_ID)?;
            let service = AgentFullCompactionService::new(
                (*context).clone(),
                (*context_size).clone(),
                (*requester).clone(),
                (*profile).clone(),
                (*registry).clone(),
                (*tool_select).clone(),
                instantiation,
                (*todo).clone(),
                (*telemetry).clone(),
                (*wire).clone(),
                (*event_bus).clone(),
                (*log).clone(),
                (*loop_service).clone(),
            )
            .map_err(|error| DiError::Factory(error.to_string()))?;
            let contract: Arc<dyn AgentFullCompactionServiceContract> = service;
            Ok(AgentFullCompactionServiceHandle(contract))
        })
        .disposable(),
        InstantiationType::Eager,
        "fullCompaction",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::context_memory::PromptOrigin,
        kosong::contract::{
            message::{
                ToolOutput, create_assistant_message, create_tool_message, create_user_message,
            },
            usage::empty_usage,
        },
    };

    fn context_message(message: Message, origin: Option<PromptOrigin>) -> ContextMessage {
        ContextMessage {
            message,
            id: None,
            provider_message_id: None,
            origin,
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    fn finish(content: Vec<ContentPart>) -> AgentLlmRequestFinish {
        AgentLlmRequestFinish {
            message: create_assistant_message(content, None),
            usage: empty_usage(),
            model: Some("test-model".into()),
            provider_finish_reason: Some(FinishReason::Completed),
            raw_finish_reason: Some("stop".into()),
            provider_message_id: Some("message-1".into()),
            timing: None,
            trace_id: Some("trace-1".into()),
        }
    }

    #[test]
    fn summary_collection_matches_text_only_finish_semantics() {
        let attempt = collect_summary(finish(vec![
            ContentPart::Think {
                think: "internal".into(),
                encrypted: None,
            },
            ContentPart::Text {
                text: "  first ".into(),
            },
            ContentPart::Text {
                text: "second  ".into(),
            },
        ]))
        .unwrap();

        assert_eq!(attempt.summary, "first second");
        assert_eq!(attempt.trace_id.as_deref(), Some("trace-1"));
        assert_eq!(attempt.usage, Some(empty_usage()));
    }

    #[test]
    fn truncated_and_empty_summaries_remain_distinguishable() {
        let mut truncated = finish(vec![ContentPart::Text {
            text: "partial".into(),
        }]);
        truncated.provider_finish_reason = Some(FinishReason::Truncated);
        let error = collect_summary(truncated).unwrap_err();
        assert!(error.downcast_ref::<CompactionTruncatedError>().is_some());

        let empty = collect_summary(finish(vec![ContentPart::Think {
            think: "reasoning only".into(),
            encrypted: None,
        }]))
        .unwrap_err();
        assert!(matches!(
            empty.downcast_ref::<ChatProviderError>(),
            Some(ChatProviderError::ApiEmptyResponse {
                raw_finish_reason: Some(reason),
                ..
            }) if reason == "stop"
        ));
    }

    #[test]
    fn history_safety_allows_only_real_user_inputs_after_an_unchanged_prefix() {
        let original = vec![
            context_message(create_user_message("one"), Some(PromptOrigin::User)),
            context_message(
                create_assistant_message(
                    vec![ContentPart::Text {
                        text: "answer".into(),
                    }],
                    None,
                ),
                None,
            ),
        ];
        assert!(history_safe_to_compact(&original, &original));

        let mut with_user = original.clone();
        with_user.push(context_message(
            create_user_message("queued prompt"),
            Some(PromptOrigin::User),
        ));
        assert!(history_safe_to_compact(&with_user, &original));

        let mut with_injection = original.clone();
        with_injection.push(context_message(
            create_user_message("reminder"),
            Some(PromptOrigin::Injection {
                variant: "goal".into(),
            }),
        ));
        assert!(!history_safe_to_compact(&with_injection, &original));

        let mut changed_prefix = original.clone();
        changed_prefix[0] =
            context_message(create_user_message("changed"), Some(PromptOrigin::User));
        assert!(!history_safe_to_compact(&changed_prefix, &original));
    }

    #[test]
    fn history_reduction_never_starts_with_orphaned_tool_results() {
        let messages = vec![
            context_message(create_user_message("oldest"), Some(PromptOrigin::User)),
            context_message(
                create_tool_message("call-1", ToolOutput::Text("first".into())),
                None,
            ),
            context_message(
                create_tool_message("call-2", ToolOutput::Text("second".into())),
                None,
            ),
            context_message(create_user_message("newest"), Some(PromptOrigin::User)),
        ];

        let reduced = drop_oldest_message_and_leading_tool_results(&messages);
        assert_eq!(reduced.len(), 1);
        assert_eq!(reduced[0].message.role, Role::User);

        let overflow_reduced = shrink_compaction_history_after_overflow(&messages, 1);
        assert!(!overflow_reduced.is_empty());
        assert_ne!(overflow_reduced[0].message.role, Role::Tool);
        assert!(overflow_reduced.len() < messages.len());
    }

    #[test]
    fn nested_provider_errors_retain_status_and_trace_metadata() {
        let provider: FullCompactionError = Arc::new(ChatProviderError::ApiContextOverflow {
            message: "context length exceeded".into(),
            data: crate::kosong::contract::errors::ApiStatusData::new(
                413,
                Some("request-1".into()),
                None,
                Some("trace-overflow".into()),
            ),
        });
        let wrapped = Error2::with_options(
            super::super::COMPACTION_FAILED,
            "wrapped",
            Error2Options {
                cause: Some(ErrorCause::Error(provider)),
                ..Error2Options::default()
            },
        );

        let found = find_provider_error(&wrapped).unwrap();
        assert_eq!(found.status_code(), Some(413));
        assert_eq!(provider_trace_id(found).as_deref(), Some("trace-overflow"));
    }
}
