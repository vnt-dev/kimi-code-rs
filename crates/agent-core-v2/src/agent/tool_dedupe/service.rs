//! Agent-scoped tool-deduplication service.
//!
//! Original: `toolDedupeService.ts`.

use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    sync::{Arc, Mutex, Weak},
};

use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
        utils::canonical_args::canonical_telemetry_args,
    },
    agent::{
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle},
        tool_executor::{
            AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle, ToolCallDupType,
        },
    },
    app::telemetry::{
        TELEMETRY_SERVICE_ID, TelemetryServiceEventExt, TelemetryServiceHandle,
        ToolCallDedupDetectedEvent, ToolCallDupType as TelemetryDupType, ToolCallRepeatAction,
        ToolCallRepeatEvent,
    },
    hooks::HookRegisterOptions,
    kosong::contract::{message::ContentPart, request_trace::LlmRequestTrace},
    tool::{ExecutableToolOutput, ExecutableToolResult},
};

const REMINDER_TEXT_1: &str = "\n\n<system-reminder>\nThe same tool call has been repeated several times in a row. Before making your next call, write one sentence stating what new information you expect it to produce. Then act on that sentence: if it names something this result does not already give you, choose the action that best provides it; otherwise, continue with the evidence you already have.\n</system-reminder>";
const REMINDER_TEXT_3: &str = "\n\n<system-reminder>\nWrite your final response now, without any further tool calls. Cover: the current blocker, each approach you have tried and what it established, and the specific information or decision you need from the user to unblock progress. Text only.\n</system-reminder>";
const REPEAT_REMINDER_1_START: u64 = 3;
const REPEAT_REMINDER_2_START: u64 = 5;
const REPEAT_REMINDER_3_START: u64 = 8;
const REPEAT_FORCE_STOP_STREAK: u64 = 12;

pub trait AgentToolDedupeServiceContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct AgentToolDedupeServiceHandle(pub Arc<dyn AgentToolDedupeServiceContract>);

impl Deref for AgentToolDedupeServiceHandle {
    type Target = dyn AgentToolDedupeServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentToolDedupeServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_TOOL_DEDUPE_SERVICE_ID: ServiceIdentifier<AgentToolDedupeServiceHandle> =
    ServiceIdentifier::new("agentToolDedupeService");

#[derive(Default)]
struct State {
    deferreds: HashMap<String, Vec<oneshot::Sender<ExecutableToolResult>>>,
    duplicate_receivers: HashMap<String, oneshot::Receiver<ExecutableToolResult>>,
    step_calls: Vec<String>,
    original_call_index: HashMap<String, usize>,
    synthetic_call_ids: HashSet<String>,
    call_key_by_call_id: HashMap<String, String>,
    consecutive_key: Option<String>,
    consecutive_count: u64,
    active_turn_id: Option<i64>,
    active_step: u64,
}

struct DedupTelemetryInput<'a> {
    call_id: &'a str,
    name: &'a str,
    args: &'a serde_json::Value,
    dup_type: ToolCallDupType,
    turn_id: Option<i64>,
    step: u64,
    trace: Option<&'a LlmRequestTrace>,
}

pub struct AgentToolDedupeService {
    telemetry: TelemetryServiceHandle,
    executor: AgentToolExecutorServiceHandle,
    state: Arc<Mutex<State>>,
    disposables: DisposableStore,
}

impl AgentToolDedupeService {
    pub fn new(
        telemetry: TelemetryServiceHandle,
        loop_service: AgentLoopServiceHandle,
        executor: AgentToolExecutorServiceHandle,
    ) -> Result<Arc<Self>, crate::hooks::HookRegistrationError> {
        let service = Arc::new(Self {
            telemetry,
            executor,
            state: Arc::new(Mutex::new(State::default())),
            disposables: DisposableStore::new(),
        });
        service.install_hooks(loop_service)?;
        Ok(service)
    }

    fn install_hooks(
        self: &Arc<Self>,
        loop_service: AgentLoopServiceHandle,
    ) -> Result<(), crate::hooks::HookRegistrationError> {
        let weak = Arc::downgrade(self);
        self.disposables
            .add(loop_service.hooks().on_will_begin_step.register(
                "tool-dedupe",
                Arc::new(move |context, next| {
                    let weak = Weak::clone(&weak);
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.begin_step(context.turn_id, context.step);
                        }
                        next(context).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);
        let weak = Arc::downgrade(self);
        self.disposables
            .add(loop_service.hooks().on_did_finish_step.register(
                "tool-dedupe",
                Arc::new(move |context, next| {
                    let weak = Weak::clone(&weak);
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.end_step();
                        }
                        next(context).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);
        let weak = Arc::downgrade(self);
        self.disposables
            .add(self.executor.hooks().on_before_execute_tool.register(
                "tool-dedupe",
                Arc::new(move |context, next| {
                    let weak = Weak::clone(&weak);
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade()
                            && service.check_tool_call(
                                &context.tool_call.id,
                                &context.tool_call.name,
                                &context.args,
                                context.trace.as_ref(),
                            )
                        {
                            context.decision =
                                Some(crate::agent::tool_executor::AuthorizeToolExecutionResult {
                                    synthetic_result: Some(ExecutableToolResult::success("")),
                                    ..Default::default()
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
            .add(self.executor.hooks().on_did_execute_tool.register(
                "tool-dedupe",
                Arc::new(move |context, next| {
                    let weak = Weak::clone(&weak);
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            context.result = service
                                .finalize_result(
                                    &context.tool_call.id,
                                    &context.tool_call.name,
                                    &context.args,
                                    context.result.clone(),
                                    context.trace.as_ref(),
                                )
                                .await;
                            if context.result.stop_turn == Some(true) {
                                context.stop_turn = Some(true);
                            }
                        }
                        next(context).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);
        Ok(())
    }

    fn begin_step(&self, turn_id: i64, step: u64) {
        let mut state = self.state.lock().unwrap();
        if state.active_turn_id != Some(turn_id) {
            state.active_turn_id = Some(turn_id);
            state.consecutive_key = None;
            state.consecutive_count = 0;
        }
        state.active_step = step;
        let pending = std::mem::take(&mut state.deferreds);
        for senders in pending.into_values() {
            for sender in senders {
                let _ = sender.send(ExecutableToolResult::error(
                    "Tool call deduplicated but original result was lost",
                ));
            }
        }
        state.duplicate_receivers.clear();
        state.step_calls.clear();
        state.original_call_index.clear();
        state.synthetic_call_ids.clear();
        state.call_key_by_call_id.clear();
    }

    fn end_step(&self) {
        let mut state = self.state.lock().unwrap();
        for key in state.step_calls.clone() {
            if state.consecutive_key.as_deref() == Some(&key) {
                state.consecutive_count += 1;
            } else {
                state.consecutive_key = Some(key);
                state.consecutive_count = 1;
            }
        }
    }

    /// Returns true when the executor should use its synthetic placeholder.
    fn check_tool_call(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        trace: Option<&LlmRequestTrace>,
    ) -> bool {
        let key = format!("{tool_name} {}", canonical_telemetry_args(args));
        let mut state = self.state.lock().unwrap();
        let index = state.step_calls.len();
        state.step_calls.push(key.clone());
        state
            .call_key_by_call_id
            .insert(tool_call_id.into(), key.clone());
        if state.deferreds.contains_key(&key) {
            let (sender, receiver) = oneshot::channel();
            state.deferreds.get_mut(&key).unwrap().push(sender);
            state
                .duplicate_receivers
                .insert(tool_call_id.into(), receiver);
            state.synthetic_call_ids.insert(tool_call_id.into());
            let active_turn_id = state.active_turn_id;
            let active_step = state.active_step;
            drop(state);
            self.record_dup_type(DedupTelemetryInput {
                call_id: tool_call_id,
                name: tool_name,
                args,
                dup_type: ToolCallDupType::SameStep,
                turn_id: active_turn_id,
                step: active_step,
                trace,
            });
            return true;
        }
        state.deferreds.insert(key.clone(), Vec::new());
        state.original_call_index.insert(tool_call_id.into(), index);
        let cross_step =
            state.consecutive_key.as_deref() == Some(&key) && state.consecutive_count > 0;
        let active_turn_id = state.active_turn_id;
        let active_step = state.active_step;
        drop(state);
        if cross_step {
            self.record_dup_type(DedupTelemetryInput {
                call_id: tool_call_id,
                name: tool_name,
                args,
                dup_type: ToolCallDupType::CrossStep,
                turn_id: active_turn_id,
                step: active_step,
                trace,
            });
        }
        false
    }

    fn record_dup_type(&self, input: DedupTelemetryInput<'_>) {
        self.executor
            .record_dup_type(input.call_id.into(), input.dup_type);
        let _ = self.telemetry.track_event(&ToolCallDedupDetectedEvent {
            turn_id: input.turn_id.and_then(|id| u64::try_from(id).ok()),
            step_no: input.step,
            tool_call_id: input.call_id.into(),
            tool_name: input.name.into(),
            dup_type: match input.dup_type {
                ToolCallDupType::SameStep => TelemetryDupType::SameStep,
                ToolCallDupType::CrossStep => TelemetryDupType::CrossStep,
            },
            args_hash: args_hash(input.args),
            trace_id: input.trace.and_then(|value| value.trace_id.clone()),
        });
    }

    async fn finalize_result(
        &self,
        call_id: &str,
        tool_name: &str,
        _args: &serde_json::Value,
        result: ExecutableToolResult,
        trace: Option<&LlmRequestTrace>,
    ) -> ExecutableToolResult {
        let (key, is_synthetic, receiver, original_index) = {
            let mut state = self.state.lock().unwrap();
            let Some(key) = state.call_key_by_call_id.remove(call_id) else {
                return result;
            };
            let synthetic = state.synthetic_call_ids.remove(call_id);
            let receiver = synthetic
                .then(|| state.duplicate_receivers.remove(call_id))
                .flatten();
            let original_index = (!synthetic)
                .then(|| state.original_call_index.remove(call_id))
                .flatten();
            (key, synthetic, receiver, original_index)
        };
        if is_synthetic {
            return match receiver {
                Some(receiver) => receiver.await.unwrap_or(result),
                None => result,
            };
        }
        let Some(index) = original_index else {
            return result;
        };
        let (streak, turn_id) = {
            let state = self.state.lock().unwrap();
            let mut last = state.consecutive_key.clone();
            let mut streak = state.consecutive_count;
            for candidate in state.step_calls.iter().take(index + 1) {
                if last.as_deref() == Some(candidate) {
                    streak += 1;
                } else {
                    last = Some(candidate.clone());
                    streak = 1;
                }
            }
            (streak, state.active_turn_id)
        };
        let (final_result, action) = if streak >= REPEAT_FORCE_STOP_STREAK {
            (
                force_stop_result(result, REMINDER_TEXT_3),
                ToolCallRepeatAction::Stop,
            )
        } else if streak >= REPEAT_REMINDER_3_START {
            (
                append_reminder(result, REMINDER_TEXT_3),
                ToolCallRepeatAction::R3,
            )
        } else if streak >= REPEAT_REMINDER_2_START {
            (
                append_reminder(result, &make_reminder_text_2(streak)),
                ToolCallRepeatAction::R2,
            )
        } else if streak >= REPEAT_REMINDER_1_START {
            (
                append_reminder(result, REMINDER_TEXT_1),
                ToolCallRepeatAction::R1,
            )
        } else {
            (result, ToolCallRepeatAction::None)
        };
        if streak >= 2 {
            let _ = self.telemetry.track_event(&ToolCallRepeatEvent {
                turn_id: turn_id.and_then(|id| u64::try_from(id).ok()),
                tool_name: tool_name.into(),
                repeat_count: streak,
                action,
                trace_id: trace.and_then(|value| value.trace_id.clone()),
            });
        }
        let senders = self
            .state
            .lock()
            .unwrap()
            .deferreds
            .remove(&key)
            .unwrap_or_default();
        for sender in senders {
            let _ = sender.send(final_result.clone());
        }
        final_result
    }
}

impl AgentToolDedupeServiceContract for AgentToolDedupeService {}
impl Disposable for AgentToolDedupeService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

fn make_reminder_text_2(repeat_count: u64) -> String {
    format!(
        "\n\n<system-reminder>\nThe same tool call has now been issued {repeat_count} times in a row. Choose exactly one of the following and state your choice before acting:\n(1) Falsification check: run the cheapest test that could conclusively disprove your current approach, if such a test exists.\n(2) Missing input: tell the user precisely what information or decision you need to proceed, and ask for it.\n(3) Conclude: deliver your best result based on the evidence already gathered, listing anything that remains uncertain.\n</system-reminder>"
    )
}
fn args_hash(args: &serde_json::Value) -> String {
    let digest = Sha256::digest(canonical_telemetry_args(args).as_bytes());
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn append_reminder(mut result: ExecutableToolResult, reminder: &str) -> ExecutableToolResult {
    match &mut result.output {
        ExecutableToolOutput::Text(text) => text.push_str(reminder),
        ExecutableToolOutput::Content(parts) => match parts.last_mut() {
            Some(ContentPart::Text { text }) => text.push_str(reminder),
            _ => parts.push(ContentPart::Text {
                text: reminder.into(),
            }),
        },
    };
    result
}
fn force_stop_result(mut result: ExecutableToolResult, reminder: &str) -> ExecutableToolResult {
    result = append_reminder(result, reminder);
    result.stop_turn = Some(true);
    result
}

pub fn register_agent_tool_dedupe_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TOOL_DEDUPE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let telemetry: TelemetryServiceHandle = (*accessor.get(TELEMETRY_SERVICE_ID)?).clone();
            let loop_service: AgentLoopServiceHandle =
                (*accessor.get(AGENT_LOOP_SERVICE_ID)?).clone();
            let executor: AgentToolExecutorServiceHandle =
                (*accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?).clone();
            let service = AgentToolDedupeService::new(telemetry, loop_service, executor)
                .map_err(|error| DiError::Factory(error.to_string()))?;
            Ok(AgentToolDedupeServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "toolDedupe",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reminders_append_to_text_content_and_preserve_error_state() {
        let result = append_reminder(ExecutableToolResult::error("failed"), " reminder");
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text("failed reminder".into())
        );
        assert!(result.is_error);
        let content = append_reminder(
            ExecutableToolResult::success(ExecutableToolOutput::Content(vec![ContentPart::Text {
                text: "ok".into(),
            }])),
            " reminder",
        );
        assert_eq!(
            content.output,
            ExecutableToolOutput::Content(vec![ContentPart::Text {
                text: "ok reminder".into()
            }])
        );
        assert_eq!(
            args_hash(&serde_json::json!({"b": 2, "a": 1})),
            args_hash(&serde_json::json!({"a": 1, "b": 2}))
        );
    }
}
