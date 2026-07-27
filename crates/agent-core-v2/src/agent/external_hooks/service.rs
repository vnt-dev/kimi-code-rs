//! Agent-scoped adapter from agent lifecycle events to external hook commands.
//!
//! Original:
//! `packages/agent-core-v2/src/agent/externalHooks/externalHooksService.ts`.
//!
//! The shared hook engine remains App-scoped. This eager Agent-scoped service
//! only installs adapters, owns their registrations, and cancels detached
//! notification tasks when the agent scope is disposed.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::serialize::{KimiErrorPayload, to_error_payload_value},
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::{AbortController, AbortSignal},
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceHandle, ContextMessage,
            PromptOrigin,
        },
        full_compaction::{
            AGENT_FULL_COMPACTION_SERVICE_ID, AgentFullCompactionServiceHandle, CompactionResult,
            CompactionSource, FullCompactionTask,
        },
        loop_::{
            AGENT_LOOP_SERVICE_ID, AfterStepContext, AgentLoopServiceHandle,
            ContinuationStepRequest, MessageStepRequestOptions, StepRequestAdmission,
            StepRequestOptions, TurnEndReason, TurnEndedEvent,
        },
        permission_gate::service::AGENT_PERMISSION_GATE_ID,
        prompt::{AGENT_PROMPT_SERVICE_ID, AgentPromptServiceHandle, PromptSubmitContext},
        task::AGENT_TASK_SERVICE_ID,
        tool_executor::{
            AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle,
            AuthorizeToolExecutionResult, ToolBeforeExecuteContext, ToolDidExecuteContext,
        },
    },
    app::{
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle, TypedEventBusExt},
        external_hooks_runner::{
            EXTERNAL_HOOKS_RUNNER_SERVICE_ID, ExternalHooksRunnerServiceHandle,
            ExternalHooksRunnerTriggerArgs,
        },
    },
    hooks::{HookRegisterOptions, HookRegistrationError},
    kosong::contract::{
        message::{ContentPart, Message, Role},
        provider::FinishReason,
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
    tool::ExecutableToolOutput,
};

use super::{
    AGENT_EXTERNAL_HOOKS_SERVICE_ID, AgentExternalHooksServiceContract,
    AgentExternalHooksServiceHandle, HookMatcherValue, render_user_prompt_hook_block_result,
    render_user_prompt_hook_result,
};

const HOOK_REGISTRATION_ID: &str = "externalHooks";
const TOOL_OUTPUT_LIMIT: usize = 2_000;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookResultEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<i64>,
    pub hook_event: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
}

pub struct AgentExternalHooksService {
    runner: ExternalHooksRunnerServiceHandle,
    context: AgentContextMemoryServiceHandle,
    event_bus: EventBusHandle,
    session_context: SessionContext,
    stop_hook_continuation_used: AtomicBool,
    disposables: DisposableStore,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl AgentExternalHooksService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runner: ExternalHooksRunnerServiceHandle,
        context: AgentContextMemoryServiceHandle,
        event_bus: EventBusHandle,
        session_context: SessionContext,
        tool_executor: AgentToolExecutorServiceHandle,
        prompt: AgentPromptServiceHandle,
        loop_service: AgentLoopServiceHandle,
        full_compaction: AgentFullCompactionServiceHandle,
    ) -> Result<Arc<Self>, HookRegistrationError> {
        let service = Arc::new(Self {
            runner,
            context,
            event_bus,
            session_context,
            stop_hook_continuation_used: AtomicBool::new(false),
            disposables: DisposableStore::new(),
            tasks: Mutex::new(Vec::new()),
        });
        if let Err(error) =
            service.register_listeners(tool_executor, prompt, loop_service, full_compaction)
        {
            let _ = service.dispose();
            return Err(error);
        }
        Ok(service)
    }

    fn register_listeners(
        self: &Arc<Self>,
        tool_executor: AgentToolExecutorServiceHandle,
        prompt: AgentPromptServiceHandle,
        loop_service: AgentLoopServiceHandle,
        full_compaction: AgentFullCompactionServiceHandle,
    ) -> Result<(), HookRegistrationError> {
        self.register_tool_hooks(tool_executor)?;
        self.register_permission_hooks();
        self.register_prompt_hooks(prompt)?;
        self.register_turn_hooks();
        self.register_loop_hooks(loop_service)?;
        self.register_full_compaction_hooks(full_compaction)?;
        self.register_task_hooks();
        Ok(())
    }

    fn register_tool_hooks(
        self: &Arc<Self>,
        tool_executor: AgentToolExecutorServiceHandle,
    ) -> Result<(), HookRegistrationError> {
        let weak = Arc::downgrade(self);
        self.disposables
            .add(tool_executor.hooks().on_before_execute_tool.register(
                HOOK_REGISTRATION_ID,
                Arc::new(move |context, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade()
                            && let Some(reason) = service.run_pre_tool_use(context).await?
                        {
                            context.decision = Some(AuthorizeToolExecutionResult {
                                block: Some(true),
                                reason: Some(reason),
                                ..AuthorizeToolExecutionResult::default()
                            });
                            return Ok(());
                        }
                        next(context).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables
            .add(tool_executor.hooks().on_did_execute_tool.register(
                HOOK_REGISTRATION_ID,
                Arc::new(move |context, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.notify_post_tool_use(context);
                        }
                        next(context).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);
        Ok(())
    }

    fn register_permission_hooks(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.disposables.add(self.event_bus.subscribe_type(
            "permission.approval.requested",
            Arc::new(move |event| {
                if let Some(service) = weak.upgrade() {
                    service.fire_and_forget(
                        "PermissionRequest",
                        event.fields().clone(),
                        string_field(&event.fields, "toolName").map(HookMatcherValue::String),
                        None,
                    );
                }
            }),
        ));

        let weak = Arc::downgrade(self);
        self.disposables.add(self.event_bus.subscribe_type(
            "permission.approval.resolved",
            Arc::new(move |event| {
                if let Some(service) = weak.upgrade() {
                    service.fire_and_forget(
                        "PermissionResult",
                        event.fields().clone(),
                        string_field(&event.fields, "toolName").map(HookMatcherValue::String),
                        None,
                    );
                }
            }),
        ));
    }

    fn register_prompt_hooks(
        self: &Arc<Self>,
        prompt: AgentPromptServiceHandle,
    ) -> Result<(), HookRegistrationError> {
        let weak = Arc::downgrade(self);
        self.disposables
            .add(prompt.hooks().on_before_submit_prompt.register(
                HOOK_REGISTRATION_ID,
                Arc::new(move |context, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade()
                            && service.run_prompt_submit_hook(context).await?
                        {
                            context.block = true;
                            return Ok(());
                        }
                        next(context).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);
        Ok(())
    }

    fn register_turn_hooks(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.disposables
            .add(
                self.event_bus
                    .subscribe_typed::<TurnEndedEvent>(Arc::new(move |event| {
                        if let Some(service) = weak.upgrade() {
                            service.notify_turn_ended(event);
                        }
                    })),
            );
    }

    fn register_loop_hooks(
        self: &Arc<Self>,
        loop_service: AgentLoopServiceHandle,
    ) -> Result<(), HookRegistrationError> {
        let weak = Arc::downgrade(self);
        let loop_for_hook = loop_service.clone();
        self.disposables
            .add(loop_service.hooks().on_did_finish_step.register(
                HOOK_REGISTRATION_ID,
                Arc::new(move |context, next| {
                    let weak = weak.clone();
                    let loop_service = loop_for_hook.clone();
                    Box::pin(async move {
                        next(context).await?;
                        if matches!(
                            context.finish_reason,
                            FinishReason::ToolCalls | FinishReason::Filtered
                        ) || loop_service.has_pending_requests()
                        {
                            return Ok(());
                        }
                        let Some(service) = weak.upgrade() else {
                            return Ok(());
                        };
                        let Some(reason) = service.run_stop(context).await? else {
                            return Ok(());
                        };
                        service
                            .stop_hook_continuation_used
                            .store(true, Ordering::Release);
                        service
                            .context
                            .append(vec![context_message(
                                Role::User,
                                reason,
                                PromptOrigin::SystemTrigger {
                                    name: "stop_hook".into(),
                                },
                            )])
                            .map_err(|error| Box::new(error) as BoxError)?;
                        loop_service
                            .enqueue(
                                Arc::new(ContinuationStepRequest::new(MessageStepRequestOptions {
                                    request: StepRequestOptions {
                                        mergeable: Some(true),
                                        admission: Some(StepRequestAdmission::ActiveOrNextTurn),
                                        ..StepRequestOptions::default()
                                    },
                                    kind: Some("stop_hook".into()),
                                })),
                                None,
                            )
                            .map_err(|error| Box::new(error) as BoxError)?;
                        Ok(())
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);
        Ok(())
    }

    fn register_full_compaction_hooks(
        self: &Arc<Self>,
        full_compaction: AgentFullCompactionServiceHandle,
    ) -> Result<(), HookRegistrationError> {
        let weak = Arc::downgrade(self);
        self.disposables
            .add(full_compaction.hooks().on_will_compact.register(
                HOOK_REGISTRATION_ID,
                Arc::new(move |context, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.run_pre_compact(context).await?;
                            service.watch_post_compact(context.clone());
                        }
                        next(context).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);
        Ok(())
    }

    fn register_task_hooks(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.disposables.add(self.event_bus.subscribe_type(
            "task.notified",
            Arc::new(move |event| {
                if let Some(service) = weak.upgrade() {
                    service.notify_task_notification(event);
                }
            }),
        ));
    }

    fn fire_and_forget(
        &self,
        event: &str,
        input_data: Map<String, Value>,
        matcher_value: Option<HookMatcherValue>,
        signal: Option<AbortSignal>,
    ) {
        let runner = self.runner.clone();
        let event = event.to_owned();
        let session_id = self.session_context.session_id.clone();
        let task = tokio::spawn(async move {
            runner
                .fire_and_forget_trigger(
                    &event,
                    ExternalHooksRunnerTriggerArgs {
                        matcher_value,
                        input_data: Some(input_data),
                        signal,
                        session_id: Some(session_id),
                        ..ExternalHooksRunnerTriggerArgs::default()
                    },
                )
                .await;
        });
        self.tasks.lock().unwrap().push(task);
    }

    async fn run_pre_tool_use(
        &self,
        context: &ToolBeforeExecuteContext,
    ) -> Result<Option<String>, BoxError> {
        throw_if_aborted(&context.signal)?;
        let tool_input = context.args.as_object().cloned().unwrap_or_default();
        let block = self
            .runner
            .trigger_block(
                "PreToolUse",
                ExternalHooksRunnerTriggerArgs {
                    matcher_value: Some(HookMatcherValue::String(context.tool_call.name.clone())),
                    signal: Some(context.signal.clone()),
                    session_id: Some(self.session_context.session_id.clone()),
                    input_data: Some(Map::from_iter([
                        (
                            "toolName".into(),
                            Value::String(context.tool_call.name.clone()),
                        ),
                        ("toolInput".into(), Value::Object(tool_input)),
                        (
                            "toolCallId".into(),
                            Value::String(context.tool_call.id.clone()),
                        ),
                    ])),
                    ..ExternalHooksRunnerTriggerArgs::default()
                },
            )
            .await;
        throw_if_aborted(&context.signal)?;
        Ok(block.map(|block| block.reason))
    }

    fn notify_post_tool_use(&self, context: &ToolDidExecuteContext) {
        let output = tool_output_text(&context.result.output);
        let is_error = context.result.is_error;
        let mut input_data = Map::from_iter([
            (
                "toolName".into(),
                Value::String(context.tool_call.name.clone()),
            ),
            (
                "toolInput".into(),
                Value::Object(context.args.as_object().cloned().unwrap_or_default()),
            ),
            (
                "toolCallId".into(),
                Value::String(context.tool_call.id.clone()),
            ),
        ]);
        if is_error {
            input_data.insert(
                "error".into(),
                serde_json::to_value(to_error_payload_value(&Value::String(output)))
                    .expect("KimiErrorPayload is serializable"),
            );
        } else {
            input_data.insert(
                "toolOutput".into(),
                Value::String(utf16_prefix_lossy(&output, TOOL_OUTPUT_LIMIT)),
            );
        }
        self.fire_and_forget(
            if is_error {
                "PostToolUseFailure"
            } else {
                "PostToolUse"
            },
            input_data,
            Some(HookMatcherValue::String(context.tool_call.name.clone())),
            Some(context.signal.clone()),
        );
    }

    async fn run_prompt_submit_hook(
        &self,
        context: &PromptSubmitContext,
    ) -> Result<bool, BoxError> {
        if !matches!(
            context
                .prompt_message
                .origin
                .as_ref()
                .unwrap_or(&PromptOrigin::User),
            PromptOrigin::User
        ) {
            return Ok(false);
        }

        let signal = AbortController::new().signal();
        let input = context.prompt_message.message.content.clone();
        throw_if_aborted(&signal)?;
        let results = self
            .runner
            .trigger(
                "UserPromptSubmit",
                ExternalHooksRunnerTriggerArgs {
                    matcher_value: Some(HookMatcherValue::Content(input.clone())),
                    signal: Some(signal.clone()),
                    session_id: Some(self.session_context.session_id.clone()),
                    input_data: Some(Map::from_iter([
                        (
                            "prompt".into(),
                            serde_json::to_value(input).expect("ContentPart is serializable"),
                        ),
                        ("isSteer".into(), Value::Bool(context.is_steer)),
                    ])),
                    ..ExternalHooksRunnerTriggerArgs::default()
                },
            )
            .await;
        throw_if_aborted(&signal)?;

        if let Some(block) = render_user_prompt_hook_block_result(Some(&results)) {
            self.context
                .append(vec![context_message(
                    Role::Assistant,
                    block.text,
                    PromptOrigin::HookResult {
                        event: block.event.clone(),
                        blocked: Some(true),
                    },
                )])
                .map_err(|error| Box::new(error) as BoxError)?;
            self.publish_hook_result(block.event, block.message, Some(true));
            return Ok(true);
        }

        if let Some(append) = render_user_prompt_hook_result(Some(&results)) {
            self.context
                .append(vec![context_message(
                    Role::User,
                    append.text,
                    PromptOrigin::HookResult {
                        event: append.event.clone(),
                        blocked: None,
                    },
                )])
                .map_err(|error| Box::new(error) as BoxError)?;
            self.publish_hook_result(append.event, append.message, None);
        }
        Ok(false)
    }

    fn publish_hook_result(&self, hook_event: String, content: String, blocked: Option<bool>) {
        let mut fields = Map::from_iter([
            ("hookEvent".into(), Value::String(hook_event)),
            ("content".into(), Value::String(content)),
        ]);
        if let Some(blocked) = blocked {
            fields.insert("blocked".into(), Value::Bool(blocked));
        }
        self.event_bus
            .publish(DomainEvent::new("hook.result", fields));
    }

    fn notify_turn_ended(&self, event: &TurnEndedEvent) {
        self.stop_hook_continuation_used
            .store(false, Ordering::Release);
        if event.reason == TurnEndReason::Failed
            && let Some(error) = &event.error
        {
            self.notify_stop_failure(error, AbortController::new().signal());
        }
        if event.reason == TurnEndReason::Cancelled {
            self.fire_and_forget(
                "Interrupt",
                Map::from_iter([
                    ("turnId".into(), Value::from(event.turn_id)),
                    ("reason".into(), Value::String("cancelled".into())),
                ]),
                None,
                None,
            );
        }
    }

    fn notify_stop_failure(&self, error: &KimiErrorPayload, signal: AbortSignal) {
        let mut input_data =
            Map::from_iter([("errorMessage".into(), Value::String(error.message.clone()))]);
        if let Some(name) = &error.name {
            input_data.insert("errorType".into(), Value::String(name.clone()));
        }
        self.fire_and_forget(
            "StopFailure",
            input_data,
            error.name.clone().map(HookMatcherValue::String),
            Some(signal),
        );
    }

    async fn run_stop(&self, context: &AfterStepContext) -> Result<Option<String>, BoxError> {
        throw_if_aborted(&context.signal)?;
        if self.stop_hook_continuation_used.load(Ordering::Acquire) {
            return Ok(None);
        }
        let block = self
            .runner
            .trigger_block(
                "Stop",
                ExternalHooksRunnerTriggerArgs {
                    signal: Some(context.signal.clone()),
                    session_id: Some(self.session_context.session_id.clone()),
                    input_data: Some(Map::from_iter([(
                        "stopHookActive".into(),
                        Value::Bool(false),
                    )])),
                    ..ExternalHooksRunnerTriggerArgs::default()
                },
            )
            .await;
        throw_if_aborted(&context.signal)?;
        Ok(block.map(|block| block.reason))
    }

    async fn run_pre_compact(&self, context: &FullCompactionTask) -> Result<(), BoxError> {
        let signal = context.abort_controller.signal();
        throw_if_aborted(&signal)?;
        let trigger = compaction_source(context.trigger);
        self.runner
            .trigger(
                "PreCompact",
                ExternalHooksRunnerTriggerArgs {
                    matcher_value: Some(HookMatcherValue::String(trigger.into())),
                    signal: Some(signal.clone()),
                    session_id: Some(self.session_context.session_id.clone()),
                    input_data: Some(Map::from_iter([
                        ("trigger".into(), Value::String(trigger.into())),
                        ("tokenCount".into(), Value::from(context.token_count)),
                    ])),
                    ..ExternalHooksRunnerTriggerArgs::default()
                },
            )
            .await;
        throw_if_aborted(&signal)
    }

    fn watch_post_compact(&self, context: FullCompactionTask) {
        let runner = self.runner.clone();
        let session_id = self.session_context.session_id.clone();
        let task = tokio::spawn(async move {
            if let Ok(result) = context.promise.await {
                notify_post_compact(&runner, &session_id, context.trigger, &result).await;
            }
        });
        self.tasks.lock().unwrap().push(task);
    }

    fn notify_task_notification(&self, event: &DomainEvent) {
        let Some(notification_type) = string_field(&event.fields, "notificationType") else {
            return;
        };
        let mut input_data = Map::from_iter([("sink".into(), Value::String("context".into()))]);
        input_data.extend(event.fields().clone());
        self.fire_and_forget(
            "Notification",
            input_data,
            Some(HookMatcherValue::String(notification_type)),
            Some(AbortController::new().signal()),
        );
    }
}

impl AgentExternalHooksServiceContract for AgentExternalHooksService {}

impl Disposable for AgentExternalHooksService {
    fn dispose(&self) -> DisposeResult {
        let result = self.disposables.dispose();
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        result
    }
}

async fn notify_post_compact(
    runner: &ExternalHooksRunnerServiceHandle,
    session_id: &str,
    trigger: CompactionSource,
    result: &CompactionResult,
) {
    let trigger = compaction_source(trigger);
    runner
        .fire_and_forget_trigger(
            "PostCompact",
            ExternalHooksRunnerTriggerArgs {
                matcher_value: Some(HookMatcherValue::String(trigger.into())),
                session_id: Some(session_id.into()),
                input_data: Some(Map::from_iter([
                    ("trigger".into(), Value::String(trigger.into())),
                    (
                        "estimatedTokenCount".into(),
                        Value::from(result.tokens_after),
                    ),
                ])),
                ..ExternalHooksRunnerTriggerArgs::default()
            },
        )
        .await;
}

fn compaction_source(source: CompactionSource) -> &'static str {
    match source {
        CompactionSource::Manual => "manual",
        CompactionSource::Auto => "auto",
    }
}

fn string_field(fields: &Map<String, Value>, key: &str) -> Option<String> {
    fields.get(key)?.as_str().map(str::to_owned)
}

fn throw_if_aborted(signal: &AbortSignal) -> Result<(), BoxError> {
    signal
        .throw_if_aborted()
        .map_err(|error| Box::new((*error).clone()) as BoxError)
}

fn context_message(role: Role, text: String, origin: PromptOrigin) -> ContextMessage {
    ContextMessage {
        message: Message::new(role, vec![ContentPart::Text { text }], Vec::new()),
        id: None,
        provider_message_id: None,
        origin: Some(origin),
        is_error: None,
        note: None,
    }
}

pub fn tool_output_text(output: &ExecutableToolOutput) -> String {
    match output {
        ExecutableToolOutput::Text(text) => text.clone(),
        ExecutableToolOutput::Content(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
    }
}

fn utf16_prefix_lossy(value: &str, max_units: usize) -> String {
    let units = value.encode_utf16().take(max_units).collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

// Original: registerScopedService(Agent, ..., Eager, "externalHooks").
pub fn register_agent_external_hooks_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_EXTERNAL_HOOKS_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let runner = accessor.get(EXTERNAL_HOOKS_RUNNER_SERVICE_ID)?;
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let session_context = accessor.get(SESSION_CONTEXT_ID)?;
            let tool_executor = accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?;
            // The source resolves these two services even though their event
            // buses carry the observed notifications.
            accessor.get(AGENT_PERMISSION_GATE_ID)?;
            let prompt = accessor.get(AGENT_PROMPT_SERVICE_ID)?;
            let loop_service = accessor.get(AGENT_LOOP_SERVICE_ID)?;
            let full_compaction = accessor.get(AGENT_FULL_COMPACTION_SERVICE_ID)?;
            accessor.get(AGENT_TASK_SERVICE_ID)?;

            let service = AgentExternalHooksService::new(
                (*runner).clone(),
                (*context).clone(),
                (*event_bus).clone(),
                (*session_context).clone(),
                (*tool_executor).clone(),
                (*prompt).clone(),
                (*loop_service).clone(),
                (*full_compaction).clone(),
            )
            .map_err(|error| DiError::Factory(error.to_string()))?;
            let service: Arc<dyn AgentExternalHooksServiceContract> = service;
            Ok(AgentExternalHooksServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "externalHooks",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        _base::{
            di::{lifecycle::DisposeResult, scope::get_scoped_service_descriptors},
            utils::abort::AbortController,
        },
        agent::{
            context_memory::{
                AgentContextMemoryServiceContract, ContextCompactionInput, ContextCompactionResult,
                ContextMemoryServiceError, LoopRecordedEvent, UndoCut,
            },
            external_hooks::{HookAction, HookBlockDecision, HookResult},
        },
        app::{
            event::{
                event_bus::{EventBusContract, EventBusHandle},
                event_bus_service::EventBusService,
            },
            external_hooks_runner::{
                ExternalHooksRunnerServiceContract, ExternalHooksRunnerTriggerArgs,
            },
        },
        kosong::contract::message::{MediaUrl, ToolCall, ToolCallType},
        session::session_context::{SessionContextInput, make_session_context},
        tool::ExecutableToolResult,
    };
    use async_trait::async_trait;

    #[derive(Clone)]
    struct RunnerCall {
        event: String,
        args: ExternalHooksRunnerTriggerArgs,
    }

    #[derive(Default)]
    struct RecordingRunner {
        calls: Mutex<Vec<RunnerCall>>,
        results: Mutex<Vec<HookResult>>,
        block: Mutex<Option<HookBlockDecision>>,
    }

    #[async_trait]
    impl ExternalHooksRunnerServiceContract for RecordingRunner {
        async fn trigger(
            &self,
            event: &str,
            args: ExternalHooksRunnerTriggerArgs,
        ) -> Vec<HookResult> {
            self.calls.lock().unwrap().push(RunnerCall {
                event: event.into(),
                args,
            });
            self.results.lock().unwrap().clone()
        }

        async fn trigger_block(
            &self,
            event: &str,
            args: ExternalHooksRunnerTriggerArgs,
        ) -> Option<HookBlockDecision> {
            self.calls.lock().unwrap().push(RunnerCall {
                event: event.into(),
                args,
            });
            self.block.lock().unwrap().clone()
        }

        async fn fire_and_forget_trigger(
            &self,
            event: &str,
            args: ExternalHooksRunnerTriggerArgs,
        ) -> Vec<HookResult> {
            self.calls.lock().unwrap().push(RunnerCall {
                event: event.into(),
                args,
            });
            Vec::new()
        }
    }

    impl Disposable for RecordingRunner {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingContext {
        messages: Mutex<Vec<ContextMessage>>,
    }

    impl AgentContextMemoryServiceContract for RecordingContext {
        fn get(&self) -> Vec<ContextMessage> {
            self.messages.lock().unwrap().clone()
        }

        fn append(&self, messages: Vec<ContextMessage>) -> Result<(), ContextMemoryServiceError> {
            self.messages.lock().unwrap().extend(messages);
            Ok(())
        }

        fn append_loop_event(
            &self,
            _event: LoopRecordedEvent,
        ) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), ContextMemoryServiceError> {
            self.messages.lock().unwrap().clear();
            Ok(())
        }

        fn undo(&self, _count: f64) -> Result<UndoCut, ContextMemoryServiceError> {
            Ok(UndoCut {
                cut_index: -1,
                removed_count: 0,
                stopped_at_compaction: false,
            })
        }

        fn apply_compaction(
            &self,
            input: ContextCompactionInput,
        ) -> Result<ContextCompactionResult, ContextMemoryServiceError> {
            Ok(ContextCompactionResult {
                summary: input.summary,
                context_summary: input.context_summary.unwrap_or_default(),
                compacted_count: input.compacted_count,
                tokens_before: input.tokens_before,
                tokens_after: input.tokens_after.unwrap_or_default(),
                kept_user_message_count: input.kept_user_message_count.unwrap_or_default(),
                kept_head_user_message_count: input.kept_head_user_message_count,
                dropped_count: input.dropped_count,
            })
        }
    }

    fn hook_result(action: HookAction, message: Option<&str>) -> HookResult {
        HookResult {
            action,
            message: message.map(str::to_owned),
            reason: None,
            stdout: None,
            stderr: None,
            exit_code: None,
            timed_out: None,
            structured_output: None,
        }
    }

    fn test_service(
        runner: Arc<RecordingRunner>,
        context: Arc<RecordingContext>,
        event_bus: Arc<EventBusService>,
    ) -> AgentExternalHooksService {
        let runner: Arc<dyn ExternalHooksRunnerServiceContract> = runner;
        let context: Arc<dyn AgentContextMemoryServiceContract> = context;
        let event_bus: Arc<dyn EventBusContract> = event_bus;
        AgentExternalHooksService {
            runner: ExternalHooksRunnerServiceHandle(runner),
            context: AgentContextMemoryServiceHandle(context),
            event_bus: EventBusHandle(event_bus),
            session_context: make_session_context(SessionContextInput {
                session_id: "session-1".into(),
                workspace_id: "workspace-1".into(),
                session_dir: "/tmp/session-1".into(),
                session_scope: "sessions/session-1".into(),
                cwd: "/work".into(),
                meta_scope: None,
            }),
            stop_hook_continuation_used: AtomicBool::new(false),
            disposables: DisposableStore::new(),
            tasks: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn tool_output_keeps_only_text_parts_and_uses_javascript_utf16_slice() {
        let output = ExecutableToolOutput::Content(vec![
            ContentPart::Text { text: "a".into() },
            ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "data:image/png;base64,x".into(),
                    id: None,
                },
            },
            ContentPart::Text { text: "b".into() },
        ]);
        assert_eq!(tool_output_text(&output), "ab");
        assert_eq!(utf16_prefix_lossy("a😀b", 2), "a�");
        assert_eq!(utf16_prefix_lossy("a😀b", 3), "a😀");
    }

    #[test]
    fn context_messages_and_hook_result_wire_shape_match_source() {
        let message = context_message(
            Role::User,
            "continue".into(),
            PromptOrigin::SystemTrigger {
                name: "stop_hook".into(),
            },
        );
        assert_eq!(message.message.role, Role::User);
        assert_eq!(
            message.origin,
            Some(PromptOrigin::SystemTrigger {
                name: "stop_hook".into()
            })
        );
        assert_eq!(
            serde_json::to_value(HookResultEvent {
                event_type: "hook.result".into(),
                turn_id: None,
                hook_event: "UserPromptSubmit".into(),
                content: "blocked".into(),
                blocked: Some(true),
            })
            .unwrap(),
            serde_json::json!({
                "type": "hook.result",
                "hookEvent": "UserPromptSubmit",
                "content": "blocked",
                "blocked": true
            })
        );
    }

    #[test]
    fn registration_is_eager_agent_scoped_with_source_domain() {
        register_agent_external_hooks_service();
        let descriptors = get_scoped_service_descriptors(LifecycleScope::Agent);
        let descriptor = descriptors
            .iter()
            .find(|entry| entry.id.to_string() == AGENT_EXTERNAL_HOOKS_SERVICE_ID.to_string())
            .expect("external hooks service is registered");
        assert!(!descriptor.descriptor.supports_delayed_instantiation);
        assert_eq!(descriptor.domain, "externalHooks");
    }

    #[test]
    fn post_tool_text_and_error_payload_helpers_preserve_contract() {
        let call = ToolCall {
            call_type: ToolCallType::Function,
            id: "call-1".into(),
            name: "Read".into(),
            arguments: None,
            extras: None,
            stream_index: None,
        };
        let result = ExecutableToolResult::error("failed");
        assert_eq!(call.name, "Read");
        assert_eq!(tool_output_text(&result.output), "failed");
        let payload = to_error_payload_value(&Value::String(tool_output_text(&result.output)));
        assert_eq!(payload.message, "failed");
    }

    #[tokio::test]
    async fn user_prompt_hook_appends_result_publishes_event_and_passes_session() {
        let runner = Arc::new(RecordingRunner::default());
        runner
            .results
            .lock()
            .unwrap()
            .push(hook_result(HookAction::Allow, Some("remember this")));
        let context = Arc::new(RecordingContext::default());
        let event_bus = Arc::new(EventBusService::new());
        let events = Arc::new(Mutex::new(Vec::new()));
        let event_sink = Arc::clone(&events);
        let _subscription = event_bus.subscribe_type(
            "hook.result",
            Arc::new(move |event| event_sink.lock().unwrap().push(event.clone())),
        );
        let service = test_service(
            Arc::clone(&runner),
            Arc::clone(&context),
            Arc::clone(&event_bus),
        );
        let prompt_message = context_message(Role::User, "hello".into(), PromptOrigin::User);

        let blocked = service
            .run_prompt_submit_hook(&PromptSubmitContext {
                prompt_message,
                is_steer: true,
                block: false,
            })
            .await
            .unwrap();

        assert!(!blocked);
        let messages = context.messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message.role, Role::User);
        assert_eq!(
            messages[0].origin,
            Some(PromptOrigin::HookResult {
                event: "UserPromptSubmit".into(),
                blocked: None
            })
        );
        assert!(
            matches!(&messages[0].message.content[..], [ContentPart::Text { text }] if text.contains("remember this"))
        );
        drop(messages);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].fields["content"], "remember this");
        drop(events);

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].event, "UserPromptSubmit");
        assert_eq!(calls[0].args.session_id.as_deref(), Some("session-1"));
        assert_eq!(
            calls[0].args.input_data.as_ref().unwrap()["isSteer"],
            Value::Bool(true)
        );
        assert!(matches!(
            calls[0].args.matcher_value,
            Some(HookMatcherValue::Content(_))
        ));
    }

    #[tokio::test]
    async fn stop_hook_is_one_shot_until_turn_end_and_checks_cancellation() {
        let runner = Arc::new(RecordingRunner::default());
        *runner.block.lock().unwrap() = Some(HookBlockDecision::new("continue"));
        let context = Arc::new(RecordingContext::default());
        let event_bus = Arc::new(EventBusService::new());
        let service = test_service(Arc::clone(&runner), context, Arc::clone(&event_bus));
        let controller = AbortController::new();
        let after_step = AfterStepContext {
            turn_id: 7,
            step: 1,
            signal: controller.signal(),
            usage: Default::default(),
            finish_reason: FinishReason::Completed,
            stop_turn: false,
        };

        assert_eq!(
            service.run_stop(&after_step).await.unwrap().as_deref(),
            Some("continue")
        );
        service
            .stop_hook_continuation_used
            .store(true, Ordering::Release);
        assert_eq!(service.run_stop(&after_step).await.unwrap(), None);
        service.notify_turn_ended(&TurnEndedEvent {
            turn_id: 7,
            reason: TurnEndReason::Completed,
            error: None,
            duration_ms: None,
        });
        assert_eq!(
            service.run_stop(&after_step).await.unwrap().as_deref(),
            Some("continue")
        );

        controller.abort(None);
        assert!(service.run_stop(&after_step).await.is_err());
        assert_eq!(runner.calls.lock().unwrap().len(), 2);
    }
}
