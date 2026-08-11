//! Agent-scoped tool executor service.
//!
//! Original: `packages/agent-core-v2/src/agent/toolExecutor/toolExecutorService.ts`.

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use futures_util::{Stream, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use serde_json::Value;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::DisposableHandle,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::AbortSignal,
    },
    agent::{
        tool_registry::{
            AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceContract,
            AgentToolRegistryServiceHandle,
        },
        tool_result_truncation::{
            AGENT_TOOL_RESULT_TRUNCATION_SERVICE_ID, AgentToolResultTruncationServiceContract,
            AgentToolResultTruncationServiceHandle, ToolResultTruncationInput,
        },
    },
    app::{
        event::event_bus::{EVENT_BUS_SERVICE_ID, EventBusHandle, TypedEventBusExt},
        telemetry::{
            TELEMETRY_SERVICE_ID, TelemetryServiceEventExt, TelemetryServiceHandle,
            ToolCallDupType as TelemetryDupType, ToolCallErrorType, ToolCallEvent, ToolCallOutcome,
        },
    },
    kosong::contract::message::ToolCall,
    tool::{
        ErasedExecutableTool, ExecutableToolOutput, ExecutableToolResult, RunnableToolExecution,
        ToolAccess, ToolAccesses, ToolExecution, ToolResult, ToolUpdate,
    },
};

use super::{
    AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorHooks, AgentToolExecutorServiceContract,
    AgentToolExecutorServiceHandle, MissingToolDescriber, PreflightedToolCall,
    RunSingleExecutionInput, ToolBeforeExecuteContext, ToolCallDupType, ToolCallGuard,
    ToolCallStartedEvent, ToolCallStartedPayload, ToolDidExecuteContext, ToolExecutionHookContext,
    ToolExecutionResult, ToolExecutorExecuteOptions, ToolExecutorState, ToolProgressEvent,
    ToolResultEvent, ToolScheduler, ToolTaskStarted, UnavailableToolDescriber,
    normalize_tool_result, preflight_tool_call, run_single_execution, tool_telemetry_error_type,
    tool_telemetry_outcome,
};

type PreparedExecute = Arc<dyn Fn(AbortSignal) -> BoxFuture<'static, ToolResult> + Send + Sync>;

struct PreparedToolCall {
    call: PreflightedToolCall,
    accesses: ToolAccesses,
    execute: PreparedExecute,
    stop_batch_after_this: bool,
}

struct TimedToolResult {
    index: usize,
    result: ToolResult,
    duration_ms: u64,
}

pub struct AgentToolExecutorService {
    registry: Arc<dyn AgentToolRegistryServiceContract>,
    event_bus: EventBusHandle,
    telemetry: TelemetryServiceHandle,
    truncation: Arc<dyn AgentToolResultTruncationServiceContract>,
    state: ToolExecutorState,
    hooks: Arc<AgentToolExecutorHooks>,
}

impl AgentToolExecutorService {
    pub fn new(
        registry: Arc<dyn AgentToolRegistryServiceContract>,
        event_bus: EventBusHandle,
        telemetry: TelemetryServiceHandle,
        truncation: Arc<dyn AgentToolResultTruncationServiceContract>,
    ) -> Self {
        Self {
            registry,
            event_bus,
            telemetry,
            truncation,
            state: ToolExecutorState::new(),
            hooks: Arc::new(AgentToolExecutorHooks::default()),
        }
    }

    fn runner(&self) -> ExecutorRunner {
        ExecutorRunner {
            registry: Arc::clone(&self.registry),
            event_bus: self.event_bus.clone(),
            telemetry: self.telemetry.clone(),
            truncation: Arc::clone(&self.truncation),
            state: self.state.clone(),
            hooks: Arc::clone(&self.hooks),
        }
    }
}

impl AgentToolExecutorServiceContract for AgentToolExecutorService {
    fn execute(
        &self,
        calls: Vec<ToolCall>,
        options: ToolExecutorExecuteOptions,
    ) -> super::ToolExecutionStream {
        if calls.is_empty() {
            return Box::pin(futures_util::stream::empty());
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        let runner = self.runner();
        let task = tokio::spawn(async move {
            if let Err(error) = runner.execute(calls, options, &sender).await {
                let _ = sender.send(Err(error));
            }
        });
        Box::pin(ManagedExecutionStream { receiver, task })
    }

    fn hooks(&self) -> &AgentToolExecutorHooks {
        self.hooks.as_ref()
    }

    fn record_dup_type(&self, tool_call_id: String, dup_type: ToolCallDupType) {
        self.state.record_dup_type(tool_call_id, dup_type);
    }

    fn register_tool_call_guard(&self, guard: ToolCallGuard) -> DisposableHandle {
        self.state.register_tool_call_guard(guard)
    }

    fn register_unavailable_tool_describer(
        &self,
        describer: UnavailableToolDescriber,
    ) -> DisposableHandle {
        self.state.register_unavailable_tool_describer(describer)
    }

    fn register_missing_tool_describer(&self, describer: MissingToolDescriber) -> DisposableHandle {
        self.state.register_missing_tool_describer(describer)
    }
}

#[derive(Clone)]
struct ExecutorRunner {
    registry: Arc<dyn AgentToolRegistryServiceContract>,
    event_bus: EventBusHandle,
    telemetry: TelemetryServiceHandle,
    truncation: Arc<dyn AgentToolResultTruncationServiceContract>,
    state: ToolExecutorState,
    hooks: Arc<AgentToolExecutorHooks>,
}

impl ExecutorRunner {
    async fn execute(
        &self,
        calls: Vec<ToolCall>,
        options: ToolExecutorExecuteOptions,
        sender: &mpsc::UnboundedSender<Result<ToolExecutionResult, BoxError>>,
    ) -> Result<(), BoxError> {
        self.state.begin_turn(options.turn_id);
        let preflighted = calls
            .iter()
            .cloned()
            .map(|call| {
                preflight_tool_call(
                    self.registry.as_ref(),
                    call,
                    self.state.tool_call_guard().as_ref(),
                    self.state.unavailable_tool_describer().as_ref(),
                    self.state.missing_tool_describer().as_ref(),
                )
            })
            .collect::<Vec<_>>();
        let mut prepared = Vec::with_capacity(preflighted.len());
        let mut stopped = false;
        for call in preflighted {
            let item = if stopped {
                self.prepare_skipped(call, &options)?
            } else {
                self.prepare(call, &calls, &options).await?
            };
            stopped |= item.stop_batch_after_this;
            prepared.push(item);
        }
        let mut scheduler = ToolScheduler::new();
        for (index, item) in prepared.iter().enumerate() {
            let execute = Arc::clone(&item.execute);
            let signal = options.signal.clone();
            scheduler.add(super::ToolCallTask {
                accesses: item.accesses.clone(),
                start: Arc::new(move || {
                    let execute = Arc::clone(&execute);
                    let signal = signal.clone();
                    Box::pin(async move {
                        let started = Instant::now();
                        Ok(ToolTaskStarted {
                            result: Box::pin(async move {
                                let result = execute(signal).await;
                                Ok(TimedToolResult {
                                    index,
                                    result,
                                    duration_ms: started
                                        .elapsed()
                                        .as_millis()
                                        .min(u128::from(u64::MAX))
                                        as u64,
                                })
                            }),
                        })
                    })
                }),
            });
        }
        // Original: `execute()` starts each finalization as a separate promise
        // and races scheduler completions against already-running
        // finalizations. This is essential for same-step deduplication: a
        // synthetic duplicate may await the original call's finalization.
        let mut finalizations = FuturesUnordered::<BoxFuture<'static, Result<(), BoxError>>>::new();
        let mut first_error = None;
        while scheduler.has_pending() || !finalizations.is_empty() {
            let scheduler_pending = scheduler.has_pending();
            tokio::select! {
                Some(timed) = scheduler.next(), if scheduler_pending => {
                    match timed {
                        Ok(timed) => {
                            let call = prepared[timed.index].call.clone();
                            let runner = self.clone();
                            let options = options.clone();
                            let sender = sender.clone();
                            finalizations.push(Box::pin(async move {
                                let result = runner.finalize(&call, timed.result, &options).await;
                                runner.dispatch_result(&call, &result, &options)?;
                                runner.track(&call, &result, timed.duration_ms, &options);
                                sender.send(Ok(ToolExecutionResult {
                                    tool_call_id: tool_call(&call).id.clone(),
                                    tool_name: tool_name(&call).into(),
                                    result,
                                }))
                                .map_err(|error| Box::new(error) as BoxError)?;
                                Ok(())
                            }));
                        }
                        Err(error) if first_error.is_none() => first_error = Some(error),
                        Err(_) => {}
                    }
                }
                Some(result) = finalizations.next(), if !finalizations.is_empty() => {
                    if let Err(error) = result
                        && first_error.is_none()
                    {
                        first_error = Some(error);
                    }
                },
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn prepare(
        &self,
        call: PreflightedToolCall,
        all_calls: &[ToolCall],
        options: &ToolExecutorExecuteOptions,
    ) -> Result<PreparedToolCall, BoxError> {
        let PreflightedToolCall::Runnable {
            tool_call,
            tool_name,
            tool,
            args,
        } = call
        else {
            return self.prepare_rejected(call, options);
        };
        let execution = match tool.resolve_execution_value(args.clone()).await {
            Ok(ToolExecution::Runnable(execution)) => execution,
            Ok(ToolExecution::Error(result)) => {
                return self.prepare_synthetic(tool_call, tool_name, tool, args, result, options);
            }
            Err(error) => {
                let output = format!("Tool \"{tool_name}\" failed to resolve execution: {error}");
                return self.prepare_error(tool_call, tool_name, args, output, options);
            }
        };
        let display = execution
            .description
            .as_ref()
            .filter(|value| !value.is_empty())
            .cloned();
        if options.signal.aborted() {
            let output = super::aborted_tool_output(&tool_name, &options.signal);
            return self.prepare_error(tool_call, tool_name, args, output, options);
        }
        let context = ToolExecutionHookContext {
            turn_id: options.turn_id,
            signal: options.signal.clone(),
            trace: options.trace.clone(),
            tool_call: tool_call.clone(),
            tool_calls: all_calls.to_vec(),
            tool: Some(Arc::clone(&tool)),
            args: args.clone(),
        };
        let mut before = ToolBeforeExecuteContext::new(
            super::ResolvedToolExecutionHookContext::new(context, execution.clone()),
        );
        self.hooks
            .on_before_execute_tool
            .run(&mut before, None)
            .await?;
        if let Some(decision) = &before.decision {
            if decision.block == Some(true) {
                let output = decision
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("Tool call \"{tool_name}\" was blocked"));
                return self.prepare_error(tool_call, tool_name, args, output, options);
            }
            if let Some(result) = decision.synthetic_result.clone() {
                return self.prepare_synthetic(tool_call, tool_name, tool, args, result, options);
            }
        }
        self.dispatch_call(
            &tool_call,
            &tool_name,
            &args,
            display,
            execution.display.clone(),
            options,
        )?;
        let metadata = before
            .decision
            .and_then(|decision| decision.execution_metadata);
        let runner = self.clone();
        let call_for_execution = PreflightedToolCall::Runnable {
            tool_call,
            tool_name,
            tool,
            args,
        };
        let execution_call = call_for_execution.clone();
        let execution_options = (*options).clone();
        Ok(PreparedToolCall {
            accesses: execution.accesses.clone().unwrap_or_else(ToolAccess::all),
            stop_batch_after_this: execution.stop_batch_after_this == Some(true),
            execute: Arc::new(move |signal| {
                let runner = runner.clone();
                let call = execution_call.clone();
                let execution = execution.clone();
                let metadata = metadata.clone();
                let options = execution_options.clone();
                Box::pin(
                    async move { runner.run(call, execution, metadata, options, signal).await },
                )
            }),
            call: call_for_execution,
        })
    }

    fn prepare_rejected(
        &self,
        call: PreflightedToolCall,
        options: &ToolExecutorExecuteOptions,
    ) -> Result<PreparedToolCall, BoxError> {
        match call {
            PreflightedToolCall::Rejected {
                tool_call,
                tool_name,
                args,
                output,
            } => self.prepare_error(tool_call, tool_name, args, output, options),
            PreflightedToolCall::Runnable { .. } => Err(std::io::Error::other(
                "expected a rejected tool call during rejected-call preparation",
            )
            .into()),
        }
    }

    fn prepare_skipped(
        &self,
        call: PreflightedToolCall,
        options: &ToolExecutorExecuteOptions,
    ) -> Result<PreparedToolCall, BoxError> {
        match call {
            PreflightedToolCall::Runnable {
                tool_call,
                tool_name,
                args,
                ..
            }
            | PreflightedToolCall::Rejected {
                tool_call,
                tool_name,
                args,
                ..
            } => self.prepare_error(
                tool_call,
                tool_name,
                args,
                "Tool skipped because a previous tool call stopped the turn.".into(),
                options,
            ),
        }
    }

    fn prepare_error(
        &self,
        tool_call: ToolCall,
        tool_name: String,
        args: Value,
        output: String,
        options: &ToolExecutorExecuteOptions,
    ) -> Result<PreparedToolCall, BoxError> {
        self.dispatch_call(&tool_call, &tool_name, &args, None, None, options)?;
        let result = ToolResult::from(ExecutableToolResult::error(output));
        Ok(PreparedToolCall {
            call: PreflightedToolCall::Rejected {
                tool_call,
                tool_name,
                args,
                output: String::new(),
            },
            accesses: ToolAccess::none(),
            execute: Arc::new(move |_| Box::pin(std::future::ready(result.clone()))),
            stop_batch_after_this: false,
        })
    }

    fn prepare_synthetic(
        &self,
        tool_call: ToolCall,
        tool_name: String,
        tool: Arc<dyn ErasedExecutableTool>,
        args: Value,
        raw: ExecutableToolResult,
        options: &ToolExecutorExecuteOptions,
    ) -> Result<PreparedToolCall, BoxError> {
        self.dispatch_call(&tool_call, &tool_name, &args, None, None, options)?;
        let result = normalize_tool_result(raw);
        let stop_batch_after_this =
            result.stop_batch_after_this == Some(true) || result.stop_turn == Some(true);
        Ok(PreparedToolCall {
            call: PreflightedToolCall::Runnable {
                tool_call,
                tool_name,
                tool,
                args,
            },
            accesses: ToolAccess::none(),
            execute: Arc::new(move |_| Box::pin(std::future::ready(result.clone()))),
            stop_batch_after_this,
        })
    }

    async fn run(
        &self,
        call: PreflightedToolCall,
        execution: RunnableToolExecution,
        metadata: Option<Value>,
        options: ToolExecutorExecuteOptions,
        signal: AbortSignal,
    ) -> ToolResult {
        let PreflightedToolCall::Runnable {
            tool_call,
            tool_name,
            ..
        } = call
        else {
            return ToolResult::from(ExecutableToolResult::error("invalid runnable tool call"));
        };
        let event_bus = self.event_bus.clone();
        let progress_call_id = tool_call.id.clone();
        let progress_turn = options.turn_id;
        let progress_signal = signal.clone();
        let update = Arc::new(move |update: ToolUpdate| {
            if !progress_signal.aborted() {
                let payload = ToolProgressEvent {
                    turn_id: progress_turn,
                    tool_call_id: progress_call_id.clone(),
                    update,
                };
                event_bus.publish_typed(payload);
            }
        });
        run_single_execution(RunSingleExecutionInput {
            tool_name: &tool_name,
            tool_call_id: tool_call.id,
            execution: &execution,
            turn_id: options.turn_id,
            trace: options.trace,
            metadata,
            signal,
            on_update: Some(update),
        })
        .await
    }

    async fn finalize(
        &self,
        call: &PreflightedToolCall,
        result: ToolResult,
        options: &ToolExecutorExecuteOptions,
    ) -> ToolResult {
        let PreflightedToolCall::Runnable {
            tool_call: source_tool_call,
            tool,
            args,
            ..
        } = call
        else {
            return result;
        };
        let mut context = ToolDidExecuteContext {
            context: ToolExecutionHookContext {
                turn_id: options.turn_id,
                signal: options.signal.clone(),
                trace: options.trace.clone(),
                tool_call: source_tool_call.clone(),
                tool_calls: vec![source_tool_call.clone()],
                tool: Some(Arc::clone(tool)),
                args: args.clone(),
            },
            result: executable_result(&result),
            stop_turn: None,
        };
        if let Err(error) = self.hooks.on_did_execute_tool.run(&mut context, None).await {
            return ToolResult {
                output: ExecutableToolOutput::Text(format!(
                    "onDidExecuteTool hook failed for \"{}\": {error}",
                    tool_name(call)
                )),
                is_error: true,
                description: result.description,
                display: result.display,
                approval_rule: result.approval_rule,
                stop_turn: None,
                truncated: None,
                note: None,
                delivery: None,
                stop_batch_after_this: None,
            };
        }
        let delivery = context.result.delivery.clone();
        let effective = normalize_tool_result(context.result);
        let merged = ToolResult {
            output: effective.output,
            is_error: effective.is_error,
            stop_turn: Some(
                result.stop_turn == Some(true)
                    || context.stop_turn == Some(true)
                    || effective.stop_turn == Some(true),
            )
            .filter(|value| *value),
            truncated: effective.truncated,
            note: effective.note,
            delivery,
            description: result.description,
            display: result.display,
            approval_rule: result.approval_rule,
            stop_batch_after_this: result.stop_batch_after_this,
        };
        let truncated = self
            .truncation
            .truncate_for_model(ToolResultTruncationInput {
                tool_name: tool_name(call).into(),
                tool_call_id: tool_call(call).id.clone(),
                result: executable_result(&merged),
            })
            .await;
        ToolResult {
            output: truncated.output,
            is_error: truncated.is_error,
            stop_turn: truncated.stop_turn,
            truncated: truncated.truncated,
            note: truncated.note,
            delivery: truncated.delivery,
            description: merged.description,
            display: merged.display,
            approval_rule: merged.approval_rule,
            stop_batch_after_this: merged.stop_batch_after_this,
        }
    }

    fn dispatch_call(
        &self,
        call: &ToolCall,
        name: &str,
        args: &Value,
        description: Option<String>,
        display: Option<crate::tool::ToolInputDisplay>,
        options: &ToolExecutorExecuteOptions,
    ) -> Result<(), BoxError> {
        let payload = ToolCallStartedEvent {
            turn_id: options.turn_id,
            tool_call_id: call.id.clone(),
            name: name.into(),
            args: args.clone(),
            description,
            display,
        };
        self.event_bus.publish_typed(payload);
        if let Some(handler) = &options.on_tool_call {
            handler(ToolCallStartedPayload {
                tool_call_id: call.id.clone(),
                name: name.into(),
                args: args.clone(),
            });
        }
        Ok(())
    }

    fn dispatch_result(
        &self,
        call: &PreflightedToolCall,
        result: &ToolResult,
        options: &ToolExecutorExecuteOptions,
    ) -> Result<(), BoxError> {
        let payload = ToolResultEvent {
            turn_id: options.turn_id,
            tool_call_id: tool_call(call).id.clone(),
            output: output_value(&result.output)?,
            is_error: Some(result.is_error),
            synthetic: None,
        };
        self.event_bus.publish_typed(payload);
        Ok(())
    }

    fn track(
        &self,
        call: &PreflightedToolCall,
        result: &ToolResult,
        duration_ms: u64,
        options: &ToolExecutorExecuteOptions,
    ) {
        let turn_id = options.turn_id;
        let outcome = match tool_telemetry_outcome(result) {
            super::ToolTelemetryOutcome::Success => ToolCallOutcome::Success,
            super::ToolTelemetryOutcome::Error => ToolCallOutcome::Error,
            super::ToolTelemetryOutcome::Cancelled => ToolCallOutcome::Cancelled,
        };
        let dup_type = match self.state.take_dup_type(&tool_call(call).id) {
            None => TelemetryDupType::Normal,
            Some(ToolCallDupType::SameStep) => TelemetryDupType::SameStep,
            Some(ToolCallDupType::CrossStep) => TelemetryDupType::CrossStep,
        };
        let error_type = result.is_error.then(|| {
            match tool_telemetry_error_type(tool_telemetry_outcome(result)) {
                "cancelled" => ToolCallErrorType::Cancelled,
                _ => ToolCallErrorType::Error,
            }
        });
        let _ = self.telemetry.track_event(&ToolCallEvent {
            turn_id,
            tool_call_id: tool_call(call).id.clone(),
            tool_name: tool_name(call).into(),
            outcome,
            duration_ms,
            dup_type,
            error_type,
            trace_id: options.trace.as_ref().and_then(|trace| trace.trace_id()),
        });
    }
}

fn tool_call(call: &PreflightedToolCall) -> &ToolCall {
    match call {
        PreflightedToolCall::Runnable { tool_call, .. }
        | PreflightedToolCall::Rejected { tool_call, .. } => tool_call,
    }
}
fn tool_name(call: &PreflightedToolCall) -> &str {
    match call {
        PreflightedToolCall::Runnable { tool_name, .. }
        | PreflightedToolCall::Rejected { tool_name, .. } => tool_name,
    }
}
fn executable_result(result: &ToolResult) -> ExecutableToolResult {
    ExecutableToolResult {
        output: result.output.clone(),
        is_error: result.is_error,
        stop_turn: result.stop_turn,
        truncated: result.truncated,
        note: result.note.clone(),
        delivery: result.delivery.clone(),
    }
}
fn output_value(output: &ExecutableToolOutput) -> Result<Value, BoxError> {
    match output {
        ExecutableToolOutput::Text(text) => Ok(Value::String(text.clone())),
        ExecutableToolOutput::Content(parts) => Ok(serde_json::to_value(parts)?),
    }
}
struct ManagedExecutionStream {
    receiver: mpsc::UnboundedReceiver<Result<ToolExecutionResult, BoxError>>,
    task: JoinHandle<()>,
}
impl Stream for ManagedExecutionStream {
    type Item = Result<ToolExecutionResult, BoxError>;
    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(context)
    }
}
impl Drop for ManagedExecutionStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// Original: registerScopedService(... AgentToolExecutorService ...).
pub fn register_agent_tool_executor_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TOOL_EXECUTOR_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let registry: AgentToolRegistryServiceHandle =
                (*accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?).clone();
            let event_bus: EventBusHandle = (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone();
            let telemetry: TelemetryServiceHandle = (*accessor.get(TELEMETRY_SERVICE_ID)?).clone();
            let truncation: AgentToolResultTruncationServiceHandle =
                (*accessor.get(AGENT_TOOL_RESULT_TRUNCATION_SERVICE_ID)?).clone();
            let service: Arc<dyn AgentToolExecutorServiceContract> = Arc::new(
                AgentToolExecutorService::new(registry.0, event_bus, telemetry, truncation.0),
            );
            Ok(AgentToolExecutorServiceHandle(service))
        }),
        InstantiationType::Eager,
        "toolExecutor",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures_util::StreamExt;

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        agent::{
            tool_registry::{
                AgentToolRegistryService, AgentToolRegistryServiceContract, ToolRegistrationOptions,
            },
            tool_result_truncation::AgentToolResultTruncationServiceContract,
        },
        app::{
            event::{event_bus::EventBusContract, event_bus_service::EventBusService},
            telemetry::noop_telemetry_service,
        },
        kosong::contract::{message::ToolCallType, tool::Tool},
        tool::ExecutableTool,
    };

    struct PassthroughTruncation;

    struct EchoTool {
        definition: Tool,
    }

    #[async_trait]
    impl ExecutableTool for EchoTool {
        type Input = Value;

        fn tool(&self) -> &Tool {
            &self.definition
        }

        async fn resolve_execution(&self, input: Self::Input) -> ToolExecution {
            let text = input["text"].as_str().unwrap_or_default().to_owned();
            ToolExecution::Runnable(RunnableToolExecution::new(
                "always",
                Arc::new(move |_| {
                    let text = text.clone();
                    Box::pin(async move { ExecutableToolResult::success(text) })
                }),
            ))
        }
    }

    #[async_trait]
    impl AgentToolResultTruncationServiceContract for PassthroughTruncation {
        async fn truncate_for_model(
            &self,
            input: ToolResultTruncationInput,
        ) -> ExecutableToolResult {
            input.result
        }
    }

    #[tokio::test]
    async fn missing_tool_is_streamed_after_started_and_result_events() {
        let events = Arc::new(EventBusService::new());
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_events = Arc::clone(&observed);
        let _subscription = events.subscribe(Arc::new(move |event| {
            observed_events
                .lock()
                .unwrap()
                .push(event.event_type.clone());
        }));
        let service = AgentToolExecutorService::new(
            Arc::new(AgentToolRegistryService::new()),
            EventBusHandle(events),
            noop_telemetry_service(),
            Arc::new(PassthroughTruncation),
        );
        let mut stream = service.execute(
            vec![ToolCall {
                call_type: ToolCallType::Function,
                id: "call-missing".into(),
                name: "Missing".into(),
                arguments: Some("{}".into()),
                extras: None,
                stream_index: None,
            }],
            ToolExecutorExecuteOptions {
                signal: AbortController::new().signal(),
                turn_id: crate::agent::TurnId::new(3),
                trace: None,
                on_tool_call: None,
            },
        );
        let result = stream.next().await.unwrap().unwrap();
        assert_eq!(result.tool_call_id, "call-missing");
        assert!(result.result.is_error);
        assert_eq!(
            result.result.output,
            ExecutableToolOutput::Text("Tool \"Missing\" not found".into())
        );
        assert_eq!(
            *observed.lock().unwrap(),
            ["tool.call.started", "tool.result"]
        );
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn runnable_tool_is_resolved_and_streamed_with_normalized_result() {
        let registry = AgentToolRegistryService::new();
        let _registration = registry.register(
            Arc::new(EchoTool {
                definition: Tool {
                    name: "Echo".into(),
                    description: "Echoes text".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "required": ["text"],
                        "properties": {"text": {"type": "string"}},
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    deferred: None,
                },
            }),
            ToolRegistrationOptions::default(),
        );
        let events = Arc::new(EventBusService::new());
        let service = AgentToolExecutorService::new(
            Arc::new(registry),
            EventBusHandle(events),
            noop_telemetry_service(),
            Arc::new(PassthroughTruncation),
        );
        let mut stream = service.execute(
            vec![ToolCall {
                call_type: ToolCallType::Function,
                id: "call-echo".into(),
                name: "Echo".into(),
                arguments: Some(r#"{"text":"hello"}"#.into()),
                extras: None,
                stream_index: None,
            }],
            ToolExecutorExecuteOptions {
                signal: AbortController::new().signal(),
                turn_id: crate::agent::TurnId::new(4),
                trace: None,
                on_tool_call: None,
            },
        );
        let result = stream.next().await.unwrap().unwrap();
        assert_eq!(result.tool_name, "Echo");
        assert_eq!(
            result.result.output,
            ExecutableToolOutput::Text("hello".into())
        );
        assert!(!result.result.is_error);
    }
}
