//! Agent-scoped FIFO turn and step loop implementation.
//!
//! Coordinates context materialization, LLM streaming, tool execution, durable
//! turn operations, domain events, telemetry, hooks, cancellation, and ordered
//! error recovery. Bound at Agent scope.

use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};

use async_trait::async_trait;
use futures_util::{
    FutureExt, StreamExt,
    future::{BoxFuture, Shared},
};
use serde_json::{Map, Value};
use tokio::sync::oneshot;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableHandle, DisposeResult, to_disposable},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::{
            errors::{BugIndicatingError, Error2, Error2Options},
            serialize::{KimiErrorPayload, to_error_payload, to_error_payload_value},
        },
        utils::abort::{
            AbortController, AbortError, AbortLink, AbortSignal, abort_error, is_abort_error,
            is_user_cancellation, link_abort_signal, user_cancellation_reason,
        },
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract,
            AgentContextMemoryServiceHandle, LoopRecordedEvent, LoopToolResult,
            LoopToolResultOutput,
        },
        llm_requester::{
            AGENT_LLM_REQUESTER_SERVICE_ID, AgentLlmRequestFinish, AgentLlmRequestOverrides,
            AgentLlmRequestPartHandler, AgentLlmRequestSource, AgentLlmRequesterServiceContract,
            AgentLlmRequesterServiceHandle,
        },
        tool_executor::{
            AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceContract,
            AgentToolExecutorServiceHandle, ToolCallStartedPayload, ToolExecutorExecuteOptions,
        },
    },
    app::{
        config::{CONFIG_SERVICE_ID, ConfigServiceContract, ConfigServiceHandle},
        event::event_bus::{
            DomainEvent, EVENT_BUS_SERVICE_ID, EventBusContract, EventBusHandle, TypedEventBusExt,
        },
        telemetry::{
            AGENT_TELEMETRY_CONTEXT_SERVICE_ID, AgentTelemetryContextPatch,
            AgentTelemetryContextServiceContract, AgentTelemetryContextServiceHandle,
            TELEMETRY_SERVICE_ID, TelemetryServiceContract, TelemetryServiceEventExt,
            TelemetryServiceHandle,
            event_payloads::{
                TurnEndReason as TelemetryTurnEndReason, TurnEndedEvent as TelemetryTurnEndedEvent,
                TurnInterruptReason, TurnInterruptedEvent,
                TurnStartedEvent as TelemetryTurnStartedEvent,
            },
        },
    },
    kosong::{
        contract::{
            message::{ContentPart, StreamIndex, StreamedMessagePart},
            provider::FinishReason,
            request_trace::LlmRequestTrace,
            usage::TokenUsage,
        },
        protocol::errors::{PROVIDER_FILTERED, ensure_protocol_errors_registered},
    },
    tool::ExecutableToolOutput,
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    AGENT_LOOP_SERVICE_ID, AfterStepContext, AgentLoopHooks, AgentLoopServiceContract,
    AgentLoopServiceHandle, AgentLoopState, AgentLoopStatus, BeforeStepContext, EnqueueReceipt,
    LOOP_CONTROL_SECTION, LoopControl, LoopErrorContext, LoopErrorHandler,
    LoopErrorHandlerRegistrationOptions, LoopRunOptions, LoopRunResult, LoopValue, StepAssignment,
    StepAssignmentFuture, StepEnqueueOptions, StepHandle, StepHandleContract, StepRequest,
    StepRequestAdmission, StepRequestBatch, StepRequestQueue, StepRequestState, StepResult,
    StepResultFuture, StepState, TURN_MODEL, TurnHandle, TurnHandleContract, TurnReadyFuture,
    TurnResultFuture, TurnSeed, TurnState, cancel_turn, create_max_steps_exceeded_error,
    ensure_turn_wire_registered, is_displayable_prompt_origin, is_max_steps_exceeded_error,
    prompt_turn, turn_prompt_text,
};

type AssignmentPromise = Promise<Result<StepAssignment, LoopValue>>;
type ReadyPromise = Promise<Result<(), LoopValue>>;
type ResultPromise = Promise<LoopRunResult>;

struct Promise<T: Clone + Send + 'static> {
    sender: Mutex<Option<oneshot::Sender<T>>>,
    future: Shared<BoxFuture<'static, T>>,
}

impl<T: Clone + Send + 'static> Promise<T> {
    fn new() -> Self {
        let (sender, receiver) = oneshot::channel();
        Self {
            sender: Mutex::new(Some(sender)),
            future: async move {
                receiver
                    .await
                    .expect("loop promise sender must settle before it is dropped")
            }
            .boxed()
            .shared(),
        }
    }

    fn settle(&self, value: T) -> bool {
        self.sender
            .lock()
            .unwrap()
            .take()
            .is_some_and(|sender| sender.send(value).is_ok())
    }

    fn future(&self) -> Shared<BoxFuture<'static, T>> {
        self.future.clone()
    }
}

struct StepMutable {
    state: StepState,
    controller: AbortController,
}

struct StepHandleImpl {
    id: String,
    turn_id: i64,
    request: Arc<dyn StepRequest>,
    mutable: Mutex<StepMutable>,
    result: Arc<Promise<StepResult>>,
}

impl StepHandleImpl {
    fn set_running(&self) -> AbortSignal {
        let mut mutable = self.mutable.lock().unwrap();
        mutable.state = StepState::Running;
        mutable.controller = AbortController::new();
        mutable.controller.signal()
    }

    fn complete(&self) {
        self.mutable.lock().unwrap().state = StepState::Completed;
        self.result.settle(StepResult::Completed);
    }
}

impl StepHandleContract for StepHandleImpl {
    fn id(&self) -> &str {
        &self.id
    }

    fn turn_id(&self) -> i64 {
        self.turn_id
    }

    fn state(&self) -> StepState {
        self.mutable.lock().unwrap().state
    }

    fn signal(&self) -> AbortSignal {
        self.mutable.lock().unwrap().controller.signal()
    }

    fn result(&self) -> StepResultFuture {
        self.result.future()
    }

    fn cancel(&self, reason: Option<LoopValue>) -> bool {
        let cancellation = reason.unwrap_or_else(user_cancellation_value);
        let mut mutable = self.mutable.lock().unwrap();
        if matches!(
            mutable.state,
            StepState::Completed | StepState::Failed | StepState::Cancelled
        ) {
            return false;
        }
        mutable.state = StepState::Cancelled;
        self.request.abort();
        mutable
            .controller
            .abort(Some(abort_from_value(&cancellation)));
        drop(mutable);
        self.result.settle(StepResult::Cancelled {
            reason: cancellation,
        });
        true
    }
}

struct TurnMutable {
    state: TurnState,
}

struct TurnHandleImpl {
    id: i64,
    mutable: Mutex<TurnMutable>,
    controller: AbortController,
    ready: Arc<ReadyPromise>,
    result: Arc<ResultPromise>,
    service: Weak<AgentLoopService>,
}

impl TurnHandleContract for TurnHandleImpl {
    fn id(&self) -> i64 {
        self.id
    }

    fn state(&self) -> Option<TurnState> {
        Some(self.mutable.lock().unwrap().state)
    }

    fn signal(&self) -> AbortSignal {
        self.controller.signal()
    }

    fn ready(&self) -> TurnReadyFuture {
        self.ready.future()
    }

    fn result(&self) -> TurnResultFuture {
        self.result.future()
    }

    fn cancel(&self, reason: Option<LoopValue>) -> bool {
        self.service
            .upgrade()
            .is_some_and(|service| service.cancel(Some(self.id), reason))
    }
}

struct TurnJob {
    request: Arc<dyn StepRequest>,
    seed: TurnSeed,
    controller: AbortController,
    ready: Arc<ReadyPromise>,
    result: Arc<ResultPromise>,
    queue: Mutex<StepRequestQueue>,
    steps: Mutex<HashMap<String, Arc<StepHandleImpl>>>,
    turn_impl: Arc<TurnHandleImpl>,
    turn: TurnHandle,
}

#[derive(Default)]
struct LoopState {
    standalone_step_queue: StepRequestQueue,
    pending_assignments: HashMap<String, Arc<AssignmentPromise>>,
    error_handlers: Vec<Arc<dyn LoopErrorHandler>>,
    pending_turns: VecDeque<Arc<TurnJob>>,
    active_turn_job: Option<Arc<TurnJob>>,
    next_reserved_turn_id: Option<i64>,
    settle_waiters: Vec<oneshot::Sender<()>>,
    active_request_trace: Option<LlmRequestTrace>,
    last_request_trace_id: Option<String>,
    disposing: bool,
}

pub struct AgentLoopService {
    context: Arc<dyn AgentContextMemoryServiceContract>,
    llm_requester: Arc<dyn AgentLlmRequesterServiceContract>,
    event_bus: Arc<dyn EventBusContract>,
    tool_executor: Arc<dyn AgentToolExecutorServiceContract>,
    config: Arc<dyn ConfigServiceContract>,
    wire: WireServiceHandle,
    telemetry: Arc<dyn TelemetryServiceContract>,
    telemetry_context: Arc<dyn AgentTelemetryContextServiceContract>,
    hooks: AgentLoopHooks,
    state: Mutex<LoopState>,
    self_weak: Weak<AgentLoopService>,
}

impl AgentLoopService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: Arc<dyn AgentContextMemoryServiceContract>,
        llm_requester: Arc<dyn AgentLlmRequesterServiceContract>,
        event_bus: Arc<dyn EventBusContract>,
        tool_executor: Arc<dyn AgentToolExecutorServiceContract>,
        config: Arc<dyn ConfigServiceContract>,
        wire: WireServiceHandle,
        telemetry: Arc<dyn TelemetryServiceContract>,
        telemetry_context: Arc<dyn AgentTelemetryContextServiceContract>,
    ) -> Arc<Self> {
        ensure_turn_wire_registered();
        Arc::new_cyclic(|weak| Self {
            context,
            llm_requester,
            event_bus,
            tool_executor,
            config,
            wire,
            telemetry,
            telemetry_context,
            hooks: AgentLoopHooks::default(),
            state: Mutex::new(LoopState::default()),
            self_weak: weak.clone(),
        })
    }

    fn create_and_queue_turn(&self, request: Arc<dyn StepRequest>) -> Result<(), LoopValue> {
        let seed = request.turn_seed().ok_or_else(|| {
            loop_error(BugIndicatingError::new(Some(&format!(
                "Step request \"{}\" cannot start a turn without turnSeed",
                request.kind()
            ))))
        })?;
        let job = self.create_pending_turn(request, seed);
        self.state.lock().unwrap().pending_turns.push_back(job);
        self.pump_turns();
        Ok(())
    }

    fn create_pending_turn(&self, request: Arc<dyn StepRequest>, seed: TurnSeed) -> Arc<TurnJob> {
        let id = self.reserve_turn_id();
        let controller = AbortController::new();
        let ready = Arc::new(ReadyPromise::new());
        let result = Arc::new(ResultPromise::new());
        let turn_impl = Arc::new(TurnHandleImpl {
            id,
            mutable: Mutex::new(TurnMutable {
                state: TurnState::Queued,
            }),
            controller: controller.clone(),
            ready: Arc::clone(&ready),
            result: Arc::clone(&result),
            service: self.self_weak.clone(),
        });
        let turn = TurnHandle(turn_impl.clone());
        let job = Arc::new(TurnJob {
            request: Arc::clone(&request),
            seed,
            controller,
            ready,
            result,
            queue: Mutex::new(StepRequestQueue::new()),
            steps: Mutex::new(HashMap::new()),
            turn_impl,
            turn,
        });
        self.assign_step(&job, request, None);
        let pending = self.state.lock().unwrap().standalone_step_queue.drain();
        for request in pending {
            if !request.aborted() {
                self.assign_step(&job, request, None);
            }
        }
        job
    }

    fn reserve_turn_id(&self) -> i64 {
        let model_next = self.wire.get_model(&TURN_MODEL).next_turn_id;
        let mut state = self.state.lock().unwrap();
        let id = model_next.max(state.next_reserved_turn_id.unwrap_or(model_next));
        state.next_reserved_turn_id = Some(id + 1);
        id
    }

    fn assign_step(
        &self,
        job: &Arc<TurnJob>,
        request: Arc<dyn StepRequest>,
        options: Option<StepEnqueueOptions>,
    ) -> StepHandle {
        let step = self.enqueue_step(job, Arc::clone(&request), options);
        if let Some(assignment) = self
            .state
            .lock()
            .unwrap()
            .pending_assignments
            .remove(request.id())
        {
            assignment.settle(Ok(StepAssignment {
                turn: job.turn.clone(),
                step: step.clone(),
            }));
        }
        step
    }

    fn enqueue_step(
        &self,
        job: &Arc<TurnJob>,
        request: Arc<dyn StepRequest>,
        options: Option<StepEnqueueOptions>,
    ) -> StepHandle {
        if let Some(existing) = job.steps.lock().unwrap().get(request.id()).cloned()
            && existing.state() != StepState::Cancelled
        {
            job.queue.lock().unwrap().enqueue(
                request,
                options.and_then(|value| value.at).unwrap_or_default(),
            );
            existing.mutable.lock().unwrap().state = StepState::Queued;
            return StepHandle(existing);
        }
        let step = Arc::new(StepHandleImpl {
            id: request.id().to_owned(),
            turn_id: job.turn.0.id(),
            request: Arc::clone(&request),
            mutable: Mutex::new(StepMutable {
                state: StepState::Queued,
                controller: AbortController::new(),
            }),
            result: Arc::new(Promise::new()),
        });
        job.steps
            .lock()
            .unwrap()
            .insert(request.id().to_owned(), Arc::clone(&step));
        job.queue.lock().unwrap().enqueue(
            request,
            options.and_then(|value| value.at).unwrap_or_default(),
        );
        StepHandle(step)
    }

    fn reject_assignment(&self, request: &Arc<dyn StepRequest>, reason: LoopValue) {
        if let Some(assignment) = self
            .state
            .lock()
            .unwrap()
            .pending_assignments
            .remove(request.id())
        {
            assignment.settle(Err(reason));
        }
    }

    fn abort_request(&self, request: &Arc<dyn StepRequest>, reason: Option<LoopValue>) -> bool {
        let jobs = {
            let state = self.state.lock().unwrap();
            state
                .active_turn_job
                .iter()
                .chain(state.pending_turns.iter())
                .cloned()
                .collect::<Vec<_>>()
        };
        for job in jobs {
            if job.turn.0.state() == Some(TurnState::Queued) && Arc::ptr_eq(&job.request, request) {
                return self.cancel(Some(job.turn.0.id()), reason);
            }
            if let Some(step) = job.steps.lock().unwrap().get(request.id()).cloned() {
                return step.cancel(reason);
            }
        }
        if !request.abort() {
            return false;
        }
        self.reject_assignment(request, reason.unwrap_or_else(user_cancellation_value));
        true
    }

    fn pump_turns(&self) {
        let job = {
            let mut state = self.state.lock().unwrap();
            if state.disposing || state.active_turn_job.is_some() {
                return;
            }
            let Some(job) = state.pending_turns.pop_front() else {
                Self::settle_waiters(&mut state);
                return;
            };
            state.active_turn_job = Some(Arc::clone(&job));
            job
        };
        let _ = self
            .wire
            .dispatch([prompt_turn(job.seed.clone()).expect("turn seed is serializable")]);
        job.turn_impl.mutable.lock().unwrap().state = TurnState::Running;
        self.event_bus.publish_typed(super::TurnStartedEvent {
            turn_id: job.turn.0.id(),
            origin: job.seed.origin.clone(),
            prompt: is_displayable_prompt_origin(&job.seed.origin)
                .then(|| turn_prompt_text(&job.seed.input))
                .flatten(),
        });
        let service = self.self_weak.upgrade().expect("loop service is alive");
        tokio::spawn(async move {
            let result = service.run_turn(Arc::clone(&job)).await;
            job.result.settle(result);
        });
    }

    async fn run_turn(self: Arc<Self>, job: Arc<TurnJob>) -> LoopRunResult {
        let started_at = Instant::now();
        let turn_id = job.turn.0.id();
        self.telemetry_context.set(AgentTelemetryContextPatch {
            turn_id: Some(Some(u64::try_from(turn_id).unwrap_or_default())),
            ..AgentTelemetryContextPatch::default()
        });
        let telemetry_context = self.telemetry_context.get();
        let turn_telemetry = self
            .telemetry
            .with_context(&telemetry_context.to_telemetry_properties());
        let thinking_effort = self
            .llm_requester
            .prepare_turn_config(turn_id)
            .map(|value| value.thinking_effort.to_string());
        let _ = turn_telemetry.track_event(&TelemetryTurnStartedEvent {
            turn_id: u64::try_from(turn_id).unwrap_or_default(),
            mode: telemetry_context.mode,
            provider_type: telemetry_context.provider_type.clone(),
            protocol: telemetry_context.protocol.clone(),
            thinking_effort: thinking_effort.clone(),
        });
        let ready = Arc::clone(&job.ready);
        let result = self
            .run(LoopRunOptions {
                turn_id,
                signal: Some(job.controller.signal()),
                on_started: Some(Arc::new(move |_| {
                    ready.settle(Ok(()));
                })),
            })
            .await;
        self.settle_turn_ready(&job, &result);
        self.release_active_turn(&job, &result);

        let duration_ms = started_at.elapsed().as_millis() as u64;
        let trace_id = {
            let state = self.state.lock().unwrap();
            match result {
                LoopRunResult::Completed { .. } => state.last_request_trace_id.clone(),
                _ => state
                    .active_request_trace
                    .as_ref()
                    .and_then(LlmRequestTrace::trace_id),
            }
        };
        let error = match &result {
            LoopRunResult::Failed { error, .. } => Some(error_payload(error)),
            _ => None,
        };
        self.event_bus.publish_typed(super::TurnEndedEvent {
            turn_id,
            reason: turn_end_reason(&result),
            error: error.clone(),
            duration_ms: Some(duration_ms as f64),
        });
        if let Some(error) = error {
            self.publish_error(error);
        }
        if !matches!(result, LoopRunResult::Completed { .. }) {
            let _ = turn_telemetry.track_event(&TurnInterruptedEvent {
                turn_id: u64::try_from(turn_id).unwrap_or_default(),
                at_step: result_steps(&result),
                mode: telemetry_context.mode,
                interrupt_reason: interrupt_reason_for(&result),
                provider_type: telemetry_context.provider_type.clone(),
                protocol: telemetry_context.protocol.clone(),
                thinking_effort: thinking_effort.clone(),
                trace_id: trace_id.clone(),
            });
        }
        let _ = turn_telemetry.track_event(&TelemetryTurnEndedEvent {
            turn_id: u64::try_from(turn_id).unwrap_or_default(),
            reason: telemetry_turn_end_reason(&result),
            duration_ms,
            mode: telemetry_context.mode,
            provider_type: telemetry_context.provider_type,
            protocol: telemetry_context.protocol,
            thinking_effort,
            trace_id,
        });
        {
            let mut state = self.state.lock().unwrap();
            state.active_request_trace = None;
            state.last_request_trace_id = None;
        }
        self.pump_turns();
        result
    }

    fn settle_turn_ready(&self, job: &TurnJob, result: &LoopRunResult) {
        let value = match result {
            LoopRunResult::Failed { error, .. } => Err(error.clone()),
            LoopRunResult::Cancelled { reason, .. } => Err(reason.clone()),
            LoopRunResult::Completed { .. } => Err(loop_error(Error2::new(
                crate::_base::errors::codes::CORE_INTERNAL,
                "Turn ended before first step",
            ))),
        };
        job.ready.settle(value);
    }

    fn release_active_turn(&self, job: &Arc<TurnJob>, result: &LoopRunResult) {
        job.turn_impl.mutable.lock().unwrap().state = match result {
            LoopRunResult::Completed { .. } => TurnState::Completed,
            LoopRunResult::Failed { .. } => TurnState::Failed,
            LoopRunResult::Cancelled { .. } => TurnState::Cancelled,
        };
        let cancellation = match result {
            LoopRunResult::Cancelled { reason, .. } => reason.clone(),
            _ => loop_error(abort_error(Some("Turn ended"))),
        };
        for step in job.steps.lock().unwrap().values() {
            if matches!(step.state(), StepState::Queued | StepState::Running) {
                step.cancel(Some(cancellation.clone()));
            }
        }
        let mut state = self.state.lock().unwrap();
        if state
            .active_turn_job
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, job))
        {
            state.active_turn_job = None;
        }
        Self::settle_waiters(&mut state);
    }

    fn settle_waiters(state: &mut LoopState) {
        if state.active_turn_job.is_some() || !state.pending_turns.is_empty() {
            return;
        }
        for waiter in state.settle_waiters.drain(..) {
            let _ = waiter.send(());
        }
    }

    fn cancel_active_turn(&self, turn_id: Option<i64>, cancellation: &LoopValue) -> bool {
        let job = self.state.lock().unwrap().active_turn_job.clone();
        let Some(job) = job else {
            return false;
        };
        if turn_id.is_some_and(|turn_id| turn_id != job.turn.0.id()) {
            return false;
        }
        let _ =
            self.wire.dispatch([cancel_turn(turn_id.map(|id| id as f64))
                .expect("turn cancellation is serializable")]);
        job.controller.abort(Some(abort_from_value(cancellation)));
        true
    }

    fn cancel_queued_turn(&self, turn_id: i64, cancellation: LoopValue) -> bool {
        let job = {
            let mut state = self.state.lock().unwrap();
            let Some(index) = state
                .pending_turns
                .iter()
                .position(|job| job.turn.0.id() == turn_id)
            else {
                return false;
            };
            state.pending_turns.remove(index).unwrap()
        };
        if job.turn.0.state() != Some(TurnState::Queued) {
            return false;
        }
        let _ = self.wire.dispatch([
            cancel_turn(Some(turn_id as f64)).expect("turn cancellation is serializable")
        ]);
        for step in job.steps.lock().unwrap().values() {
            step.cancel(Some(cancellation.clone()));
        }
        job.controller.abort(Some(abort_from_value(&cancellation)));
        job.turn_impl.mutable.lock().unwrap().state = TurnState::Cancelled;
        job.ready.settle(Err(cancellation.clone()));
        job.result.settle(LoopRunResult::Cancelled {
            steps: 0,
            reason: cancellation,
        });
        let mut state = self.state.lock().unwrap();
        Self::settle_waiters(&mut state);
        true
    }

    async fn run_loop(&self, options: LoopRunOptions) -> LoopRunResult {
        let job = self
            .state
            .lock()
            .unwrap()
            .active_turn_job
            .clone()
            .filter(|job| job.turn.0.id() == options.turn_id);
        let turn_signal = options
            .signal
            .clone()
            .unwrap_or_else(|| AbortController::new().signal());
        let mut runtime = LoopRuntime {
            turn_id: options.turn_id,
            turn_signal,
            job,
            steps: 0,
            last_stop_reason: None,
            current: None,
        };
        let result = loop {
            match self.begin_loop_step(&mut runtime) {
                Ok(BeginStep::Completed(result)) => break result,
                Ok(BeginStep::Step(step)) => {
                    runtime.current = Some(step);
                    let current = runtime.current.as_ref().unwrap();
                    let executed = self
                        .execute_loop_step(
                            runtime.turn_id,
                            current.signal.clone(),
                            current.number,
                            current.uuid.clone(),
                            options.on_started.clone(),
                        )
                        .await;
                    match executed {
                        Ok(result) => match self.complete_loop_step(&mut runtime, result) {
                            Ok(Some(result)) => break result,
                            Ok(None) => {}
                            Err(error) => {
                                if let Some(result) =
                                    self.handle_loop_step_error(&mut runtime, error).await
                                {
                                    break result;
                                }
                            }
                        },
                        Err(error) => {
                            if let Some(result) =
                                self.handle_loop_step_error(&mut runtime, error).await
                            {
                                break result;
                            }
                        }
                    }
                }
                Err(error) => {
                    if let Some(result) = self.handle_loop_step_error(&mut runtime, error).await {
                        break result;
                    }
                }
            }
        };
        self.with_runtime_queue(&runtime, |queue| queue.abort_turn_scoped());
        result
    }

    fn with_runtime_queue<T>(
        &self,
        runtime: &LoopRuntime,
        operation: impl FnOnce(&mut StepRequestQueue) -> T,
    ) -> T {
        if let Some(job) = &runtime.job {
            operation(&mut job.queue.lock().unwrap())
        } else {
            operation(&mut self.state.lock().unwrap().standalone_step_queue)
        }
    }

    fn begin_loop_step(&self, runtime: &mut LoopRuntime) -> Result<BeginStep, LoopValue> {
        runtime.current = None;
        runtime
            .turn_signal
            .throw_if_aborted()
            .map_err(|error| LoopValue::Error(error))?;
        if !self.with_runtime_queue(runtime, |queue| queue.has_pending_requests()) {
            return Ok(BeginStep::Completed(LoopRunResult::Completed {
                steps: runtime.steps,
                truncated: runtime.last_stop_reason == Some(FinishReason::Truncated),
            }));
        }
        let max_steps = self
            .config
            .get(LOOP_CONTROL_SECTION)
            .and_then(|value| serde_json::from_value::<LoopControl>(value).ok())
            .and_then(|value| value.max_steps_per_turn);
        if max_steps.is_some_and(|max| max > 0 && runtime.steps >= max) {
            return Err(loop_error(create_max_steps_exceeded_error(
                max_steps.unwrap() as f64,
                None,
            )));
        }
        let batch = self
            .with_runtime_queue(runtime, |queue| queue.take_next_batch())
            .expect("pending loop queue must yield a batch");
        let mutable_step = runtime
            .job
            .as_ref()
            .and_then(|job| job.steps.lock().unwrap().get(batch.driver.id()).cloned());
        let (signal, links) = if let Some(step) = &mutable_step {
            let step_signal = step.set_running();
            combined_signal(&runtime.turn_signal, &step_signal)
        } else {
            (runtime.turn_signal.clone(), Vec::new())
        };
        runtime.steps += 1;
        let step = StepRuntime {
            number: runtime.steps,
            uuid: uuid::Uuid::new_v4().to_string(),
            batch,
            mutable_step,
            signal,
            _links: links,
        };
        self.materialize_batch(&step.batch)?;
        Ok(BeginStep::Step(step))
    }

    fn complete_loop_step(
        &self,
        runtime: &mut LoopRuntime,
        result: StepExecutionResult,
    ) -> Result<Option<LoopRunResult>, LoopValue> {
        if let Some(step) = runtime
            .current
            .as_ref()
            .and_then(|current| current.mutable_step.as_ref())
        {
            step.complete();
        }
        runtime.current = None;
        runtime.last_stop_reason = Some(result.stop_reason);
        if result.stop_reason == FinishReason::Filtered {
            ensure_protocol_errors_registered();
            return Err(loop_error(Error2::with_options(
                PROVIDER_FILTERED,
                "Provider safety policy blocked the response.",
                Error2Options {
                    name: Some("ProviderFilteredError".into()),
                    details: Some(Map::from_iter([(
                        "finishReason".into(),
                        Value::String("filtered".into()),
                    )])),
                    cause: None,
                },
            )));
        }
        Ok(result.hook_stop_turn.then_some(LoopRunResult::Completed {
            steps: runtime.steps,
            truncated: result.stop_reason == FinishReason::Truncated,
        }))
    }

    async fn handle_loop_step_error(
        &self,
        runtime: &mut LoopRuntime,
        error: LoopValue,
    ) -> Option<LoopRunResult> {
        if let Some(result) = self.handle_loop_cancellation(runtime, &error) {
            return result;
        }
        match self.try_recover_loop_error(runtime, error.clone()).await {
            ErrorRecovery::Recovered => return None,
            ErrorRecovery::Unhandled => {}
            ErrorRecovery::Failed(handler_error) => {
                if let Some(result) = self.handle_loop_cancellation(runtime, &handler_error) {
                    return result;
                }
                return Some(self.fail_loop_step(runtime, handler_error));
            }
        }
        Some(self.fail_loop_step(runtime, error))
    }

    fn fail_loop_step(&self, runtime: &LoopRuntime, error: LoopValue) -> LoopRunResult {
        let reason = if loop_value_is_max_steps(&error) {
            "max_steps"
        } else {
            "error"
        };
        self.emit_step_interrupted(
            runtime.turn_id,
            runtime.current.as_ref().map(|step| step.number),
            reason,
            Some(error.to_string()),
        );
        LoopRunResult::Failed {
            error,
            steps: runtime.steps,
        }
    }

    fn handle_loop_cancellation(
        &self,
        runtime: &mut LoopRuntime,
        error: &LoopValue,
    ) -> Option<Option<LoopRunResult>> {
        let step = runtime
            .current
            .as_ref()
            .and_then(|current| current.mutable_step.as_ref());
        if !loop_value_is_abort(error)
            && !runtime.turn_signal.aborted()
            && !step.is_some_and(|step| step.signal().aborted())
        {
            return None;
        }
        let reason = runtime
            .turn_signal
            .reason()
            .map(|value| LoopValue::Error(value))
            .or_else(|| {
                step.and_then(|step| step.signal().reason())
                    .map(|value| LoopValue::Error(value))
            })
            .unwrap_or_else(|| error.clone());
        self.emit_step_interrupted(
            runtime.turn_id,
            runtime.current.as_ref().map(|step| step.number),
            "aborted",
            (!loop_value_is_user_cancel(&reason)).then(|| reason.to_string()),
        );
        if !runtime.turn_signal.aborted()
            && step.is_some_and(|step| step.state() == StepState::Cancelled)
        {
            runtime.current = None;
            return Some(None);
        }
        Some(Some(LoopRunResult::Cancelled {
            reason,
            steps: runtime.steps,
        }))
    }

    async fn try_recover_loop_error(
        &self,
        runtime: &mut LoopRuntime,
        error: LoopValue,
    ) -> ErrorRecovery {
        let current_step = runtime
            .current
            .as_ref()
            .and_then(|current| current.mutable_step.clone())
            .map(|step| StepHandle(step));
        let failed_driver = runtime
            .current
            .as_ref()
            .map(|current| Arc::clone(&current.batch.driver));
        let step = runtime.current.as_ref().map(|current| current.number);
        let step_id = runtime.current.as_ref().map(|current| current.uuid.clone());
        let job = runtime.job.clone();
        let service = self.self_weak.clone();
        let turn_signal = runtime.turn_signal.clone();
        let fallback_step = current_step.clone();
        let turn_id = runtime.turn_id;
        let retry = Arc::new(move |request: Arc<dyn StepRequest>, options| {
            if let Some(service) = service.upgrade() {
                if let Some(job) = job.as_ref() {
                    return service.enqueue_step(job, request, options);
                }
                service.state.lock().unwrap().standalone_step_queue.enqueue(
                    Arc::clone(&request),
                    options.and_then(|value| value.at).unwrap_or_default(),
                );
                return fallback_step.clone().unwrap_or_else(|| {
                    completed_step_handle(request, turn_id, turn_signal.clone())
                });
            }
            fallback_step
                .clone()
                .unwrap_or_else(|| completed_step_handle(request, turn_id, turn_signal.clone()))
        });
        let mut context = LoopErrorContext {
            current_step,
            turn_id: runtime.turn_id,
            step,
            step_id,
            signal: runtime.turn_signal.clone(),
            error,
            failed_driver,
            retry,
        };
        let handler = self
            .state
            .lock()
            .unwrap()
            .error_handlers
            .iter()
            .find(|handler| handler.matches(&context))
            .cloned();
        let Some(handler) = handler else {
            return ErrorRecovery::Unhandled;
        };
        match handler.handle(&mut context).await {
            Ok(Some(true)) => {
                runtime.current = None;
                ErrorRecovery::Recovered
            }
            Ok(_) => ErrorRecovery::Unhandled,
            Err(handler_error) => ErrorRecovery::Failed(handler_error),
        }
    }

    fn materialize_batch(&self, batch: &StepRequestBatch) -> Result<(), LoopValue> {
        self.materialize_request(&batch.driver)?;
        for request in &batch.merged {
            self.materialize_request(request)?;
        }
        Ok(())
    }

    fn materialize_request(&self, request: &Arc<dyn StepRequest>) -> Result<(), LoopValue> {
        if request.state() != StepRequestState::Pending {
            return Ok(());
        }
        request.on_will_materialize();
        let messages = request.resolve_context_messages();
        if !messages.is_empty() {
            self.context.append(messages).map_err(loop_error)?;
        }
        request.mark_materialized();
        Ok(())
    }

    async fn execute_loop_step(
        &self,
        turn_id: i64,
        signal: AbortSignal,
        current_step: u64,
        step_uuid: String,
        on_started: Option<Arc<dyn Fn(u64) + Send + Sync>>,
    ) -> Result<StepExecutionResult, LoopValue> {
        self.state.lock().unwrap().active_request_trace = None;
        let mut before = BeforeStepContext {
            turn_id,
            step: current_step,
            signal: signal.clone(),
        };
        self.hooks
            .on_will_begin_step
            .run(&mut before, None)
            .await
            .map_err(|error| LoopValue::Error(Arc::from(error)))?;
        self.begin_step(turn_id, &signal, current_step, &step_uuid)?;
        let started = Arc::new(Mutex::new(false));
        let mark_started: Arc<dyn Fn() + Send + Sync> = {
            let started = Arc::clone(&started);
            Arc::new(move || {
                let mut started = started.lock().unwrap();
                if !*started {
                    *started = true;
                    if let Some(callback) = &on_started {
                        callback(current_step);
                    }
                }
            })
        };
        let request = self.llm_requester.start(
            Some(AgentLlmRequestOverrides {
                source: Some(AgentLlmRequestSource::Turn {
                    turn_id: turn_id as f64,
                    step: Some(current_step as f64),
                    log_fields: None,
                }),
                ..AgentLlmRequestOverrides::default()
            }),
            Some(self.create_stream_part_handler(turn_id, Arc::clone(&mark_started))),
            Some(signal.clone()),
        );
        self.state.lock().unwrap().active_request_trace = Some(request.trace.clone());
        let response = request.result.await.map_err(LoopValue::Error)?;
        self.state.lock().unwrap().last_request_trace_id = request.trace.trace_id();
        self.append_response_content(turn_id, current_step, &step_uuid, &response)?;
        let finish_reason = self
            .execute_step_tools(
                turn_id,
                signal.clone(),
                current_step,
                &step_uuid,
                &response,
                request.trace,
            )
            .await?;
        self.finish_step(
            turn_id,
            &signal,
            current_step,
            &step_uuid,
            &response,
            finish_reason,
            mark_started,
        )?;
        let hook_stop_turn = self
            .run_after_step(turn_id, signal, current_step, response.usage, finish_reason)
            .await?;
        Ok(StepExecutionResult {
            stop_reason: finish_reason,
            hook_stop_turn,
        })
    }

    fn begin_step(
        &self,
        turn_id: i64,
        signal: &AbortSignal,
        current_step: u64,
        step_uuid: &str,
    ) -> Result<(), LoopValue> {
        signal
            .throw_if_aborted()
            .map_err(|error| LoopValue::Error(error))?;
        self.event_bus.publish_typed(super::TurnStepStartedEvent {
            turn_id,
            step: current_step,
            step_id: Some(step_uuid.into()),
        });
        self.context
            .append_loop_event(LoopRecordedEvent::StepBegin {
                uuid: step_uuid.into(),
                turn_id: Some(turn_id.to_string()),
                step: Some(current_step as f64),
            })
            .map_err(loop_error)
    }

    fn append_response_content(
        &self,
        turn_id: i64,
        current_step: u64,
        step_uuid: &str,
        response: &AgentLlmRequestFinish,
    ) -> Result<(), LoopValue> {
        for part in &response.message.content {
            self.context
                .append_loop_event(LoopRecordedEvent::ContentPart {
                    step_uuid: step_uuid.into(),
                    part: part.clone(),
                    uuid: Some(uuid::Uuid::new_v4().to_string()),
                    turn_id: Some(turn_id.to_string()),
                    step: Some(current_step as f64),
                })
                .map_err(loop_error)?;
        }
        Ok(())
    }

    async fn execute_step_tools(
        &self,
        turn_id: i64,
        signal: AbortSignal,
        current_step: u64,
        step_uuid: &str,
        response: &AgentLlmRequestFinish,
        trace: LlmRequestTrace,
    ) -> Result<FinishReason, LoopValue> {
        let mut finish_reason = response
            .provider_finish_reason
            .unwrap_or(FinishReason::Completed);
        if response.message.tool_calls.is_empty() {
            return Ok(if finish_reason == FinishReason::ToolCalls {
                FinishReason::Other
            } else {
                finish_reason
            });
        }
        let call_uuids = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        let calls_for_callback = Arc::clone(&call_uuids);
        let context = Arc::clone(&self.context);
        let step_uuid_owned = step_uuid.to_owned();
        let on_tool_call = Arc::new(move |call: ToolCallStartedPayload| {
            let uuid = uuid::Uuid::new_v4().to_string();
            calls_for_callback
                .lock()
                .unwrap()
                .insert(call.tool_call_id.clone(), uuid.clone());
            let _ = context.append_loop_event(LoopRecordedEvent::ToolCall {
                step_uuid: step_uuid_owned.clone(),
                tool_call_id: call.tool_call_id,
                name: call.name,
                args: Some(call.args),
                extras: None,
                uuid: Some(uuid),
                turn_id: Some(turn_id.to_string()),
                step: Some(current_step as f64),
            });
        });
        let mut stream = self.tool_executor.execute(
            response.message.tool_calls.clone(),
            ToolExecutorExecuteOptions {
                signal,
                turn_id,
                trace: Some(trace),
                on_tool_call: Some(on_tool_call),
            },
        );
        let mut stop_turn = false;
        while let Some(item) = stream.next().await {
            let tool_result = item.map_err(|error| LoopValue::Error(Arc::from(error)))?;
            let result = tool_result.result;
            let output = match result.output {
                ExecutableToolOutput::Text(text) => LoopToolResultOutput::Text(text),
                ExecutableToolOutput::Content(parts) => LoopToolResultOutput::Parts(parts),
            };
            self.context
                .append_loop_event(LoopRecordedEvent::ToolResult {
                    tool_call_id: tool_result.tool_call_id.clone(),
                    result: LoopToolResult {
                        output,
                        is_error: Some(result.is_error),
                        note: result.note,
                    },
                    parent_uuid: Some(
                        call_uuids
                            .lock()
                            .unwrap()
                            .get(&tool_result.tool_call_id)
                            .cloned()
                            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    ),
                })
                .map_err(loop_error)?;
            stop_turn |= result.stop_turn == Some(true);
        }
        finish_reason = if stop_turn {
            FinishReason::Completed
        } else {
            FinishReason::ToolCalls
        };
        Ok(finish_reason)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_step(
        &self,
        turn_id: i64,
        signal: &AbortSignal,
        current_step: u64,
        step_uuid: &str,
        response: &AgentLlmRequestFinish,
        finish_reason: FinishReason,
        mark_started: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), LoopValue> {
        signal
            .throw_if_aborted()
            .map_err(|error| LoopValue::Error(error))?;
        mark_started();
        let timing = response.timing.as_ref();
        let normalized = normalize_finish_reason(finish_reason).to_owned();
        self.context
            .append_loop_event(LoopRecordedEvent::StepEnd {
                uuid: step_uuid.into(),
                turn_id: Some(turn_id.to_string()),
                step: Some(current_step as f64),
                finish_reason: Some(normalized.clone()),
                usage: Some(response.usage),
                llm_first_token_latency_ms: timing.map(|value| value.first_token_latency_ms),
                llm_stream_duration_ms: timing.map(|value| value.stream_duration_ms),
                llm_request_build_ms: timing.and_then(|value| value.request_build_ms),
                llm_server_first_token_ms: timing.and_then(|value| value.server_first_token_ms),
                llm_server_decode_ms: timing.and_then(|value| value.server_decode_ms),
                llm_client_consume_ms: timing.and_then(|value| value.client_consume_ms),
                message_id: response.provider_message_id.clone(),
                provider_finish_reason: response.provider_finish_reason,
                raw_finish_reason: response.raw_finish_reason.clone(),
            })
            .map_err(loop_error)?;
        self.event_bus.publish_typed(super::TurnStepCompletedEvent {
            turn_id,
            step: current_step,
            step_id: Some(step_uuid.into()),
            usage: Some(response.usage),
            finish_reason: Some(normalized),
            llm_first_token_latency_ms: timing.map(|value| value.first_token_latency_ms),
            llm_stream_duration_ms: timing.map(|value| value.stream_duration_ms),
            llm_request_build_ms: timing.and_then(|value| value.request_build_ms),
            llm_server_first_token_ms: timing.and_then(|value| value.server_first_token_ms),
            llm_server_decode_ms: timing.and_then(|value| value.server_decode_ms),
            llm_client_consume_ms: timing.and_then(|value| value.client_consume_ms),
            provider_finish_reason: response.provider_finish_reason,
            raw_finish_reason: response.raw_finish_reason.clone(),
        });
        Ok(())
    }

    async fn run_after_step(
        &self,
        turn_id: i64,
        signal: AbortSignal,
        current_step: u64,
        usage: TokenUsage,
        finish_reason: FinishReason,
    ) -> Result<bool, LoopValue> {
        let mut context = AfterStepContext {
            turn_id,
            step: current_step,
            signal: signal.clone(),
            usage,
            finish_reason,
            stop_turn: false,
        };
        if let Err(error) = self.hooks.on_did_finish_step.run(&mut context, None).await
            && (is_abort_error(error.as_ref()) || signal.aborted())
        {
            return Err(LoopValue::Error(Arc::from(error)));
        }
        Ok(context.stop_turn)
    }

    fn emit_step_interrupted(
        &self,
        turn_id: i64,
        step: Option<u64>,
        reason: &str,
        message: Option<String>,
    ) {
        let Some(step) = step else {
            return;
        };
        self.event_bus
            .publish_typed(super::TurnStepInterruptedEvent {
                turn_id,
                step,
                step_id: None,
                reason: reason.into(),
                message,
            });
    }

    fn create_stream_part_handler(
        &self,
        turn_id: i64,
        on_response_event: Arc<dyn Fn() + Send + Sync>,
    ) -> AgentLlmRequestPartHandler {
        let event_bus = Arc::clone(&self.event_bus);
        let calls = Arc::new(Mutex::new(
            HashMap::<Option<StreamIndex>, (String, String)>::new(),
        ));
        Arc::new(move |part| {
            let event_bus = Arc::clone(&event_bus);
            let calls = Arc::clone(&calls);
            let on_response_event = Arc::clone(&on_response_event);
            async move {
                match part {
                    StreamedMessagePart::Content(ContentPart::Text { text }) => {
                        on_response_event();
                        event_bus.publish_typed(super::AssistantDeltaEvent {
                            turn_id,
                            delta: text,
                        });
                    }
                    StreamedMessagePart::Content(ContentPart::Think { think, .. }) => {
                        on_response_event();
                        event_bus.publish_typed(super::ThinkingDeltaEvent {
                            turn_id,
                            delta: think,
                        });
                    }
                    StreamedMessagePart::Content(content) => {
                        on_response_event();
                        event_bus.publish_typed(super::AssistantContentEvent { turn_id, content });
                    }
                    StreamedMessagePart::ToolCall(call) => {
                        on_response_event();
                        calls
                            .lock()
                            .unwrap()
                            .insert(call.stream_index, (call.id.clone(), call.name.clone()));
                        event_bus.publish_typed(super::ToolCallDeltaEvent {
                            turn_id,
                            tool_call_id: call.id,
                            name: Some(call.name),
                            arguments_part: call.arguments,
                        });
                    }
                    StreamedMessagePart::ToolCallPart(part) => {
                        let Some(arguments) = part.arguments_part else {
                            return Ok(());
                        };
                        let call = calls.lock().unwrap().get(&part.index).cloned();
                        if let Some((id, name)) = call {
                            on_response_event();
                            event_bus.publish_typed(super::ToolCallDeltaEvent {
                                turn_id,
                                tool_call_id: id,
                                name: Some(name),
                                arguments_part: Some(arguments),
                            });
                        }
                    }
                }
                Ok(())
            }
            .boxed()
        })
    }

    fn publish_error(&self, error: KimiErrorPayload) {
        if let Ok(Value::Object(fields)) = serde_json::to_value(error) {
            self.event_bus.publish(DomainEvent::new("error", fields));
        }
    }

    fn delete_error_handler(&self, id: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(index) = state
            .error_handlers
            .iter()
            .position(|handler| handler.id() == id)
        else {
            return false;
        };
        state.error_handlers.remove(index);
        true
    }
}

#[async_trait]
impl AgentLoopServiceContract for AgentLoopService {
    fn enqueue(
        &self,
        request: Arc<dyn StepRequest>,
        options: Option<StepEnqueueOptions>,
    ) -> Result<EnqueueReceipt, LoopValue> {
        let assignment = Arc::new(AssignmentPromise::new());
        {
            let mut state = self.state.lock().unwrap();
            if state.disposing {
                return Err(loop_error(abort_error(Some("Agent loop disposed"))));
            }
            state
                .pending_assignments
                .insert(request.id().to_owned(), Arc::clone(&assignment));
        }
        let active = self.state.lock().unwrap().active_turn_job.clone();
        let admission_result = match request.admission() {
            StepRequestAdmission::NewTurn => self.create_and_queue_turn(Arc::clone(&request)),
            StepRequestAdmission::ActiveOrNewTurn => {
                if let Some(active) = active {
                    self.assign_step(&active, Arc::clone(&request), options);
                    Ok(())
                } else {
                    self.create_and_queue_turn(Arc::clone(&request))
                }
            }
            StepRequestAdmission::ActiveOrNextTurn => {
                if let Some(active) = active {
                    self.assign_step(&active, Arc::clone(&request), options);
                } else {
                    self.state.lock().unwrap().standalone_step_queue.enqueue(
                        Arc::clone(&request),
                        options.and_then(|value| value.at).unwrap_or_default(),
                    );
                }
                Ok(())
            }
            StepRequestAdmission::ActiveTurnOnly => {
                if let Some(active) = active {
                    self.assign_step(&active, Arc::clone(&request), options);
                    Ok(())
                } else {
                    Err(loop_error(BugIndicatingError::new(Some(&format!(
                        "Step request \"{}\" requires an active turn",
                        request.kind()
                    )))))
                }
            }
        };
        if let Err(error) = admission_result {
            self.reject_assignment(&request, error.clone());
            return Err(error);
        }
        let service = self.self_weak.clone();
        let request_for_abort = Arc::clone(&request);
        Ok(EnqueueReceipt::new(
            assignment.future() as StepAssignmentFuture,
            Arc::new(move |reason| {
                service
                    .upgrade()
                    .is_some_and(|service| service.abort_request(&request_for_abort, reason))
            }),
        ))
    }

    async fn run(&self, options: LoopRunOptions) -> LoopRunResult {
        self.run_loop(options).await
    }

    fn status(&self) -> AgentLoopStatus {
        let state = self.state.lock().unwrap();
        AgentLoopStatus {
            state: if state.active_turn_job.is_some() {
                AgentLoopState::Running
            } else {
                AgentLoopState::Idle
            },
            active_turn_id: state.active_turn_job.as_ref().map(|job| job.turn.0.id()),
            pending_turn_ids: state
                .pending_turns
                .iter()
                .map(|job| job.turn.0.id())
                .collect(),
            has_pending_requests: state
                .active_turn_job
                .as_ref()
                .is_some_and(|job| job.queue.lock().unwrap().has_pending_requests())
                || state.standalone_step_queue.has_pending_requests()
                || !state.pending_turns.is_empty(),
            active_trace_id: state
                .active_request_trace
                .as_ref()
                .and_then(LlmRequestTrace::trace_id),
        }
    }

    fn cancel(&self, turn_id: Option<i64>, reason: Option<LoopValue>) -> bool {
        let cancellation = reason.unwrap_or_else(user_cancellation_value);
        self.cancel_active_turn(turn_id, &cancellation)
            || turn_id.is_some_and(|id| self.cancel_queued_turn(id, cancellation))
    }

    async fn settled(&self) {
        let receiver = {
            let mut state = self.state.lock().unwrap();
            if state.active_turn_job.is_none() && state.pending_turns.is_empty() {
                return;
            }
            let (sender, receiver) = oneshot::channel();
            state.settle_waiters.push(sender);
            receiver
        };
        let _ = receiver.await;
    }

    fn has_pending_requests(&self) -> bool {
        self.status().has_pending_requests
    }

    fn register_loop_error_handler(
        &self,
        handler: Arc<dyn LoopErrorHandler>,
        options: LoopErrorHandlerRegistrationOptions<'_>,
    ) -> Result<DisposableHandle, LoopValue> {
        if options.before.is_some() && options.after.is_some() {
            return Err(string_error(
                "Loop error handler registration cannot specify both before and after",
            ));
        }
        self.delete_error_handler(handler.id());
        {
            let mut state = self.state.lock().unwrap();
            let target = options.before.or(options.after);
            let insert_at = if let Some(target) = target {
                let index = state
                    .error_handlers
                    .iter()
                    .position(|entry| entry.id() == target)
                    .ok_or_else(|| {
                        string_error(format!(
                            "Loop error handler target \"{target}\" is not registered"
                        ))
                    })?;
                index + usize::from(options.after.is_some())
            } else {
                state.error_handlers.len()
            };
            state.error_handlers.insert(insert_at, handler.clone());
        }
        let service = self.self_weak.clone();
        let id = handler.id().to_owned();
        Ok(to_disposable(move || {
            if let Some(service) = service.upgrade() {
                service.delete_error_handler(&id);
            }
        }))
    }

    fn hooks(&self) -> &AgentLoopHooks {
        &self.hooks
    }

    fn dispose(&self) -> DisposeResult {
        Disposable::dispose(self)
    }
}

impl Disposable for AgentLoopService {
    fn dispose(&self) -> DisposeResult {
        let (turn_ids, active, standalone) = {
            let mut state = self.state.lock().unwrap();
            if state.disposing {
                return Ok(());
            }
            state.disposing = true;
            (
                state
                    .pending_turns
                    .iter()
                    .map(|job| job.turn.0.id())
                    .collect::<Vec<_>>(),
                state.active_turn_job.clone(),
                state.standalone_step_queue.drain(),
            )
        };
        let reason = loop_error(abort_error(Some("Agent loop disposed")));
        for id in turn_ids {
            self.cancel(Some(id), Some(reason.clone()));
        }
        if let Some(active) = active {
            active.turn.0.cancel(Some(reason.clone()));
        }
        for request in standalone {
            request.abort();
            self.reject_assignment(&request, reason.clone());
        }
        let mut state = self.state.lock().unwrap();
        Self::settle_waiters(&mut state);
        Ok(())
    }
}

struct LoopRuntime {
    turn_id: i64,
    turn_signal: AbortSignal,
    job: Option<Arc<TurnJob>>,
    steps: u64,
    last_stop_reason: Option<FinishReason>,
    current: Option<StepRuntime>,
}

struct StepRuntime {
    number: u64,
    uuid: String,
    batch: StepRequestBatch,
    mutable_step: Option<Arc<StepHandleImpl>>,
    signal: AbortSignal,
    _links: Vec<AbortLink>,
}

enum BeginStep {
    Step(StepRuntime),
    Completed(LoopRunResult),
}

struct StepExecutionResult {
    stop_reason: FinishReason,
    hook_stop_turn: bool,
}

enum ErrorRecovery {
    Recovered,
    Unhandled,
    Failed(LoopValue),
}

fn combined_signal(first: &AbortSignal, second: &AbortSignal) -> (AbortSignal, Vec<AbortLink>) {
    let controller = AbortController::new();
    let signal = controller.signal();
    let links = vec![
        link_abort_signal(first, controller.clone()),
        link_abort_signal(second, controller),
    ];
    (signal, links)
}

fn completed_step_handle(
    request: Arc<dyn StepRequest>,
    turn_id: i64,
    _signal: AbortSignal,
) -> StepHandle {
    let promise = Arc::new(Promise::new());
    promise.settle(StepResult::Completed);
    StepHandle(Arc::new(StepHandleImpl {
        id: request.id().to_owned(),
        turn_id,
        request,
        mutable: Mutex::new(StepMutable {
            state: StepState::Queued,
            controller: AbortController::new(),
        }),
        result: promise,
    }))
}

fn loop_error(error: impl Error + Send + Sync + 'static) -> LoopValue {
    LoopValue::Error(Arc::new(error))
}

fn string_error(message: impl Into<String>) -> LoopValue {
    loop_error(std::io::Error::other(message.into()))
}

fn user_cancellation_value() -> LoopValue {
    loop_error(user_cancellation_reason())
}

fn abort_from_value(value: &LoopValue) -> AbortError {
    match value {
        LoopValue::Error(error) => error
            .downcast_ref::<AbortError>()
            .cloned()
            .unwrap_or_else(|| AbortError::new(error.to_string())),
        LoopValue::Value(value) => AbortError::new(value.to_string()),
    }
}

fn loop_value_is_abort(value: &LoopValue) -> bool {
    matches!(value, LoopValue::Error(error) if is_abort_error(error.as_ref()))
}

fn loop_value_is_user_cancel(value: &LoopValue) -> bool {
    matches!(value, LoopValue::Error(error) if is_user_cancellation(error.as_ref()))
}

fn loop_value_is_max_steps(value: &LoopValue) -> bool {
    matches!(value, LoopValue::Error(error) if is_max_steps_exceeded_error(error.as_ref()))
}

fn error_payload(value: &LoopValue) -> KimiErrorPayload {
    match value {
        LoopValue::Error(error) => to_error_payload(error.as_ref()),
        LoopValue::Value(value) => to_error_payload_value(value),
    }
}

fn result_steps(result: &LoopRunResult) -> u64 {
    match result {
        LoopRunResult::Completed { steps, .. }
        | LoopRunResult::Failed { steps, .. }
        | LoopRunResult::Cancelled { steps, .. } => *steps,
    }
}

fn turn_end_reason(result: &LoopRunResult) -> super::TurnEndReason {
    match result {
        LoopRunResult::Completed { .. } => super::TurnEndReason::Completed,
        LoopRunResult::Failed { .. } => super::TurnEndReason::Failed,
        LoopRunResult::Cancelled { .. } => super::TurnEndReason::Cancelled,
    }
}

fn telemetry_turn_end_reason(result: &LoopRunResult) -> TelemetryTurnEndReason {
    match result {
        LoopRunResult::Completed { .. } => TelemetryTurnEndReason::Completed,
        LoopRunResult::Failed { .. } => TelemetryTurnEndReason::Failed,
        LoopRunResult::Cancelled { .. } => TelemetryTurnEndReason::Cancelled,
    }
}

fn interrupt_reason_for(result: &LoopRunResult) -> TurnInterruptReason {
    match result {
        LoopRunResult::Cancelled { reason, .. } if loop_value_is_user_cancel(reason) => {
            TurnInterruptReason::UserCancelled
        }
        LoopRunResult::Cancelled { .. } => TurnInterruptReason::Aborted,
        LoopRunResult::Failed { error, .. } if loop_value_is_max_steps(error) => {
            TurnInterruptReason::MaxSteps
        }
        LoopRunResult::Failed {
            error: LoopValue::Error(error),
            ..
        } if error
            .downcast_ref::<Error2>()
            .is_some_and(|error| error.code == PROVIDER_FILTERED) =>
        {
            TurnInterruptReason::Filtered
        }
        LoopRunResult::Failed { .. } => TurnInterruptReason::Error,
        LoopRunResult::Completed { .. } => TurnInterruptReason::Error,
    }
}

fn normalize_finish_reason(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::ToolCalls => "tool_use",
        FinishReason::Completed => "end_turn",
        FinishReason::Truncated => "max_tokens",
        FinishReason::Filtered => "filtered",
        FinishReason::Paused => "paused",
        FinishReason::Other => "other",
    }
}

pub fn register_agent_loop_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_LOOP_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context: AgentContextMemoryServiceHandle =
                (*accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?).clone();
            let llm_requester: AgentLlmRequesterServiceHandle =
                (*accessor.get(AGENT_LLM_REQUESTER_SERVICE_ID)?).clone();
            let event_bus: EventBusHandle = (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone();
            let tool_executor: AgentToolExecutorServiceHandle =
                (*accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?).clone();
            let config: ConfigServiceHandle = (*accessor.get(CONFIG_SERVICE_ID)?).clone();
            let wire: WireServiceHandle = (*accessor.get(WIRE_SERVICE_ID)?).clone();
            let telemetry: TelemetryServiceHandle = (*accessor.get(TELEMETRY_SERVICE_ID)?).clone();
            let telemetry_context: AgentTelemetryContextServiceHandle =
                (*accessor.get(AGENT_TELEMETRY_CONTEXT_SERVICE_ID)?).clone();
            let service: Arc<dyn AgentLoopServiceContract> = AgentLoopService::new(
                context.0,
                llm_requester.0,
                event_bus.0,
                tool_executor.0,
                config.0,
                wire,
                telemetry.0,
                telemetry_context.0,
            );
            Ok(AgentLoopServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "loop",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loop_promises_are_shared_and_settle_once() {
        let promise = Promise::new();
        let first = promise.future();
        let second = promise.future();
        assert!(promise.settle(7_u64));
        assert!(!promise.settle(8));
        assert_eq!(first.await, 7);
        assert_eq!(second.await, 7);
    }

    #[tokio::test]
    async fn combined_signal_aborts_from_either_parent() {
        let turn = AbortController::new();
        let step = AbortController::new();
        let (combined, _links) = combined_signal(&turn.signal(), &step.signal());
        assert!(!combined.aborted());
        step.abort(Some(AbortError::new("step cancelled")));
        let reason = combined.cancelled().await;
        assert_eq!(reason.to_string(), "step cancelled");

        let already_aborted = AbortController::new();
        already_aborted.abort(Some(AbortError::new("turn cancelled")));
        let other = AbortController::new();
        let (combined, _links) = combined_signal(&already_aborted.signal(), &other.signal());
        assert_eq!(
            combined.throw_if_aborted().unwrap_err().to_string(),
            "turn cancelled"
        );
    }

    #[test]
    fn finish_reason_normalization_matches_context_wire_format() {
        assert_eq!(normalize_finish_reason(FinishReason::ToolCalls), "tool_use");
        assert_eq!(normalize_finish_reason(FinishReason::Completed), "end_turn");
        assert_eq!(
            normalize_finish_reason(FinishReason::Truncated),
            "max_tokens"
        );
        assert_eq!(normalize_finish_reason(FinishReason::Filtered), "filtered");
    }
}
