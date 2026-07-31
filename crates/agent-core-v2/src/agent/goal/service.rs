//! Main-agent durable goal lifecycle service.
//!
//! Original: `packages/agent-core-v2/src/agent/goal/goalService.ts`.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex, OnceLock, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableHandle, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options},
        utils::abort::abort_error,
    },
    agent::{
        context_injector::{AGENT_CONTEXT_INJECTOR_SERVICE_ID, AgentContextInjectorServiceHandle},
        context_memory::{ContextAppendMessagePayload, ContextMessage, PromptOrigin},
        goal::{
            AGENT_GOAL_SERVICE_ID, AgentGoalServiceContract, AgentGoalServiceHandle, CLEAR_GOAL,
            CREATE_GOAL, CreateGoalInput, CreateGoalPayload, EmptyGoalPayload, FORK_GOAL,
            GOAL_ALREADY_EXISTS, GOAL_BUDGET_BLOCK_PREFIX, GOAL_MODEL, GOAL_NOT_FOUND,
            GOAL_NOT_RESUMABLE, GOAL_OBJECTIVE_EMPTY, GOAL_OBJECTIVE_TOO_LONG, GOAL_STATUS_INVALID,
            GOAL_UNSUPPORTED_AGENT, GoalActor, GoalBudgetLimits, GoalChange, GoalChangeKind,
            GoalChangeStats, GoalReasonInput, GoalServiceError, GoalServiceResult, GoalSnapshot,
            GoalState, GoalStatus, GoalToolResult, ResumeGoalInput, SetGoalBudgetLimitsInput,
            UPDATE_GOAL, UpdateGoalPayload, clear_goal, compute_budget_report, create_goal,
            ensure_goal_errors_registered, goal_budget_block_reason, has_step_budget_remaining,
            is_goal_mutation_tool, matches_goal, update_goal,
        },
        llm_requester::AgentLlmRequestSource,
        loop_::{
            AGENT_LOOP_SERVICE_ID, AfterStepContext, AgentLoopServiceHandle, AgentLoopState,
            BeforeStepContext, ContinuationStepRequest, LoopValue, MessageStepRequest,
            MessageStepRequestOptions, StepRequestAdmission, StepRequestOptions, TurnEndReason,
            TurnEndedEvent, TurnStartedEvent,
        },
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
        system_reminder::{AGENT_SYSTEM_REMINDER_SERVICE_ID, AgentSystemReminderServiceHandle},
        tool_executor::{
            AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle,
            AuthorizeToolExecutionResult, ToolBeforeExecuteContext, ToolDidExecuteContext,
        },
        usage::{AGENT_USAGE_SERVICE_ID, AgentUsageServiceHandle, UsageRecordedContext},
    },
    app::{
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle, TypedEventBusExt},
        telemetry::{
            GoalBudgetProperties, GoalBudgetSetEvent, GoalClearedEvent, GoalContinuedEvent,
            GoalCreatedEvent, GoalStatus as TelemetryGoalStatus, GoalStatusChangedEvent,
            TELEMETRY_SERVICE_ID, TelemetryGoalActor, TelemetryServiceEventExt,
            TelemetryServiceHandle,
        },
    },
    hooks::HookRegisterOptions,
    kosong::{
        contract::{
            message::{ContentPart, Message, Role},
            provider::FinishReason,
        },
        protocol::errors::{
            PROVIDER_API_ERROR, PROVIDER_AUTH_ERROR, PROVIDER_CONNECTION_ERROR, PROVIDER_FILTERED,
            PROVIDER_RATE_LIMIT,
        },
    },
    tool::ExecutableToolResult,
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        model::{ModelCrossReducer, ModelDef, ModelOptions, define_model},
    },
};

use super::{
    goal_deadline_scheduler::{GOAL_DEADLINE_SCHEDULER_ID, GoalDeadlineSchedulerHandle},
    injection::{GoalInjection, GoalReader},
};
use crate::agent::profile::{MODEL_CONFIG_INVALID, MODEL_NOT_CONFIGURED};

const MAX_GOAL_OBJECTIVE_LENGTH: usize = 4_000;
const MAX_GOAL_COMPLETION_CRITERION_LENGTH: usize = MAX_GOAL_OBJECTIVE_LENGTH;

const GOAL_CANCELLED_REMINDER: &str = "The user cancelled the current goal. Ignore earlier active-goal reminders for that goal. Handle the next user request normally unless the user starts or resumes a goal.";
const GOAL_FORK_CLEARED_REMINDER: &str = "This fork does not have a current goal. Ignore earlier active-goal reminders from the source session. Handle requests normally unless the user starts a new goal.";
const GOAL_FORK_CLEARED_REMINDER_NAME: &str = "goal_fork_cleared";
const GOAL_RATE_LIMIT_PAUSE_REASON: &str = "Paused after provider rate limit";
const GOAL_PROVIDER_CONNECTION_PAUSE_PREFIX: &str = "Paused after provider connection error";
const GOAL_PROVIDER_AUTH_PAUSE_PREFIX: &str = "Paused after provider authentication error";
const GOAL_PROVIDER_API_PAUSE_PREFIX: &str = "Paused after provider API error";
const GOAL_MODEL_CONFIG_PAUSE_PREFIX: &str = "Paused after model configuration error";
const GOAL_RUNTIME_PAUSE_PREFIX: &str = "Paused after runtime error";
const GOAL_CONTINUATION_FAILURE_PAUSE_PREFIX: &str = "Paused after goal continuation failure";
const GOAL_PROVIDER_FILTERED_PAUSE_REASON: &str = "Paused after provider safety policy block";
const LLM_NOT_SET_MESSAGE: &str = "LLM not set, send \"/login\" to login";
const GOAL_BUDGET_STOP_REMINDER_NAME: &str = "goal_budget_stop";
const GOAL_BUDGET_STOP_REMINDER: &str = "The goal's hard budget was reached and the goal is now blocked; the user can resume it with /goal resume. Stop immediately. Do not call any more tools: they will be rejected. Write a brief final status message summarizing the progress so far.";
const GOAL_BUDGET_TOOLS_REJECTED_MESSAGE: &str =
    "Goal budget exhausted; tool calls are rejected. Write your final message.";
const GOAL_STALE_TOOL_RESULT: &str =
    "Goal changed since this turn started; ignored stale goal tool call.";
const GOAL_CONTINUATION_PROMPT: &str = "Continue working toward the active goal. Keep the self-audit brief. Do not explore unrelated interpretations once the goal can be decided. If the objective is simple, already answered, impossible, unsafe, or contradictory, do not run another goal turn. Explain briefly if useful, then call UpdateGoal with `complete` or `blocked` in the same turn. Otherwise, weigh the objective and any completion criteria against the work done so far, choose one bounded, useful slice of work, and use the existing conversation context and your tools. Do not try to finish a broad goal in one turn unless the whole goal is genuinely small. Most goal turns should not call UpdateGoal: after completing a useful slice, if material work remains, end the turn normally without calling UpdateGoal so the runtime can continue the goal in the next turn. Call UpdateGoal with `complete` only when all required work is done, any stated validation has passed, and there is no useful next action. Completion audit: before calling `complete`, verify the current state against the actual objective and every explicit requirement. Treat weak or indirect evidence as not complete. Do not mark complete after only producing a plan, summary, first pass, or partial result. Do not mark complete merely because a budget is nearly exhausted or you want to stop. Blocked audit: do not call UpdateGoal with `blocked` the first time you hit a blocker. Use `blocked` only for a genuine impasse: an external condition, required user input, missing credentials or permissions, or a persistent technical failure. For those non-terminal blockers, the same blocking condition must repeat for at least 3 consecutive goal turns before you call `blocked`, counting the original/user-triggered turn and automatic continuations. If a previously blocked goal is resumed, treat the resumed run as a fresh blocked audit. Exception: if the objective itself is impossible, unsafe, or contradictory, call UpdateGoal with `blocked` in the same turn; do not run more goal turns just to satisfy the audit. Do not use `blocked` because the work is large, hard, slow, uncertain, incomplete, still needs validation, would benefit from clarification, or needs more goal turns. Once the 3-turn threshold is met and you cannot make meaningful progress without user input or an external-state change, call UpdateGoal with `blocked`; do not keep reporting the blocker while leaving the goal active. Do not ask the user for input unless a real blocker prevents progress.";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct GoalForkNoticeState {
    goal_present: bool,
    reminder_pending: bool,
}

static GOAL_FORK_NOTICE_MODEL: LazyLock<ModelDef<GoalForkNoticeState>> = LazyLock::new(|| {
    define_model(
        "goalForkNotice",
        GoalForkNoticeState::default,
        ModelOptions {
            blobs: None,
            reducers: vec![
                ModelCrossReducer::typed(
                    "goal.create",
                    |mut state: GoalForkNoticeState, _: &CreateGoalPayload| {
                        state.goal_present = true;
                        state
                    },
                ),
                ModelCrossReducer::typed(
                    "goal.clear",
                    |mut state: GoalForkNoticeState, _: &EmptyGoalPayload| {
                        state.goal_present = false;
                        state
                    },
                ),
                ModelCrossReducer::typed(
                    "forked",
                    |state: GoalForkNoticeState, _: &EmptyGoalPayload| GoalForkNoticeState {
                        goal_present: false,
                        reminder_pending: state.goal_present || state.reminder_pending,
                    },
                ),
                ModelCrossReducer::typed(
                    "context.append_message",
                    |mut state: GoalForkNoticeState, payload: &ContextAppendMessagePayload| {
                        if state.reminder_pending && is_goal_fork_cleared_reminder(&payload.message)
                        {
                            state.reminder_pending = false;
                        }
                        state
                    },
                ),
            ],
        },
    )
});

#[derive(Clone)]
struct PendingContinuation {
    id: u64,
    receipt: crate::agent::loop_::EnqueueReceipt,
    goal_id: String,
    turn_id: Option<i64>,
}

#[derive(Default)]
struct RuntimeState {
    live_turn_id: Option<i64>,
    goal_driven_turns: HashMap<i64, String>,
    counted_goal_turns: HashSet<i64>,
    goal_starter_turns: HashSet<i64>,
    goal_outcome_tool_result_turns: HashMap<i64, String>,
    goal_outcome_continuation_turns: HashSet<i64>,
    budget_grace_turns: HashSet<i64>,
    pending_continuation_goals: HashMap<i64, String>,
    goal_turn_targets: HashMap<i64, String>,
    exhausted_turn_budget_goals: HashMap<i64, String>,
    wall_clock_deadline: Option<DisposableHandle>,
    live_wall_clock_started_at: Option<f64>,
    pending_continuation: Option<PendingContinuation>,
    resume_continuation: Option<(i64, String)>,
    next_pending_id: u64,
}

pub struct AgentGoalService {
    wire: WireServiceHandle,
    event_bus: EventBusHandle,
    reminders: AgentSystemReminderServiceHandle,
    telemetry: TelemetryServiceHandle,
    loop_service: AgentLoopServiceHandle,
    config: ConfigServiceHandle,
    deadline_scheduler: GoalDeadlineSchedulerHandle,
    agent_context: AgentScopeContext,
    state: Mutex<RuntimeState>,
    self_weak: OnceLock<Weak<AgentGoalService>>,
    disposables: DisposableStore,
}

impl AgentGoalService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wire: WireServiceHandle,
        event_bus: EventBusHandle,
        reminders: AgentSystemReminderServiceHandle,
        telemetry: TelemetryServiceHandle,
        dynamic_injector: AgentContextInjectorServiceHandle,
        loop_service: AgentLoopServiceHandle,
        tool_executor: AgentToolExecutorServiceHandle,
        usage_service: AgentUsageServiceHandle,
        config: ConfigServiceHandle,
        deadline_scheduler: GoalDeadlineSchedulerHandle,
        agent_context: AgentScopeContext,
    ) -> GoalServiceResult<Arc<Self>> {
        ensure_goal_errors_registered();
        register_goal_wire_types();
        let service = Arc::new(Self {
            wire,
            event_bus,
            reminders,
            telemetry,
            loop_service,
            config,
            deadline_scheduler,
            agent_context,
            state: Mutex::new(RuntimeState::default()),
            self_weak: OnceLock::new(),
            disposables: DisposableStore::new(),
        });
        service
            .self_weak
            .set(Arc::downgrade(&service))
            .expect("goal service self weak is initialized once");
        if service.is_supported_agent() {
            service.install_hooks(dynamic_injector, tool_executor, usage_service)?;
        }
        Ok(service)
    }

    fn install_hooks(
        self: &Arc<Self>,
        dynamic_injector: AgentContextInjectorServiceHandle,
        tool_executor: AgentToolExecutorServiceHandle,
        usage_service: AgentUsageServiceHandle,
    ) -> GoalServiceResult<()> {
        let reader: Arc<dyn GoalReader> = Arc::new(GoalServiceReader(Arc::downgrade(self)));
        self.disposables.add(Arc::new(GoalInjection::new(
            reader,
            dynamic_injector.0.as_ref(),
        )));

        let weak = Arc::downgrade(self);
        self.disposables
            .add(self.wire.hooks().on_did_restore.register(
                "goal",
                Arc::new(move |context, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.normalize_after_replay().map_err(|error| {
                                Box::new(error)
                                    as crate::_base::lifecycle::lifecycle_machine::BoxError
                            })?;
                        }
                        next(context).await
                    })
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables
            .add(
                self.event_bus
                    .subscribe_typed::<TurnStartedEvent>(Arc::new(move |event| {
                        if let Some(service) = weak.upgrade() {
                            let _ = service.handle_turn_launched(event.turn_id, &event.origin);
                        }
                    })),
            );

        let weak = Arc::downgrade(self);
        self.disposables
            .add(usage_service.on_did_record().subscribe(move |context| {
                if let Some(service) = weak.upgrade() {
                    let _ = service.handle_usage_recorded(context);
                }
            }));

        let weak = Arc::downgrade(self);
        self.disposables
            .add(self.loop_service.hooks().on_will_begin_step.register(
                "goal-count-turn",
                Arc::new(move |context: &mut BeforeStepContext, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.handle_before_step(context).map_err(|error| {
                                Box::new(error)
                                    as crate::_base::lifecycle::lifecycle_machine::BoxError
                            })?;
                        }
                        next(context).await
                    }) as BoxFuture<'_, _>
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables
            .add(self.loop_service.hooks().on_did_finish_step.register(
                "goal-outcome-continuation",
                Arc::new(move |context: &mut AfterStepContext, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.handle_after_step(context).map_err(|error| {
                                Box::new(error)
                                    as crate::_base::lifecycle::lifecycle_machine::BoxError
                            })?;
                        }
                        next(context).await
                    }) as BoxFuture<'_, _>
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables
            .add(tool_executor.hooks().on_before_execute_tool.register(
                "goal-budget-reject",
                Arc::new(move |context: &mut ToolBeforeExecuteContext, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            if service.is_stale_goal_tool_call(context) {
                                context.decision = Some(AuthorizeToolExecutionResult {
                                    synthetic_result: Some(ExecutableToolResult::success(
                                        GOAL_STALE_TOOL_RESULT,
                                    )),
                                    ..AuthorizeToolExecutionResult::default()
                                });
                                return Ok(());
                            }
                            if service
                                .state
                                .lock()
                                .unwrap()
                                .budget_grace_turns
                                .contains(&context.turn_id)
                            {
                                context.decision = Some(AuthorizeToolExecutionResult {
                                    synthetic_result: Some(ExecutableToolResult::success(
                                        GOAL_BUDGET_TOOLS_REJECTED_MESSAGE,
                                    )),
                                    ..AuthorizeToolExecutionResult::default()
                                });
                                return Ok(());
                            }
                        }
                        next(context).await
                    }) as BoxFuture<'_, _>
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables
            .add(tool_executor.hooks().on_did_execute_tool.register(
                "goal-outcome-tool-result",
                Arc::new(move |context: &mut ToolDidExecuteContext, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            let goal_id = service.goal_turn_target(context.turn_id);
                            if let Some(goal_id) = goal_id
                                && is_terminal_update_goal_result(
                                    &context.tool_call.name,
                                    &context.args,
                                    &context.result,
                                )
                            {
                                service
                                    .state
                                    .lock()
                                    .unwrap()
                                    .goal_outcome_tool_result_turns
                                    .insert(context.turn_id, goal_id);
                            }
                        }
                        next(context).await
                    }) as BoxFuture<'_, _>
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables
            .add(
                self.event_bus
                    .subscribe_typed::<TurnEndedEvent>(Arc::new(move |event| {
                        let Some(service) = weak.upgrade() else {
                            return;
                        };
                        let goal_id = service.goal_turn_target(event.turn_id);
                        if let Err(error) = service.handle_turn_ended(event.clone()) {
                            service.settle_goal_after_continuation_failure(
                                &error.to_string(),
                                goal_id.as_deref(),
                            );
                        }
                    })),
            );
        Ok(())
    }

    fn is_supported_agent(&self) -> bool {
        self.agent_context.agent_id == "main"
    }

    fn assert_supported_agent(&self) -> GoalServiceResult<()> {
        if self.is_supported_agent() {
            return Ok(());
        }
        Err(coded_error_with_details(
            GOAL_UNSUPPORTED_AGENT,
            "Goals are only supported by the main agent",
            Map::from_iter([(
                "agentId".into(),
                Value::String(self.agent_context.agent_id.clone()),
            )]),
        ))
    }

    fn goal_state(&self) -> Option<GoalState> {
        self.wire.get_model(&GOAL_MODEL)
    }

    fn snapshot(&self, state: &GoalState) -> GoalSnapshot {
        let wall_clock_ms = self.live_wall_clock_ms(state);
        GoalSnapshot {
            goal_id: state.goal_id.clone(),
            objective: state.objective.clone(),
            completion_criterion: state.completion_criterion.clone(),
            status: state.status,
            turns_used: state.turns_used,
            tokens_used: state.tokens_used,
            wall_clock_ms,
            budget: compute_budget_report(state, wall_clock_ms),
            terminal_reason: state.terminal_reason.clone(),
        }
    }

    pub async fn pause_active_goal(
        &self,
        input: GoalReasonInput,
        actor: GoalActor,
    ) -> GoalServiceResult<Option<GoalSnapshot>> {
        self.assert_supported_agent()?;
        let Some(state) = self
            .goal_state()
            .filter(|state| state.status == GoalStatus::Active)
        else {
            return Ok(None);
        };
        self.apply_lifecycle(&state, GoalStatus::Paused, input.reason, actor, false, None)
            .map(Some)
    }

    pub async fn pause_on_interrupt(
        &self,
        input: GoalReasonInput,
    ) -> GoalServiceResult<Option<GoalSnapshot>> {
        self.pause_active_goal(input, GoalActor::User).await
    }

    pub async fn record_token_usage(
        &self,
        token_delta: f64,
    ) -> GoalServiceResult<Option<GoalSnapshot>> {
        self.assert_supported_agent()?;
        self.account_token_usage(token_delta, None)
    }

    pub async fn increment_turn(&self) -> GoalServiceResult<Option<GoalSnapshot>> {
        self.assert_supported_agent()?;
        self.increment_goal_turn(None)
    }

    fn validate_objective(&self, value: &str) -> GoalServiceResult<String> {
        let objective = value.trim().to_owned();
        if objective.is_empty() {
            return Err(coded_error(
                GOAL_OBJECTIVE_EMPTY,
                "Goal objective cannot be empty",
            ));
        }
        if objective.chars().count() > MAX_GOAL_OBJECTIVE_LENGTH {
            return Err(coded_error(
                GOAL_OBJECTIVE_TOO_LONG,
                format!("Goal objective cannot exceed {MAX_GOAL_OBJECTIVE_LENGTH} characters"),
            ));
        }
        Ok(objective)
    }

    fn prepare_for_goal_creation(&self, replace: bool) -> GoalServiceResult<()> {
        if self.goal_state().is_none() {
            return Ok(());
        }
        if !replace {
            return Err(coded_error(
                GOAL_ALREADY_EXISTS,
                "A goal already exists; use replace to start a new one",
            ));
        }
        self.clear_internal(GoalActor::System, true, true, false)
    }

    fn require_state(&self) -> GoalServiceResult<GoalState> {
        self.goal_state()
            .ok_or_else(|| coded_error(GOAL_NOT_FOUND, "No current goal"))
    }

    fn account_token_usage(
        &self,
        token_delta: f64,
        goal_id: Option<&str>,
    ) -> GoalServiceResult<Option<GoalSnapshot>> {
        let Some(state) = self
            .goal_state()
            .filter(|state| state.status == GoalStatus::Active && matches_goal(state, goal_id))
        else {
            return Ok(None);
        };
        self.wire.dispatch([update_goal(UpdateGoalPayload {
            tokens_used: Some(state.tokens_used + token_delta.max(0.0)),
            ..UpdateGoalPayload::default()
        })?])?;
        let next = self.require_state()?;
        Ok(self
            .block_if_budget_reached(&next)?
            .or_else(|| Some(self.snapshot(&next))))
    }

    fn increment_goal_turn(
        &self,
        goal_id: Option<&str>,
    ) -> GoalServiceResult<Option<GoalSnapshot>> {
        let Some(state) = self
            .goal_state()
            .filter(|state| state.status == GoalStatus::Active && matches_goal(state, goal_id))
        else {
            return Ok(None);
        };
        self.wire.dispatch([update_goal(UpdateGoalPayload {
            turns_used: Some(state.turns_used + 1.0),
            ..UpdateGoalPayload::default()
        })?])?;
        let next = self.require_state()?;
        let snapshot = self.snapshot(&next);
        self.emit_goal_updated(Some(snapshot.clone()), None);
        let _ = self.telemetry.track_event(&GoalContinuedEvent {
            turns_used: next.turns_used as u64,
        });
        Ok(Some(snapshot))
    }

    fn handle_turn_launched(&self, turn_id: i64, origin: &PromptOrigin) -> GoalServiceResult<()> {
        {
            let mut runtime = self.state.lock().unwrap();
            runtime.live_turn_id = Some(turn_id);
            runtime.goal_turn_targets.remove(&turn_id);
            runtime.exhausted_turn_budget_goals.remove(&turn_id);
        }
        let has_driven = self
            .state
            .lock()
            .unwrap()
            .goal_driven_turns
            .contains_key(&turn_id);
        if !has_driven {
            let state = self.goal_state();
            let continuation_goal_id = if is_goal_continuation_origin(origin) {
                self.state
                    .lock()
                    .unwrap()
                    .pending_continuation_goals
                    .get(&turn_id)
                    .cloned()
            } else {
                None
            };
            if let Some(continuation_goal_id) = continuation_goal_id.clone()
                && state
                    .as_ref()
                    .is_none_or(|state| state.goal_id != continuation_goal_id)
            {
                self.state
                    .lock()
                    .unwrap()
                    .goal_driven_turns
                    .insert(turn_id, continuation_goal_id);
            } else if let Some(state) = state
                && state.status == GoalStatus::Active
                && self.block_if_budget_reached(&state)?.is_none()
            {
                self.state
                    .lock()
                    .unwrap()
                    .goal_driven_turns
                    .insert(turn_id, state.goal_id);
            }
        }
        let mut runtime = self.state.lock().unwrap();
        runtime.pending_continuation_goals.remove(&turn_id);
        runtime.goal_outcome_tool_result_turns.remove(&turn_id);
        runtime.goal_outcome_continuation_turns.remove(&turn_id);
        Ok(())
    }

    fn adopt_starter_turn(&self, actor: GoalActor) {
        let Some(turn_id) = self.state.lock().unwrap().live_turn_id else {
            return;
        };
        let Some(state) = self
            .goal_state()
            .filter(|state| state.status == GoalStatus::Active)
        else {
            return;
        };
        let turn_budget_reached = self.snapshot(&state).budget.turn_budget_reached;
        let mut runtime = self.state.lock().unwrap();
        let goal_id = runtime.goal_driven_turns.get(&turn_id).cloned();
        if actor == GoalActor::Model {
            runtime
                .goal_turn_targets
                .insert(turn_id, state.goal_id.clone());
        }
        if turn_budget_reached {
            runtime
                .exhausted_turn_budget_goals
                .insert(turn_id, state.goal_id.clone());
        } else {
            runtime.exhausted_turn_budget_goals.remove(&turn_id);
        }
        if goal_id.is_none() {
            runtime.goal_driven_turns.insert(turn_id, state.goal_id);
            runtime.counted_goal_turns.insert(turn_id);
            runtime.goal_starter_turns.insert(turn_id);
        }
    }

    fn handle_before_step(&self, context: &BeforeStepContext) -> GoalServiceResult<()> {
        let goal_id = {
            let mut runtime = self.state.lock().unwrap();
            let goal_id = runtime.goal_driven_turns.get(&context.turn_id).cloned();
            if goal_id.is_none() || !runtime.counted_goal_turns.insert(context.turn_id) {
                return Ok(());
            }
            goal_id
        };
        self.increment_goal_turn(goal_id.as_deref())?;
        Ok(())
    }

    fn handle_usage_recorded(&self, context: &UsageRecordedContext) -> GoalServiceResult<()> {
        let Some(AgentLlmRequestSource::Turn { turn_id, .. }) = &context.source else {
            return Ok(());
        };
        let turn_id = *turn_id as i64;
        let goal_id = self
            .state
            .lock()
            .unwrap()
            .goal_driven_turns
            .get(&turn_id)
            .cloned();
        let Some(goal_id) = goal_id else {
            return Ok(());
        };
        self.account_token_usage(context.usage.output, Some(&goal_id))?;
        Ok(())
    }

    fn handle_after_step(&self, context: &mut AfterStepContext) -> GoalServiceResult<()> {
        if self.stop_after_budget_reached(context)? {
            return Ok(());
        }
        self.enqueue_goal_outcome_continuation(context)
    }

    fn stop_after_budget_reached(&self, context: &mut AfterStepContext) -> GoalServiceResult<bool> {
        let goal_id = self.goal_turn_target(context.turn_id);
        let state = self.goal_state();
        let budget = state.as_ref().map(|state| self.snapshot(state).budget);
        let turn_budget_blocks_current_turn = budget
            .as_ref()
            .is_some_and(|budget| budget.turn_budget_reached)
            && (self
                .state
                .lock()
                .unwrap()
                .exhausted_turn_budget_goals
                .get(&context.turn_id)
                == goal_id.as_ref()
                || state.as_ref().is_some_and(|state| {
                    state.status == GoalStatus::Blocked
                        && state
                            .terminal_reason
                            .as_deref()
                            .is_some_and(|reason| reason.starts_with(GOAL_BUDGET_BLOCK_PREFIX))
                }));
        let should_stop = match (&goal_id, &state, &budget) {
            (Some(goal_id), Some(state), Some(budget)) if state.goal_id == *goal_id => {
                budget.token_budget_reached
                    || budget.wall_clock_budget_reached
                    || turn_budget_blocks_current_turn
            }
            _ => false,
        };
        if !should_stop {
            return Ok(false);
        }
        let max_steps = self
            .config
            .get(crate::agent::loop_::LOOP_CONTROL_SECTION)
            .and_then(|value| {
                serde_json::from_value::<crate::agent::loop_::LoopControl>(value).ok()
            })
            .and_then(|control| control.max_steps_per_turn)
            .map(|value| value as f64);
        let mut runtime = self.state.lock().unwrap();
        if context.finish_reason == FinishReason::ToolCalls
            && !runtime.budget_grace_turns.contains(&context.turn_id)
            && has_step_budget_remaining(max_steps, context.step as f64)
        {
            runtime.budget_grace_turns.insert(context.turn_id);
            drop(runtime);
            self.reminders.append_system_reminder(
                GOAL_BUDGET_STOP_REMINDER,
                PromptOrigin::SystemTrigger {
                    name: GOAL_BUDGET_STOP_REMINDER_NAME.into(),
                },
            )?;
            return Ok(true);
        }
        context.stop_turn = true;
        Ok(true)
    }

    fn enqueue_goal_outcome_continuation(
        &self,
        context: &AfterStepContext,
    ) -> GoalServiceResult<()> {
        let goal_id = self.goal_turn_target(context.turn_id);
        let outcome_goal_id = {
            let mut runtime = self.state.lock().unwrap();
            if runtime
                .goal_outcome_continuation_turns
                .contains(&context.turn_id)
            {
                return Ok(());
            }
            runtime
                .goal_outcome_tool_result_turns
                .remove(&context.turn_id)
        };
        let Some(goal_id) = goal_id.filter(|goal_id| outcome_goal_id.as_ref() == Some(goal_id))
        else {
            return Ok(());
        };
        if self
            .goal_state()
            .as_ref()
            .is_some_and(|state| state.goal_id != goal_id)
        {
            return Ok(());
        }
        self.state
            .lock()
            .unwrap()
            .goal_outcome_continuation_turns
            .insert(context.turn_id);
        let max_steps = self
            .config
            .get(crate::agent::loop_::LOOP_CONTROL_SECTION)
            .and_then(|value| {
                serde_json::from_value::<crate::agent::loop_::LoopControl>(value).ok()
            })
            .and_then(|control| control.max_steps_per_turn)
            .map(|value| value as f64);
        if !has_step_budget_remaining(max_steps, context.step as f64) {
            return Ok(());
        }
        self.loop_service
            .enqueue(
                Arc::new(ContinuationStepRequest::new(
                    MessageStepRequestOptions::default(),
                )),
                None,
            )
            .map_err(GoalServiceError::Loop)?;
        Ok(())
    }

    fn handle_turn_ended(&self, event: TurnEndedEvent) -> GoalServiceResult<()> {
        let (goal_id, lifecycle_goal_id, starter_turn) = self.clear_turn_tracking(event.turn_id);
        let resume_continuation = {
            let mut runtime = self.state.lock().unwrap();
            let resume = runtime.resume_continuation.clone();
            if resume
                .as_ref()
                .is_some_and(|(turn_id, _)| *turn_id == event.turn_id)
            {
                runtime.resume_continuation = None;
            }
            resume
        };
        if let Some((turn_id, goal_id)) = resume_continuation
            && turn_id == event.turn_id
            && event.reason == TurnEndReason::Cancelled
        {
            let Some(state) = self
                .goal_state()
                .filter(|state| state.status == GoalStatus::Active && state.goal_id == goal_id)
            else {
                return Ok(());
            };
            if self.block_if_budget_reached(&state)?.is_none() {
                self.launch_continuation_turn(&goal_id)?;
            }
            return Ok(());
        }
        let (Some(goal_id), Some(lifecycle_goal_id)) = (goal_id, lifecycle_goal_id) else {
            return Ok(());
        };
        if matches!(
            event.reason,
            TurnEndReason::Blocked | TurnEndReason::Cancelled | TurnEndReason::Failed
        ) {
            self.settle_abnormal_turn(&event, &lifecycle_goal_id)?;
            return Ok(());
        }
        if starter_turn {
            self.increment_goal_turn(Some(&goal_id))?;
        }
        let Some(state) = self.goal_state().filter(|state| {
            state.status == GoalStatus::Active && state.goal_id == lifecycle_goal_id
        }) else {
            return Ok(());
        };
        if self.block_if_budget_reached(&state)?.is_none() {
            self.launch_continuation_turn(&lifecycle_goal_id)?;
        }
        Ok(())
    }

    fn clear_turn_tracking(&self, turn_id: i64) -> (Option<String>, Option<String>, bool) {
        let mut runtime = self.state.lock().unwrap();
        if runtime
            .pending_continuation
            .as_ref()
            .and_then(|pending| pending.turn_id)
            == Some(turn_id)
        {
            runtime.pending_continuation = None;
        }
        if runtime.live_turn_id == Some(turn_id) {
            runtime.live_turn_id = None;
        }
        let goal_id = runtime.goal_driven_turns.remove(&turn_id);
        let lifecycle_goal_id = runtime
            .goal_turn_targets
            .remove(&turn_id)
            .or_else(|| goal_id.clone());
        let starter_turn = runtime.goal_starter_turns.remove(&turn_id);
        runtime.counted_goal_turns.remove(&turn_id);
        runtime.goal_outcome_tool_result_turns.remove(&turn_id);
        runtime.goal_outcome_continuation_turns.remove(&turn_id);
        runtime.budget_grace_turns.remove(&turn_id);
        runtime.pending_continuation_goals.remove(&turn_id);
        runtime.exhausted_turn_budget_goals.remove(&turn_id);
        (goal_id, lifecycle_goal_id, starter_turn)
    }

    fn settle_abnormal_turn(
        &self,
        event: &TurnEndedEvent,
        goal_id: &str,
    ) -> GoalServiceResult<bool> {
        if !self.is_active_goal(goal_id) {
            return Ok(false);
        }
        match event.reason {
            TurnEndReason::Blocked => {
                let state = self.require_state()?;
                self.apply_lifecycle(
                    &state,
                    GoalStatus::Blocked,
                    Some("Blocked by UserPromptSubmit hook".into()),
                    GoalActor::Runtime,
                    true,
                    None,
                )?;
                Ok(true)
            }
            TurnEndReason::Cancelled => {
                let state = self.require_state()?;
                self.apply_lifecycle(
                    &state,
                    GoalStatus::Paused,
                    Some("Paused after interruption".into()),
                    GoalActor::User,
                    false,
                    None,
                )?;
                Ok(true)
            }
            TurnEndReason::Failed => {
                let reason = goal_failure_pause_reason(event.error.as_ref());
                let state = self.require_state()?;
                self.apply_lifecycle(
                    &state,
                    GoalStatus::Paused,
                    Some(reason),
                    GoalActor::Runtime,
                    false,
                    None,
                )?;
                Ok(true)
            }
            TurnEndReason::Completed => Ok(false),
        }
    }

    fn settle_goal_after_continuation_failure(&self, error: &str, goal_id: Option<&str>) {
        let Some(_) = goal_id.filter(|goal_id| self.is_active_goal(goal_id)) else {
            return;
        };
        let reason = pause_reason_with_message(GOAL_CONTINUATION_FAILURE_PAUSE_PREFIX, Some(error));
        if let Some(state) = self.goal_state() {
            let _ = self.apply_lifecycle(
                &state,
                GoalStatus::Paused,
                Some(reason),
                GoalActor::System,
                false,
                None,
            );
        }
    }

    fn launch_continuation_turn(&self, goal_id: &str) -> GoalServiceResult<()> {
        if !self.is_active_goal(goal_id)
            || self.state.lock().unwrap().pending_continuation.is_some()
        {
            return Ok(());
        }
        let message = ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text {
                    text: GOAL_CONTINUATION_PROMPT.into(),
                }],
                Vec::new(),
            ),
            id: None,
            provider_message_id: None,
            origin: Some(PromptOrigin::SystemTrigger {
                name: "goal_continuation".into(),
            }),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        };
        let request = MessageStepRequest::new(
            message,
            MessageStepRequestOptions {
                request: StepRequestOptions {
                    admission: Some(StepRequestAdmission::NewTurn),
                    ..StepRequestOptions::default()
                },
                kind: Some("goal_continuation".into()),
            },
        );
        let receipt = self
            .loop_service
            .enqueue(Arc::new(request), None)
            .map_err(GoalServiceError::Loop)?;
        let pending = {
            let mut runtime = self.state.lock().unwrap();
            runtime.next_pending_id += 1;
            let pending = PendingContinuation {
                id: runtime.next_pending_id,
                receipt,
                goal_id: goal_id.into(),
                turn_id: None,
            };
            runtime.pending_continuation = Some(pending.clone());
            pending
        };
        let weak = self
            .self_weak
            .get()
            .expect("goal service self weak is initialized")
            .clone();
        tokio::spawn(async move {
            let Ok(assignment) = pending.receipt.assigned.clone().await else {
                cleanup_pending(&weak, &pending);
                return;
            };
            if let Some(service) = weak.upgrade() {
                let mut runtime = service.state.lock().unwrap();
                if let Some(current) = runtime
                    .pending_continuation
                    .as_mut()
                    .filter(|current| current.id == pending.id)
                {
                    current.turn_id = Some(assignment.turn.id());
                }
                if !runtime
                    .goal_driven_turns
                    .contains_key(&assignment.turn.id())
                {
                    runtime
                        .pending_continuation_goals
                        .insert(assignment.turn.id(), pending.goal_id.clone());
                }
            }
            let _ = assignment.turn.result().await;
            let mut completed = pending.clone();
            completed.turn_id = Some(assignment.turn.id());
            cleanup_pending(&weak, &completed);
        });
        Ok(())
    }

    fn can_launch_continuation(&self) -> bool {
        let runtime = self.state.lock().unwrap();
        if runtime.live_turn_id.is_some() || runtime.pending_continuation.is_some() {
            return false;
        }
        drop(runtime);
        let status = self.loop_service.status();
        status.state == AgentLoopState::Idle && !status.has_pending_requests
    }

    fn is_active_goal(&self, goal_id: &str) -> bool {
        self.goal_state()
            .is_some_and(|state| state.status == GoalStatus::Active && state.goal_id == goal_id)
    }

    fn is_stale_goal_tool_call(&self, context: &ToolBeforeExecuteContext) -> bool {
        if !is_goal_mutation_tool(&context.tool_call.name) {
            return false;
        }
        let Some(goal_id) = self.goal_turn_target(context.turn_id) else {
            return false;
        };
        self.goal_state().as_ref().map(|state| &state.goal_id) != Some(&goal_id)
    }

    fn goal_turn_target(&self, turn_id: i64) -> Option<String> {
        let runtime = self.state.lock().unwrap();
        runtime
            .goal_turn_targets
            .get(&turn_id)
            .or_else(|| runtime.goal_driven_turns.get(&turn_id))
            .cloned()
    }

    fn cancel_pending_continuation(
        &self,
        preserve_live_continuation: bool,
        reason: Option<LoopValue>,
    ) {
        let pending = {
            let mut runtime = self.state.lock().unwrap();
            if preserve_live_continuation
                && runtime
                    .pending_continuation
                    .as_ref()
                    .and_then(|pending| pending.turn_id)
                    == runtime.live_turn_id
            {
                return;
            }
            runtime.pending_continuation.take()
        };
        let Some(pending) = pending else {
            return;
        };
        let aborted = pending.receipt.abort(reason.clone());
        if !aborted && let Some(turn_id) = pending.turn_id {
            self.loop_service.cancel(Some(turn_id), reason);
        }
    }

    fn normalize_after_replay(&self) -> GoalServiceResult<()> {
        self.append_fork_cleared_reminder()?;
        self.clear_wall_clock_deadline();
        self.state.lock().unwrap().live_wall_clock_started_at = None;
        let Some(state) = self.goal_state() else {
            return Ok(());
        };
        if state.status == GoalStatus::Complete {
            self.clear_internal(GoalActor::Runtime, false, false, false)?;
            return Ok(());
        }
        if state.status != GoalStatus::Active {
            return Ok(());
        }
        let reason = "Paused after agent resume".to_owned();
        self.wire.dispatch([update_goal(UpdateGoalPayload {
            status: Some(GoalStatus::Paused),
            reason: Some(reason),
            wall_clock_ms: Some(self.settle_wall_clock(&state)),
            actor: Some(GoalActor::Runtime),
            ..UpdateGoalPayload::default()
        })?])?;
        self.track_status_changed(&self.require_state()?, GoalActor::Runtime);
        Ok(())
    }

    fn append_fork_cleared_reminder(&self) -> GoalServiceResult<()> {
        if !self
            .wire
            .get_model(&GOAL_FORK_NOTICE_MODEL)
            .reminder_pending
        {
            return Ok(());
        }
        self.reminders.append_system_reminder(
            GOAL_FORK_CLEARED_REMINDER,
            PromptOrigin::SystemTrigger {
                name: GOAL_FORK_CLEARED_REMINDER_NAME.into(),
            },
        )?;
        Ok(())
    }

    fn clear_internal(
        &self,
        actor: GoalActor,
        emit: bool,
        track: bool,
        preserve_live_continuation: bool,
    ) -> GoalServiceResult<()> {
        if self.goal_state().is_none() {
            return Ok(());
        }
        self.state.lock().unwrap().resume_continuation = None;
        self.cancel_pending_continuation(preserve_live_continuation, None);
        self.clear_wall_clock_deadline();
        self.state.lock().unwrap().live_wall_clock_started_at = None;
        self.wire.dispatch([clear_goal()?])?;
        if emit {
            self.emit_goal_updated(None, None);
        }
        if track {
            let _ = self.telemetry.track_event(&GoalClearedEvent {
                actor: telemetry_actor(actor),
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_lifecycle(
        &self,
        state: &GoalState,
        status: GoalStatus,
        reason: Option<String>,
        actor: GoalActor,
        preserve_live_continuation: bool,
        cancellation_reason: Option<LoopValue>,
    ) -> GoalServiceResult<GoalSnapshot> {
        let wall_clock_ms = self.settle_wall_clock(state);
        let wall_clock_resumed_at = (status == GoalStatus::Active).then(epoch_millis);
        if status == GoalStatus::Active {
            self.state.lock().unwrap().live_wall_clock_started_at =
                Some(self.deadline_scheduler.now());
        } else if state.status == GoalStatus::Active {
            self.state.lock().unwrap().resume_continuation = None;
            self.cancel_pending_continuation(preserve_live_continuation, cancellation_reason);
            self.clear_wall_clock_deadline();
            self.state.lock().unwrap().live_wall_clock_started_at = None;
        }
        self.wire.dispatch([update_goal(UpdateGoalPayload {
            status: Some(status),
            reason: reason.clone(),
            wall_clock_ms: Some(wall_clock_ms),
            wall_clock_resumed_at,
            actor: Some(actor),
            ..UpdateGoalPayload::default()
        })?])?;
        let next = self.require_state()?;
        if status == GoalStatus::Active {
            self.adopt_starter_turn(actor);
            self.refresh_wall_clock_deadline(&next);
        }
        let snapshot = self.snapshot(&next);
        self.emit_goal_updated(
            Some(snapshot.clone()),
            Some(GoalChange {
                kind: GoalChangeKind::Lifecycle,
                status: Some(status),
                reason,
                stats: None,
                actor: Some(actor),
            }),
        );
        self.track_status_changed(&next, actor);
        Ok(snapshot)
    }

    fn track_status_changed(&self, state: &GoalState, actor: GoalActor) {
        let limits = state.budget_limits;
        let _ = self.telemetry.track_event(&GoalStatusChangedEvent {
            actor: telemetry_actor(actor),
            status: telemetry_status(state.status),
            turns_used: state.turns_used as u64,
            tokens_used: state.tokens_used as u64,
            wall_clock_ms: self.live_wall_clock_ms(state) as u64,
            budget: telemetry_budget(limits),
        });
    }

    fn emit_goal_updated(&self, snapshot: Option<GoalSnapshot>, change: Option<GoalChange>) {
        let mut fields = Map::from_iter([(
            "snapshot".into(),
            serde_json::to_value(snapshot).expect("goal snapshot serializes"),
        )]);
        if let Some(change) = change {
            fields.insert(
                "change".into(),
                serde_json::to_value(change).expect("goal change serializes"),
            );
        }
        self.event_bus
            .publish(DomainEvent::new("goal.updated", fields));
    }

    fn settle_wall_clock(&self, state: &GoalState) -> f64 {
        self.live_wall_clock_ms(state)
    }

    fn live_wall_clock_ms(&self, state: &GoalState) -> f64 {
        if state.status == GoalStatus::Active {
            if let Some(started_at) = self.state.lock().unwrap().live_wall_clock_started_at {
                return state.wall_clock_ms + (self.deadline_scheduler.now() - started_at).max(0.0);
            }
            if let Some(resumed_at) = state.wall_clock_resumed_at {
                return state.wall_clock_ms + (epoch_millis() - resumed_at).max(0.0);
            }
        }
        state.wall_clock_ms
    }

    fn stats_of(&self, state: &GoalState) -> GoalChangeStats {
        GoalChangeStats {
            turns_used: state.turns_used,
            tokens_used: state.tokens_used,
            wall_clock_ms: self.live_wall_clock_ms(state),
        }
    }

    fn block_if_budget_reached(
        &self,
        state: &GoalState,
    ) -> GoalServiceResult<Option<GoalSnapshot>> {
        if state.status != GoalStatus::Active {
            return Ok(None);
        }
        let Some(reason) = goal_budget_block_reason(&self.snapshot(state).budget) else {
            return Ok(None);
        };
        self.apply_lifecycle(
            state,
            GoalStatus::Blocked,
            Some(reason),
            GoalActor::Runtime,
            true,
            None,
        )
        .map(Some)
    }

    fn clear_wall_clock_deadline(&self) {
        if let Some(deadline) = self.state.lock().unwrap().wall_clock_deadline.take() {
            let _ = deadline.dispose();
        }
    }

    fn refresh_wall_clock_deadline(&self, state: &GoalState) {
        self.clear_wall_clock_deadline();
        let runtime = self.state.lock().unwrap();
        let started = runtime.live_wall_clock_started_at.is_some();
        drop(runtime);
        let Some(budget_ms) = state.budget_limits.wall_clock_budget_ms else {
            return;
        };
        if state.status != GoalStatus::Active || !started {
            return;
        }
        let remaining_ms = (budget_ms - self.live_wall_clock_ms(state)).max(0.0);
        let weak = self
            .self_weak
            .get()
            .expect("goal service self weak is initialized")
            .clone();
        let deadline = self.deadline_scheduler.schedule(
            remaining_ms,
            Arc::new(move || {
                if let Some(service) = weak.upgrade() {
                    service.handle_wall_clock_deadline();
                }
            }),
        );
        self.state.lock().unwrap().wall_clock_deadline = Some(deadline);
    }

    fn handle_wall_clock_deadline(&self) {
        self.clear_wall_clock_deadline();
        let Some(state) = self
            .goal_state()
            .filter(|state| state.status == GoalStatus::Active)
        else {
            return;
        };
        let Some(budget_ms) = state.budget_limits.wall_clock_budget_ms else {
            return;
        };
        if self.live_wall_clock_ms(&state) < budget_ms {
            self.refresh_wall_clock_deadline(&state);
            return;
        }
        let Some(reason) = goal_budget_block_reason(&self.snapshot(&state).budget) else {
            return;
        };
        let cancellation = LoopValue::Error(
            Arc::new(abort_error(Some(&reason))) as Arc<dyn std::error::Error + Send + Sync>
        );
        let (live_turn_id, pending_turn_id) = {
            let runtime = self.state.lock().unwrap();
            (
                runtime.live_turn_id,
                runtime
                    .pending_continuation
                    .as_ref()
                    .and_then(|pending| pending.turn_id),
            )
        };
        let _ = self.apply_lifecycle(
            &state,
            GoalStatus::Blocked,
            Some(reason),
            GoalActor::Runtime,
            false,
            Some(cancellation.clone()),
        );
        if live_turn_id.is_some() && live_turn_id != pending_turn_id {
            self.loop_service.cancel(live_turn_id, Some(cancellation));
        }
    }
}

#[async_trait]
impl AgentGoalServiceContract for AgentGoalService {
    fn get_goal(&self) -> GoalServiceResult<GoalToolResult> {
        self.assert_supported_agent()?;
        Ok(GoalToolResult {
            goal: self.goal_state().as_ref().map(|state| self.snapshot(state)),
        })
    }

    fn is_goal_tool_target(&self, turn_id: f64, goal_id: &str) -> GoalServiceResult<bool> {
        self.assert_supported_agent()?;
        Ok(self
            .state
            .lock()
            .unwrap()
            .goal_turn_targets
            .get(&(turn_id as i64))
            .is_some_and(|target| target == goal_id))
    }

    async fn create_goal(
        &self,
        input: CreateGoalInput,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot> {
        self.assert_supported_agent()?;
        let actor = actor.unwrap_or(GoalActor::User);
        let objective = self.validate_objective(&input.objective)?;
        let replace = input.replace == Some(true);
        self.prepare_for_goal_creation(replace)?;
        self.wire.dispatch([create_goal(CreateGoalPayload {
            goal_id: uuid::Uuid::new_v4().to_string(),
            objective,
            completion_criterion: normalize_completion_criterion(
                input.completion_criterion.as_deref(),
            ),
            wall_clock_resumed_at: Some(epoch_millis()),
            status: None,
            actor: None,
            budget_limits: None,
        })?])?;
        self.state.lock().unwrap().live_wall_clock_started_at = Some(self.deadline_scheduler.now());
        self.adopt_starter_turn(actor);
        let state = self.require_state()?;
        // `refresh_wall_clock_deadline` needs the service Arc in timer
        // callbacks. Creation cannot have a wall-clock budget yet, so no timer
        // can be armed here.
        let snapshot = self.snapshot(&state);
        self.emit_goal_updated(Some(snapshot.clone()), None);
        let _ = self.telemetry.track_event(&GoalCreatedEvent {
            actor: telemetry_actor(actor),
            replace,
        });
        Ok(snapshot)
    }

    async fn pause_goal(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot> {
        self.assert_supported_agent()?;
        let state = self.require_state()?;
        if state.status == GoalStatus::Paused {
            return Ok(self.snapshot(&state));
        }
        if state.status != GoalStatus::Active {
            return Err(coded_error(
                GOAL_STATUS_INVALID,
                format!(
                    "Cannot pause a goal in status \"{}\"",
                    status_name(state.status)
                ),
            ));
        }
        self.apply_lifecycle(
            &state,
            GoalStatus::Paused,
            input.unwrap_or_default().reason,
            actor.unwrap_or(GoalActor::User),
            false,
            None,
        )
    }

    async fn resume_goal(
        &self,
        input: Option<ResumeGoalInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot> {
        self.assert_supported_agent()?;
        let input = input.unwrap_or_default();
        let actor = actor.unwrap_or(GoalActor::User);
        let state = self.require_state()?;
        if state.status == GoalStatus::Active {
            return Ok(self.snapshot(&state));
        }
        if !matches!(state.status, GoalStatus::Paused | GoalStatus::Blocked) {
            return Err(coded_error(
                GOAL_NOT_RESUMABLE,
                format!(
                    "Cannot resume a goal in status \"{}\"",
                    status_name(state.status)
                ),
            ));
        }
        let continue_paused = actor == GoalActor::User
            && state.status == GoalStatus::Paused
            && input.continue_if_paused == Some(true);
        let should_continue = continue_paused
            || (actor == GoalActor::User
                && state.status == GoalStatus::Blocked
                && input.continue_if_blocked == Some(true));
        let snapshot =
            self.apply_lifecycle(&state, GoalStatus::Active, input.reason, actor, false, None)?;
        if !should_continue {
            return Ok(snapshot);
        }
        if let Some(blocked) = self.block_if_budget_reached(&self.require_state()?)? {
            return Ok(blocked);
        }
        if self.can_launch_continuation() {
            if let Err(error) = self.launch_continuation_turn(&state.goal_id) {
                self.settle_goal_after_continuation_failure(
                    &error.to_string(),
                    Some(&state.goal_id),
                );
                return Err(error);
            }
        } else if continue_paused {
            let mut runtime = self.state.lock().unwrap();
            if let Some(turn_id) = runtime.live_turn_id {
                runtime.resume_continuation = Some((turn_id, state.goal_id));
            }
        }
        Ok(snapshot)
    }

    async fn cancel_goal(
        &self,
        _input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot> {
        self.assert_supported_agent()?;
        let actor = actor.unwrap_or(GoalActor::User);
        let state = self.require_state()?;
        let snapshot = self.snapshot(&state);
        if state.status == GoalStatus::Active
            && let Some(turn_id) = self.state.lock().unwrap().live_turn_id
        {
            self.loop_service.cancel(Some(turn_id), None);
        }
        self.clear_internal(actor, true, true, false)?;
        if actor == GoalActor::User {
            self.reminders.append_system_reminder(
                GOAL_CANCELLED_REMINDER,
                PromptOrigin::SystemTrigger {
                    name: "goal_cancelled".into(),
                },
            )?;
        }
        Ok(snapshot)
    }

    async fn set_budget_limits(
        &self,
        input: SetGoalBudgetLimitsInput,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<GoalSnapshot> {
        self.assert_supported_agent()?;
        let actor = actor.unwrap_or(GoalActor::User);
        let state = self.require_state()?;
        let budget_limits = GoalBudgetLimits {
            token_budget: input
                .budget_limits
                .token_budget
                .or(state.budget_limits.token_budget),
            turn_budget: input
                .budget_limits
                .turn_budget
                .or(state.budget_limits.turn_budget),
            wall_clock_budget_ms: input
                .budget_limits
                .wall_clock_budget_ms
                .or(state.budget_limits.wall_clock_budget_ms),
        };
        self.wire.dispatch([update_goal(UpdateGoalPayload {
            budget_limits: Some(budget_limits),
            ..UpdateGoalPayload::default()
        })?])?;
        let next = self.require_state()?;
        let snapshot = self.snapshot(&next);
        self.emit_goal_updated(Some(snapshot.clone()), None);
        let _ = self.telemetry.track_event(&GoalBudgetSetEvent {
            actor: telemetry_actor(actor),
            budget: telemetry_budget(input.budget_limits),
        });
        if let Some(blocked) = self.block_if_budget_reached(&next)? {
            return Ok(blocked);
        }
        self.refresh_wall_clock_deadline(&next);
        Ok(snapshot)
    }

    async fn mark_complete(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<Option<GoalSnapshot>> {
        self.assert_supported_agent()?;
        let Some(state) = self
            .goal_state()
            .filter(|state| state.status == GoalStatus::Active)
        else {
            return Ok(None);
        };
        let actor = actor.unwrap_or(GoalActor::Model);
        let reason = input.unwrap_or_default().reason;
        self.wire.dispatch([update_goal(UpdateGoalPayload {
            status: Some(GoalStatus::Complete),
            reason: reason.clone(),
            wall_clock_ms: Some(self.settle_wall_clock(&state)),
            actor: Some(actor),
            ..UpdateGoalPayload::default()
        })?])?;
        let completed = self.require_state()?;
        let snapshot = self.snapshot(&completed);
        self.emit_goal_updated(
            Some(snapshot.clone()),
            Some(GoalChange {
                kind: GoalChangeKind::Completion,
                status: Some(GoalStatus::Complete),
                reason,
                stats: Some(self.stats_of(&completed)),
                actor: Some(actor),
            }),
        );
        self.track_status_changed(&completed, actor);
        self.clear_internal(actor, true, true, true)?;
        Ok(Some(snapshot))
    }

    async fn mark_blocked(
        &self,
        input: Option<GoalReasonInput>,
        actor: Option<GoalActor>,
    ) -> GoalServiceResult<Option<GoalSnapshot>> {
        self.assert_supported_agent()?;
        let Some(state) = self
            .goal_state()
            .filter(|state| state.status == GoalStatus::Active)
        else {
            return Ok(None);
        };
        self.apply_lifecycle(
            &state,
            GoalStatus::Blocked,
            input.unwrap_or_default().reason,
            actor.unwrap_or(GoalActor::Runtime),
            true,
            None,
        )
        .map(Some)
    }
}

impl Disposable for AgentGoalService {
    fn dispose(&self) -> DisposeResult {
        self.clear_wall_clock_deadline();
        self.cancel_pending_continuation(false, None);
        self.disposables.dispose()
    }
}

impl Drop for AgentGoalService {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

struct GoalServiceReader(Weak<AgentGoalService>);

impl GoalReader for GoalServiceReader {
    fn get_goal(&self) -> Option<GoalSnapshot> {
        let service = self.0.upgrade()?;
        service
            .goal_state()
            .as_ref()
            .map(|state| service.snapshot(state))
    }
}

pub fn register_agent_goal_service() {
    register_goal_wire_types();

    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_GOAL_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = AgentGoalService::new(
                (*accessor.get(WIRE_SERVICE_ID)?).clone(),
                (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_CONTEXT_INJECTOR_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_LOOP_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_USAGE_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
                (*accessor.get(GOAL_DEADLINE_SCHEDULER_ID)?).clone(),
                (*accessor.get(AGENT_SCOPE_CONTEXT_ID)?).clone(),
            )
            .map_err(|error| crate::_base::di::errors::DiError::Factory(error.to_string()))?;
            let contract: Arc<dyn AgentGoalServiceContract> = service;
            Ok(AgentGoalServiceHandle(contract))
        })
        .disposable(),
        InstantiationType::Eager,
        "goal",
    );
}

fn register_goal_wire_types() {
    // TypeScript registers the Goal descriptors and cross reducers as module
    // import side effects. Rust's LazyLock definitions must be forced before
    // WireService.restore() encounters records written by either runtime.
    LazyLock::force(&GOAL_MODEL);
    LazyLock::force(&CREATE_GOAL);
    LazyLock::force(&UPDATE_GOAL);
    LazyLock::force(&CLEAR_GOAL);
    LazyLock::force(&FORK_GOAL);
    LazyLock::force(&GOAL_FORK_NOTICE_MODEL);
}

fn cleanup_pending(service: &Weak<AgentGoalService>, pending: &PendingContinuation) {
    let Some(service) = service.upgrade() else {
        return;
    };
    let mut runtime = service.state.lock().unwrap();
    if let Some(turn_id) = pending.turn_id {
        runtime.pending_continuation_goals.remove(&turn_id);
    }
    if runtime
        .pending_continuation
        .as_ref()
        .is_some_and(|current| current.id == pending.id)
    {
        runtime.pending_continuation = None;
    }
}

fn is_goal_fork_cleared_reminder(message: &ContextMessage) -> bool {
    matches!(
        message.origin.as_ref(),
        Some(PromptOrigin::SystemTrigger { name })
            if name == GOAL_FORK_CLEARED_REMINDER_NAME
    )
}

fn is_goal_continuation_origin(origin: &PromptOrigin) -> bool {
    matches!(
        origin,
        PromptOrigin::SystemTrigger { name } if name == "goal_continuation"
    )
}

fn normalize_completion_criterion(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    Some(
        value
            .chars()
            .take(MAX_GOAL_COMPLETION_CRITERION_LENGTH)
            .collect(),
    )
}

fn is_terminal_update_goal_result(
    tool_name: &str,
    args: &Value,
    result: &ExecutableToolResult,
) -> bool {
    if tool_name != "UpdateGoal" || result.is_error || result.stop_turn != Some(true) {
        return false;
    }
    matches!(
        args.as_object()
            .and_then(|args| args.get("status"))
            .and_then(Value::as_str),
        Some("complete" | "blocked")
    )
}

fn goal_failure_pause_reason(
    error: Option<&crate::_base::errors::serialize::KimiErrorPayload>,
) -> String {
    let code = error.map(|error| error.code.as_str());
    let message = error.map(|error| error.message.as_str());
    match code {
        Some(PROVIDER_RATE_LIMIT) => GOAL_RATE_LIMIT_PAUSE_REASON.into(),
        Some(PROVIDER_CONNECTION_ERROR) => {
            pause_reason_with_message(GOAL_PROVIDER_CONNECTION_PAUSE_PREFIX, message)
        }
        Some(PROVIDER_AUTH_ERROR) => {
            pause_reason_with_message(GOAL_PROVIDER_AUTH_PAUSE_PREFIX, message)
        }
        Some(PROVIDER_FILTERED) => GOAL_PROVIDER_FILTERED_PAUSE_REASON.into(),
        Some(PROVIDER_API_ERROR) => {
            pause_reason_with_message(GOAL_PROVIDER_API_PAUSE_PREFIX, message)
        }
        Some(MODEL_NOT_CONFIGURED) => {
            pause_reason_with_message(GOAL_MODEL_CONFIG_PAUSE_PREFIX, Some(LLM_NOT_SET_MESSAGE))
        }
        Some(MODEL_CONFIG_INVALID) => {
            pause_reason_with_message(GOAL_MODEL_CONFIG_PAUSE_PREFIX, message)
        }
        _ => pause_reason_with_message(GOAL_RUNTIME_PAUSE_PREFIX, message),
    }
}

fn pause_reason_with_message(prefix: &str, message: Option<&str>) -> String {
    let message = message.map(str::trim).filter(|message| !message.is_empty());
    message.map_or_else(|| prefix.into(), |message| format!("{prefix}: {message}"))
}

fn coded_error(code: &str, message: impl Into<String>) -> GoalServiceError {
    ensure_goal_errors_registered();
    GoalServiceError::Coded(Box::new(Error2::new(code, message)))
}

fn coded_error_with_details(
    code: &str,
    message: impl Into<String>,
    details: Map<String, Value>,
) -> GoalServiceError {
    ensure_goal_errors_registered();
    GoalServiceError::Coded(Box::new(Error2::with_options(
        code,
        message,
        Error2Options {
            details: Some(details),
            ..Error2Options::default()
        },
    )))
}

fn epoch_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}

fn status_name(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Active => "active",
        GoalStatus::Paused => "paused",
        GoalStatus::Blocked => "blocked",
        GoalStatus::Complete => "complete",
    }
}

fn telemetry_actor(actor: GoalActor) -> TelemetryGoalActor {
    match actor {
        GoalActor::User => TelemetryGoalActor::User,
        GoalActor::Model => TelemetryGoalActor::Model,
        GoalActor::Runtime => TelemetryGoalActor::Runtime,
        GoalActor::System => TelemetryGoalActor::System,
    }
}

fn telemetry_status(status: GoalStatus) -> TelemetryGoalStatus {
    match status {
        GoalStatus::Active => TelemetryGoalStatus::Active,
        GoalStatus::Paused => TelemetryGoalStatus::Paused,
        GoalStatus::Blocked => TelemetryGoalStatus::Blocked,
        GoalStatus::Complete => TelemetryGoalStatus::Complete,
    }
}

fn telemetry_budget(limits: GoalBudgetLimits) -> GoalBudgetProperties {
    GoalBudgetProperties {
        has_token_budget: limits.token_budget.is_some(),
        has_turn_budget: limits.turn_budget.is_some(),
        has_wall_clock_budget: limits.wall_clock_budget_ms.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream;

    use super::*;
    use crate::{
        _base::{
            di::lifecycle::{DisposableHandle, disposable_none},
            event::Event,
        },
        agent::{
            context_memory::{
                AgentContextMemoryServiceContract, ContextCompactionInput, ContextCompactionResult,
                ContextMemoryServiceError, LoopRecordedEvent, UndoCut, compute_undo_cut,
            },
            loop_::{
                AgentLoopHooks, AgentLoopStatus, LoopErrorHandler,
                LoopErrorHandlerRegistrationOptions, LoopRunOptions, LoopRunResult,
                StepEnqueueOptions,
            },
        },
        app::{
            config::{
                ConfigChangedEvent, ConfigDiagnostic, ConfigInspectValue,
                ConfigSectionChangedEvent, ConfigServiceContract, ConfigServiceError, ConfigTarget,
                ResolvedConfig,
            },
            event::{event_bus::EventBusContract, event_bus_service::EventBusService},
            telemetry::noop_telemetry_service,
        },
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        tool::ExecutableToolOutput,
        wire::wire_service::{WireBlobService, WireService},
    };

    #[derive(Default)]
    struct MemoryLog(Mutex<Vec<Value>>);

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, value: Value, _: AppendLogOptions) {
            self.0.lock().unwrap().push(value);
        }

        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::iter(
                self.0.lock().unwrap().clone().into_iter().map(Ok),
            ))
        }

        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            records: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            *self.0.lock().unwrap() = records;
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

    #[derive(Default)]
    struct MemoryContext(Mutex<Vec<ContextMessage>>);

    impl AgentContextMemoryServiceContract for MemoryContext {
        fn get(&self) -> crate::agent::context_memory::ContextMemorySnapshot {
            self.0.lock().unwrap().clone().into()
        }

        fn append(&self, messages: Vec<ContextMessage>) -> Result<(), ContextMemoryServiceError> {
            self.0.lock().unwrap().extend(messages);
            Ok(())
        }

        fn append_loop_event(&self, _: LoopRecordedEvent) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), ContextMemoryServiceError> {
            self.0.lock().unwrap().clear();
            Ok(())
        }

        fn undo(&self, count: f64) -> Result<UndoCut, ContextMemoryServiceError> {
            Ok(compute_undo_cut(&self.get(), count))
        }

        fn apply_compaction(
            &self,
            input: ContextCompactionInput,
        ) -> Result<ContextCompactionResult, ContextMemoryServiceError> {
            Ok(ContextCompactionResult {
                summary: input.summary.clone(),
                context_summary: input.context_summary.unwrap_or(input.summary),
                compacted_count: input.compacted_count,
                tokens_before: input.tokens_before,
                tokens_after: input.tokens_after.unwrap_or(0.0),
                kept_user_message_count: input.kept_user_message_count.unwrap_or(0.0),
                kept_head_user_message_count: input.kept_head_user_message_count,
                dropped_count: input.dropped_count,
            })
        }
    }

    #[derive(Default)]
    struct StubLoop {
        hooks: AgentLoopHooks,
    }

    #[async_trait]
    impl crate::agent::loop_::AgentLoopServiceContract for StubLoop {
        fn enqueue(
            &self,
            _: Arc<dyn crate::agent::loop_::StepRequest>,
            _: Option<StepEnqueueOptions>,
        ) -> Result<crate::agent::loop_::EnqueueReceipt, LoopValue> {
            Err(Value::String("unexpected continuation".into()).into())
        }

        async fn run(&self, _: LoopRunOptions) -> LoopRunResult {
            LoopRunResult::Completed {
                steps: 0,
                truncated: false,
            }
        }

        fn status(&self) -> AgentLoopStatus {
            AgentLoopStatus {
                state: AgentLoopState::Idle,
                active_turn_id: None,
                pending_turn_ids: Vec::new(),
                has_pending_requests: false,
                active_trace_id: None,
            }
        }

        fn cancel(&self, _: Option<i64>, _: Option<LoopValue>) -> bool {
            true
        }

        async fn settled(&self) {}

        fn has_pending_requests(&self) -> bool {
            false
        }

        fn register_loop_error_handler(
            &self,
            _: Arc<dyn LoopErrorHandler>,
            _: LoopErrorHandlerRegistrationOptions<'_>,
        ) -> Result<DisposableHandle, LoopValue> {
            Ok(disposable_none())
        }

        fn hooks(&self) -> &AgentLoopHooks {
            &self.hooks
        }
    }

    #[derive(Default)]
    struct EmptyConfig;

    impl Disposable for EmptyConfig {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[async_trait]
    impl ConfigServiceContract for EmptyConfig {
        async fn ready(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }

        fn on_did_change_configuration(&self) -> Event<ConfigChangedEvent> {
            Event::none()
        }

        fn on_did_section_change(&self) -> Event<ConfigSectionChangedEvent> {
            Event::none()
        }

        fn get(&self, _: &str) -> Option<Value> {
            None
        }

        fn inspect(&self, _: &str) -> ConfigInspectValue {
            ConfigInspectValue::default()
        }

        fn get_all(&self) -> ResolvedConfig {
            Map::new()
        }

        async fn set(
            &self,
            _: &str,
            _: Option<Value>,
            _: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            Ok(())
        }

        async fn replace(
            &self,
            _: &str,
            _: Option<Value>,
            _: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            Ok(())
        }

        async fn reload(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }

        fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
            Vec::new()
        }
    }

    fn service_fixture() -> (
        Arc<AgentGoalService>,
        Arc<MemoryContext>,
        Arc<Mutex<Vec<DomainEvent>>>,
    ) {
        ensure_goal_errors_registered();
        LazyLock::force(&GOAL_MODEL);
        LazyLock::force(&GOAL_FORK_NOTICE_MODEL);
        let event_bus = Arc::new(EventBusService::new());
        let wire = WireServiceHandle(Arc::new(WireService::new(
            "goal-test",
            AppendLogStoreHandle(Arc::new(MemoryLog::default())),
            Arc::new(IdentityBlobs),
            event_bus.clone(),
        )));
        let context = Arc::new(MemoryContext::default());
        let reminders = AgentSystemReminderServiceHandle(Arc::new(
            crate::agent::system_reminder::AgentSystemReminderService::new(context.clone()),
        ));
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        let registration = event_bus.subscribe(Arc::new(move |event| {
            recorded.lock().unwrap().push(event.clone());
        }));
        let service = Arc::new(AgentGoalService {
            wire,
            event_bus: EventBusHandle(event_bus),
            reminders,
            telemetry: noop_telemetry_service(),
            loop_service: AgentLoopServiceHandle(Arc::new(StubLoop::default())),
            config: ConfigServiceHandle(Arc::new(EmptyConfig)),
            deadline_scheduler: GoalDeadlineSchedulerHandle(Arc::new(
                crate::agent::goal::GoalDeadlineSchedulerService,
            )),
            agent_context: crate::agent::scope_context::make_agent_scope_context(
                crate::agent::scope_context::AgentScopeContextInput {
                    agent_id: "main".into(),
                    agent_scope: "goal-test".into(),
                },
            ),
            state: Mutex::new(RuntimeState::default()),
            self_weak: OnceLock::new(),
            disposables: DisposableStore::new(),
        });
        service.self_weak.set(Arc::downgrade(&service)).unwrap();
        service.disposables.add(registration);
        (service, context, events)
    }

    #[test]
    fn runtime_registration_installs_all_goal_ops_before_restore() {
        register_goal_wire_types();

        for op_type in ["goal.create", "goal.update", "goal.clear", "forked"] {
            assert!(
                crate::wire::op::registered_op(op_type).is_some(),
                "{op_type} must be replayable before goal service instantiation"
            );
        }
    }

    #[tokio::test]
    async fn restore_replays_forked_to_clear_goal_and_schedule_notice() {
        register_goal_wire_types();
        let records = vec![
            Value::Object(
                crate::wire::record::create_wire_metadata_record_at(1).into_wire_record(),
            ),
            serde_json::json!({
                "type": "goal.create",
                "goalId": "source-goal",
                "objective": "finish source task",
                "time": 2
            }),
            serde_json::json!({
                "type": "forked",
                "time": 3
            }),
        ];
        let log = Arc::new(MemoryLog(Mutex::new(records)));
        let wire = WireService::new(
            "goal-fork-restore-test",
            AppendLogStoreHandle(log),
            Arc::new(IdentityBlobs),
            Arc::new(EventBusService::new()),
        );

        wire.restore().await.unwrap();

        assert_eq!(wire.get_model(&GOAL_MODEL), None);
        assert_eq!(
            wire.get_model(&GOAL_FORK_NOTICE_MODEL),
            GoalForkNoticeState {
                goal_present: false,
                reminder_pending: true,
            }
        );
    }

    #[test]
    fn pure_source_helpers_preserve_validation_and_failure_mapping() {
        assert_eq!(
            normalize_completion_criterion(Some("  done  ")).as_deref(),
            Some("done")
        );
        assert_eq!(
            normalize_completion_criterion(Some(&"x".repeat(4_001)))
                .unwrap()
                .chars()
                .count(),
            4_000
        );
        assert_eq!(normalize_completion_criterion(Some("   ")), None);
        assert_eq!(
            goal_failure_pause_reason(Some(&crate::_base::errors::serialize::make_error_payload(
                PROVIDER_RATE_LIMIT,
                "rate limited",
                None,
                None,
            ))),
            GOAL_RATE_LIMIT_PAUSE_REASON
        );
        assert_eq!(
            pause_reason_with_message("prefix", Some("  detail  ")),
            "prefix: detail"
        );
    }

    #[test]
    fn terminal_tool_detection_matches_only_successful_stopping_outcomes() {
        let mut result = ExecutableToolResult::success(ExecutableToolOutput::Text("done".into()));
        result.stop_turn = Some(true);
        assert!(is_terminal_update_goal_result(
            "UpdateGoal",
            &serde_json::json!({"status": "complete"}),
            &result
        ));
        assert!(!is_terminal_update_goal_result(
            "UpdateGoal",
            &serde_json::json!({"status": "active"}),
            &result
        ));
        result.is_error = true;
        assert!(!is_terminal_update_goal_result(
            "UpdateGoal",
            &serde_json::json!({"status": "blocked"}),
            &result
        ));
    }

    #[test]
    fn fork_notice_model_tracks_goal_and_consumes_only_its_own_reminder() {
        let initial = GOAL_FORK_NOTICE_MODEL.initial();
        assert_eq!(initial, GoalForkNoticeState::default());
        assert!(is_goal_fork_cleared_reminder(&ContextMessage {
            message: Message::new(Role::User, vec![], vec![]),
            id: None,
            provider_message_id: None,
            origin: Some(PromptOrigin::SystemTrigger {
                name: GOAL_FORK_CLEARED_REMINDER_NAME.into()
            }),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }));
    }

    #[tokio::test]
    async fn lifecycle_budget_completion_and_cancel_match_source_behavior() {
        let (service, context, events) = service_fixture();
        let created = service
            .create_goal(
                CreateGoalInput {
                    objective: "  ship feature  ".into(),
                    completion_criterion: Some(format!("  {}  ", "x".repeat(4_001))),
                    replace: None,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(created.objective, "ship feature");
        assert_eq!(
            created.completion_criterion.unwrap().chars().count(),
            MAX_GOAL_COMPLETION_CRITERION_LENGTH
        );
        assert!(matches!(
            service
                .create_goal(
                    CreateGoalInput {
                        objective: "duplicate".into(),
                        completion_criterion: None,
                        replace: None,
                    },
                    None,
                )
                .await,
            Err(GoalServiceError::Coded(error)) if error.code == GOAL_ALREADY_EXISTS
        ));

        assert_eq!(
            service
                .pause_goal(
                    Some(GoalReasonInput {
                        reason: Some("wait".into())
                    }),
                    None
                )
                .await
                .unwrap()
                .status,
            GoalStatus::Paused
        );
        service.resume_goal(None, None).await.unwrap();
        service.increment_turn().await.unwrap();
        let blocked = service
            .set_budget_limits(
                SetGoalBudgetLimitsInput {
                    budget_limits: GoalBudgetLimits {
                        turn_budget: Some(1.0),
                        ..GoalBudgetLimits::default()
                    },
                },
                Some(GoalActor::Model),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status, GoalStatus::Blocked);
        assert_eq!(
            blocked.terminal_reason.as_deref(),
            Some("Blocked after goal budget reached: turn budget 1")
        );
        service.resume_goal(None, None).await.unwrap();
        let completed = service
            .mark_complete(
                Some(GoalReasonInput {
                    reason: Some("done".into()),
                }),
                Some(GoalActor::Model),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, GoalStatus::Complete);
        assert!(service.get_goal().unwrap().goal.is_none());

        service
            .create_goal(
                CreateGoalInput {
                    objective: "another".into(),
                    completion_criterion: None,
                    replace: None,
                },
                None,
            )
            .await
            .unwrap();
        service.cancel_goal(None, None).await.unwrap();
        assert!(!context.get().iter().any(is_goal_fork_cleared_reminder));
        assert!(context.get().iter().any(|message| {
            matches!(
                message.origin.as_ref(),
                Some(PromptOrigin::SystemTrigger { name }) if name == "goal_cancelled"
            )
        }));
        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == "goal.updated" && event.fields.get("snapshot") == Some(&Value::Null)
        }));
    }

    #[tokio::test]
    async fn failed_continuation_enqueue_repauses_the_resumed_goal() {
        let (service, _, _) = service_fixture();
        service
            .create_goal(
                CreateGoalInput {
                    objective: "continue work".into(),
                    completion_criterion: None,
                    replace: None,
                },
                None,
            )
            .await
            .unwrap();
        service
            .mark_blocked(
                Some(GoalReasonInput {
                    reason: Some("waiting".into()),
                }),
                None,
            )
            .await
            .unwrap();

        let result = service
            .resume_goal(
                Some(ResumeGoalInput {
                    continue_if_blocked: Some(true),
                    ..ResumeGoalInput::default()
                }),
                None,
            )
            .await;
        assert!(matches!(result, Err(GoalServiceError::Loop(_))));
        let goal = service.get_goal().unwrap().goal.unwrap();
        assert_eq!(goal.status, GoalStatus::Paused);
        assert!(
            goal.terminal_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with(GOAL_CONTINUATION_FAILURE_PAUSE_PREFIX))
        );
    }

    #[test]
    fn registration_is_eager_agent_scoped_and_disposable() {
        register_agent_goal_service();
        let registration =
            crate::_base::di::scope::get_scoped_service_descriptors(LifecycleScope::Agent)
                .into_iter()
                .find(|entry| entry.id.to_string() == AGENT_GOAL_SERVICE_ID.to_string())
                .unwrap();
        assert_eq!(registration.scope, LifecycleScope::Agent);
        assert_eq!(registration.domain, "goal");
        assert!(!registration.descriptor.supports_delayed_instantiation);
    }
}
