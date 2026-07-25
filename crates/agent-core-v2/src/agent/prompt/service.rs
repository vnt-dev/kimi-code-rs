//! Per-agent prompt scheduler.
//!
//! Original: `packages/agent-core-v2/src/agent/prompt/promptService.ts`.

use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use serde_json::{Map, Value};
use tokio::sync::oneshot;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{INSTANTIATION_SERVICE_ID, ServicesAccessorExt},
            instantiation_service::InstantiationService,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options},
        utils::abort::user_cancellation_reason,
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceHandle, ContextMessage,
            PromptOrigin, UndoPrecheck, format_undo_unavailable_message, new_message_id,
            precheck_undo,
        },
        full_compaction::AGENT_FULL_COMPACTION_SERVICE_ID,
        loop_::{
            AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle, AgentLoopState, LoopRunResult,
            LoopValue, StepRequestAdmission, TurnHandle, TurnSeed,
        },
        media::extract_image_compression_captions,
        system_reminder::{AGENT_SYSTEM_REMINDER_SERVICE_ID, AgentSystemReminderServiceHandle},
        tool_executor::{AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle},
    },
    app::event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle},
    kosong::contract::message::{ContentPart, Message, Role},
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    contract::{
        AGENT_PROMPT_SERVICE_ID, AgentPromptHooks, AgentPromptServiceContract,
        AgentPromptServiceHandle, PromptCompletion, PromptCompletionFuture, PromptCompletionState,
        PromptHandle, PromptHandleContract, PromptInput, PromptLaunchedFuture, PromptQueueSnapshot,
        PromptServiceResult, PromptSnapshot, PromptState, PromptSubmitContext,
    },
    errors::{
        PROMPT_NOT_FOUND, REQUEST_INVALID, SESSION_UNDO_UNAVAILABLE,
        ensure_prompt_errors_registered,
    },
    step_requests::{PromptStepRequest, RetryStepRequest, SteerStepRequest},
};

struct Deferred<T: Clone + Send + 'static> {
    sender: Mutex<Option<oneshot::Sender<T>>>,
    future: Shared<BoxFuture<'static, T>>,
}

impl<T: Clone + Send + 'static> Deferred<T> {
    fn new() -> Self {
        let (sender, receiver) = oneshot::channel();
        let future = async move { receiver.await.expect("prompt deferred must settle") }
            .boxed()
            .shared();
        Self {
            sender: Mutex::new(Some(sender)),
            future,
        }
    }
    fn resolve(&self, value: T) {
        if let Some(sender) = self.sender.lock().unwrap().take() {
            let _ = sender.send(value);
        }
    }
}

struct PromptRecord {
    snapshot: Mutex<PromptSnapshot>,
    launched: Deferred<Option<TurnHandle>>,
    completion: Deferred<PromptCompletion>,
}

impl PromptRecord {
    fn snapshot(&self) -> PromptSnapshot {
        self.snapshot.lock().unwrap().clone()
    }
    fn set_state(&self, state: PromptState) {
        self.snapshot.lock().unwrap().state = state;
    }
}

struct RecordHandle(Arc<PromptRecord>);
impl PromptHandleContract for RecordHandle {
    fn snapshot(&self) -> PromptSnapshot {
        self.0.snapshot()
    }
    fn launched(&self) -> PromptLaunchedFuture {
        self.0.launched.future.clone()
    }
    fn completion(&self) -> PromptCompletionFuture {
        self.0.completion.future.clone()
    }
}

#[derive(Default)]
struct SchedulerState {
    active: Option<(Arc<PromptRecord>, TurnHandle)>,
    pending: Vec<Arc<PromptRecord>>,
    steered: HashMap<String, Vec<Arc<PromptRecord>>>,
    launching: bool,
}

struct Runtime {
    state: Mutex<SchedulerState>,
    context: AgentContextMemoryServiceHandle,
    reminders: AgentSystemReminderServiceHandle,
    instantiation: Arc<InstantiationService>,
    loop_service: AgentLoopServiceHandle,
    wire: WireServiceHandle,
    event_bus: EventBusHandle,
    hooks: Arc<AgentPromptHooks>,
}

pub struct AgentPromptService {
    runtime: Arc<Runtime>,
    hooks: Arc<AgentPromptHooks>,
    disposables: DisposableStore,
}

impl AgentPromptService {
    pub fn new(
        context: AgentContextMemoryServiceHandle,
        reminders: AgentSystemReminderServiceHandle,
        instantiation: Arc<InstantiationService>,
        loop_service: AgentLoopServiceHandle,
        tool_executor: AgentToolExecutorServiceHandle,
        wire: WireServiceHandle,
        event_bus: EventBusHandle,
    ) -> Self {
        ensure_prompt_errors_registered();
        let hooks = Arc::new(AgentPromptHooks::default());
        let runtime = Arc::new(Runtime {
            state: Mutex::new(SchedulerState::default()),
            context,
            reminders,
            instantiation,
            loop_service,
            wire,
            event_bus,
            hooks: Arc::clone(&hooks),
        });
        let delivery_runtime = Arc::clone(&runtime);
        let registration = tool_executor
            .hooks()
            .on_did_execute_tool
            .register(
                "prompt-service-delivery",
                Arc::new(move |context, next| {
                    let runtime = Arc::clone(&delivery_runtime);
                    Box::pin(async move {
                        if let Some(delivery) = context.result.delivery.take()
                            && matches!(delivery.kind, crate::tool::ToolDeliveryKind::Steer)
                        {
                            let origin = delivery
                                .message
                                .origin
                                .and_then(|value| serde_json::from_value(value).ok());
                            let message = ContextMessage {
                                message: Message::new(
                                    Role::User,
                                    delivery.message.content,
                                    delivery.message.tool_calls.unwrap_or_default(),
                                ),
                                id: None,
                                provider_message_id: None,
                                origin,
                                is_error: None,
                                note: None,
                            };
                            let _ = inject_runtime(runtime, message).await;
                        }
                        next(context).await
                    })
                }),
                Default::default(),
            )
            .expect("prompt-service-delivery hook registration must succeed");
        let disposables = DisposableStore::new();
        disposables.add(registration);
        Self {
            runtime,
            hooks,
            disposables,
        }
    }

    fn handle(record: Arc<PromptRecord>) -> PromptHandle {
        PromptHandle(Arc::new(RecordHandle(record)))
    }
}

#[async_trait]
impl AgentPromptServiceContract for AgentPromptService {
    async fn enqueue(&self, input: PromptInput) -> PromptServiceResult<PromptHandle> {
        let id = input
            .id
            .or_else(|| input.message.id.clone())
            .unwrap_or_else(new_message_id);
        let mut message = input.message;
        message.id = Some(id.clone());
        let record = Arc::new(PromptRecord {
            snapshot: Mutex::new(PromptSnapshot {
                id: id.clone(),
                user_message_id: id,
                created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                state: PromptState::Pending,
                message,
            }),
            launched: Deferred::new(),
            completion: Deferred::new(),
        });
        let should_start = {
            let mut state = self.runtime.state.lock().unwrap();
            state.pending.push(Arc::clone(&record));
            state.active.is_none() && !state.launching
        };
        if should_start {
            start_next(Arc::clone(&self.runtime)).await;
            tokio::select! { _ = record.launched.future.clone() => {}, _ = record.completion.future.clone() => {} }
        }
        Ok(Self::handle(record))
    }

    fn list(&self) -> PromptQueueSnapshot {
        let state = self.runtime.state.lock().unwrap();
        PromptQueueSnapshot {
            active: state.active.as_ref().map(|(record, _)| record.snapshot()),
            pending: state
                .pending
                .iter()
                .map(|record| record.snapshot())
                .collect(),
        }
    }

    async fn steer(&self, prompt_ids: &[String]) -> PromptServiceResult<Vec<PromptHandle>> {
        if prompt_ids.is_empty() {
            return Err(coded(REQUEST_INVALID, "prompt_ids must not be empty"));
        }
        let (active_id, selected) = {
            let mut state = self.runtime.state.lock().unwrap();
            let active_id = state
                .active
                .as_ref()
                .ok_or_else(|| coded(PROMPT_NOT_FOUND, "no active prompt to steer into"))?
                .0
                .snapshot()
                .id;
            let mut wanted = std::collections::HashSet::new();
            if prompt_ids.iter().any(|id| !wanted.insert(id)) {
                return Err(coded(
                    PROMPT_NOT_FOUND,
                    "one or more prompts are not pending",
                ));
            }
            if state
                .pending
                .iter()
                .filter(|record| wanted.contains(&record.snapshot().id))
                .count()
                != wanted.len()
            {
                return Err(coded(
                    PROMPT_NOT_FOUND,
                    "one or more prompts are not pending",
                ));
            }
            let mut selected = Vec::new();
            state.pending.retain(|record| {
                if wanted.contains(&record.snapshot().id) {
                    selected.push(Arc::clone(record));
                    false
                } else {
                    true
                }
            });
            (active_id, selected)
        };
        let content = selected
            .iter()
            .flat_map(|record| record.snapshot().message.message.content)
            .collect();
        let message = ContextMessage {
            message: Message::new(Role::User, content, Vec::new()),
            id: None,
            provider_message_id: None,
            origin: Some(PromptOrigin::User),
            is_error: None,
            note: None,
        };
        let content = message.message.content.clone();
        let turn = enqueue_steer(
            Arc::clone(&self.runtime),
            message,
            StepRequestAdmission::ActiveTurnOnly,
        )
        .await?;
        let Some(turn) = turn else {
            return Err(coded(PROMPT_NOT_FOUND, "no active turn to steer into"));
        };
        for record in &selected {
            record.set_state(PromptState::Steered);
            record.launched.resolve(Some(turn.clone()));
        }
        self.runtime
            .state
            .lock()
            .unwrap()
            .steered
            .entry(active_id.clone())
            .or_default()
            .extend(selected.iter().cloned());
        self.runtime.event_bus.publish(DomainEvent::new(
            "prompt.steered",
            Map::from_iter([
                ("activePromptId".into(), Value::String(active_id)),
                (
                    "promptIds".into(),
                    Value::Array(
                        selected
                            .iter()
                            .map(|r| Value::String(r.snapshot().id))
                            .collect(),
                    ),
                ),
                ("content".into(), serde_json::to_value(&content)?),
                ("steeredAt".into(), Value::String(now())),
            ]),
        ));
        Ok(selected.into_iter().map(Self::handle).collect())
    }

    fn abort(&self, prompt_id: &str, reason: Option<Arc<dyn Error + Send + Sync>>) -> bool {
        let mut state = self.runtime.state.lock().unwrap();
        if let Some((record, turn)) = state.active.as_ref()
            && record.snapshot().id == prompt_id
        {
            return self.runtime.loop_service.cancel(
                Some(turn.0.id()),
                Some(LoopValue::Error(
                    reason.unwrap_or_else(|| Arc::new(user_cancellation_reason())),
                )),
            );
        }
        let Some(index) = state
            .pending
            .iter()
            .position(|record| record.snapshot().id == prompt_id)
        else {
            return false;
        };
        let record = state.pending.remove(index);
        drop(state);
        record.set_state(PromptState::Cancelled);
        record.launched.resolve(None);
        record.completion.resolve(PromptCompletion {
            prompt_id: prompt_id.into(),
            result: None,
            state: PromptCompletionState::Cancelled,
        });
        publish_aborted(&self.runtime, prompt_id);
        true
    }

    async fn inject(&self, message: ContextMessage) -> PromptServiceResult<Option<TurnHandle>> {
        inject_runtime(Arc::clone(&self.runtime), message).await
    }
    async fn retry(&self) -> PromptServiceResult<Option<TurnHandle>> {
        Ok(self
            .runtime
            .loop_service
            .enqueue(Arc::new(RetryStepRequest::new()), None)?
            .assigned
            .await?
            .turn
            .into())
    }
    fn undo(&self, count: f64) -> PromptServiceResult<usize> {
        if count <= 0.0 {
            return Ok(0);
        }
        let check = precheck_undo(&self.runtime.context.get(), count);
        if let UndoPrecheck::Unavailable {
            reason,
            requested,
            undoable,
        } = check
        {
            let details = Map::from_iter([
                ("reason".into(), Value::String(format!("{reason:?}"))),
                ("requestedCount".into(), Value::from(requested)),
                ("undoableCount".into(), Value::from(undoable as u64)),
            ]);
            return Err(Box::new(Error2::with_options(
                SESSION_UNDO_UNAVAILABLE,
                format_undo_unavailable_message(check).unwrap(),
                Error2Options {
                    details: Some(details),
                    ..Default::default()
                },
            )));
        }
        Ok(self.runtime.context.undo(count)?.removed_count)
    }
    fn clear(&self) -> PromptServiceResult<()> {
        let ids = self
            .runtime
            .state
            .lock()
            .unwrap()
            .pending
            .iter()
            .map(|record| record.snapshot().id)
            .collect::<Vec<_>>();
        for id in ids {
            self.abort(&id, None);
        }
        let active_id = self
            .runtime
            .state
            .lock()
            .unwrap()
            .active
            .as_ref()
            .map(|(record, _)| record.snapshot().id);
        if let Some(id) = active_id {
            self.abort(&id, None);
        }
        self.runtime.context.clear()?;
        Ok(())
    }
    fn hooks(&self) -> &AgentPromptHooks {
        &self.hooks
    }
}

impl Disposable for AgentPromptService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

pub fn register_agent_prompt_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_PROMPT_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let reminders = accessor.get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?;
            let instantiation = accessor.get(INSTANTIATION_SERVICE_ID)?;
            let loop_service = accessor.get(AGENT_LOOP_SERVICE_ID)?;
            let executor = accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?;
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let service: Arc<dyn AgentPromptServiceContract> = Arc::new(AgentPromptService::new(
                (*context).clone(),
                (*reminders).clone(),
                instantiation,
                (*loop_service).clone(),
                (*executor).clone(),
                (*wire).clone(),
                (*event_bus).clone(),
            ));
            Ok(AgentPromptServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "prompt",
    );
}

fn coded(code: &str, message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(Error2::new(code, message))
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn start_next(runtime: Arc<Runtime>) -> BoxFuture<'static, ()> {
    Box::pin(async move {
        let record = {
            let mut state = runtime.state.lock().unwrap();
            if state.active.is_some() || state.launching {
                return;
            }
            let Some(record) = state.pending.first().cloned() else {
                return;
            };
            state.launching = true;
            state.pending.remove(0);
            record
        };
        let compaction = runtime
            .instantiation
            .get(AGENT_FULL_COMPACTION_SERVICE_ID)
            .ok();
        if compaction
            .as_ref()
            .is_some_and(|service| service.compacting().is_some())
            && runtime.loop_service.status().state != AgentLoopState::Running
        {
            let mut state = runtime.state.lock().unwrap();
            state.pending.insert(0, record);
            state.launching = false;
            return;
        }
        let snapshot = record.snapshot();
        let extracted = extract_message(&snapshot.message);
        let mut hook = PromptSubmitContext {
            prompt_message: extracted.0.clone(),
            is_steer: false,
            block: false,
        };
        let outcome: Result<(), _> = runtime
            .hooks
            .on_before_submit_prompt
            .run(&mut hook, None)
            .await;
        if outcome.is_err() {
            fail_record(&runtime, &record);
        } else if hook.block {
            append_prompt(&runtime, &hook.prompt_message, &extracted.1);
            record.set_state(PromptState::Blocked);
            record.launched.resolve(None);
            record.completion.resolve(PromptCompletion {
                prompt_id: snapshot.id.clone(),
                result: None,
                state: PromptCompletionState::Blocked,
            });
            publish_completed(&runtime, &snapshot.id, "blocked");
        } else {
            match runtime.loop_service.enqueue(
                Arc::new(PromptStepRequest::new(
                    hook.prompt_message,
                    extracted.1,
                    runtime.reminders.0.clone(),
                )),
                None,
            ) {
                Ok(receipt) => match receipt.assigned.await {
                    Ok(assignment) => {
                        let turn = assignment.turn;
                        record.set_state(PromptState::Running);
                        record.launched.resolve(Some(turn.clone()));
                        runtime.state.lock().unwrap().active =
                            Some((Arc::clone(&record), turn.clone()));
                        let settle_runtime = Arc::clone(&runtime);
                        tokio::spawn(async move {
                            settle(settle_runtime, record, turn.0.result().await);
                        });
                    }
                    Err(_) => fail_record(&runtime, &record),
                },
                Err(_) => fail_record(&runtime, &record),
            }
        }
        {
            runtime.state.lock().unwrap().launching = false;
        }
        if runtime.state.lock().unwrap().active.is_none() {
            tokio::spawn(start_next(runtime));
        }
    })
}

fn settle(runtime: Arc<Runtime>, record: Arc<PromptRecord>, result: LoopRunResult) {
    let id = record.snapshot().id;
    {
        let mut state = runtime.state.lock().unwrap();
        if state
            .active
            .as_ref()
            .is_none_or(|(active, _)| active.snapshot().id != id)
        {
            return;
        }
        state.active = None;
        let children = state.steered.remove(&id).unwrap_or_default();
        let completion = match result {
            LoopRunResult::Completed { .. } => PromptCompletionState::Completed,
            LoopRunResult::Failed { .. } => PromptCompletionState::Failed,
            LoopRunResult::Cancelled { .. } => PromptCompletionState::Cancelled,
        };
        record.set_state(match completion {
            PromptCompletionState::Completed => PromptState::Completed,
            PromptCompletionState::Failed => PromptState::Failed,
            PromptCompletionState::Cancelled => PromptState::Cancelled,
            PromptCompletionState::Blocked => PromptState::Blocked,
        });
        record.completion.resolve(PromptCompletion {
            prompt_id: id.clone(),
            result: Some(result.clone()),
            state: completion,
        });
        for child in children {
            child.set_state(record.snapshot().state);
            child.completion.resolve(PromptCompletion {
                prompt_id: child.snapshot().id,
                result: Some(result.clone()),
                state: completion,
            });
        }
        if completion == PromptCompletionState::Cancelled {
            publish_aborted(&runtime, &id);
        } else {
            publish_completed(
                &runtime,
                &id,
                if completion == PromptCompletionState::Completed {
                    "completed"
                } else {
                    "failed"
                },
            );
        }
    }
    tokio::spawn(start_next(runtime));
}

fn fail_record(runtime: &Arc<Runtime>, record: &Arc<PromptRecord>) {
    let id = record.snapshot().id;
    record.set_state(PromptState::Failed);
    record.launched.resolve(None);
    record.completion.resolve(PromptCompletion {
        prompt_id: id.clone(),
        result: None,
        state: PromptCompletionState::Failed,
    });
    publish_completed(runtime, &id, "failed");
}
fn extract_message(message: &ContextMessage) -> (ContextMessage, Vec<String>) {
    if !matches!(message.origin, None | Some(PromptOrigin::User)) {
        return (message.clone(), Vec::new());
    }
    let mut result = message.clone();
    let mut captions = Vec::new();
    result.message.content = result
        .message
        .content
        .into_iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => {
                let extracted = extract_image_compression_captions(&text);
                captions.extend(extracted.captions);
                (!extracted.text.trim().is_empty()).then_some(ContentPart::Text {
                    text: extracted.text,
                })
            }
            other => Some(other),
        })
        .collect();
    (result, captions)
}
fn append_prompt(runtime: &Runtime, message: &ContextMessage, captions: &[String]) {
    for caption in captions {
        let _ = runtime.reminders.append_system_reminder(
            caption,
            PromptOrigin::Injection {
                variant: "image_compression".into(),
            },
        );
    }
    if !message.message.content.is_empty() {
        let _ = runtime.context.append(vec![message.clone()]);
    }
}
async fn enqueue_steer(
    runtime: Arc<Runtime>,
    message: ContextMessage,
    admission: StepRequestAdmission,
) -> PromptServiceResult<Option<TurnHandle>> {
    let (message, captions) = extract_message(&message);
    let wire = runtime.wire.clone();
    let request = SteerStepRequest::new(
        message,
        captions,
        runtime.reminders.0.clone(),
        Arc::new(move |materialized| {
            let seed = TurnSeed {
                input: materialized.message.content.clone(),
                origin: materialized.origin.clone().unwrap_or(PromptOrigin::User),
            };
            if let Ok(operation) = crate::agent::loop_::steer_turn(seed) {
                let _ = wire.dispatch([operation]);
            }
        }),
        Arc::new(|| {}),
        Some(admission),
    );
    Ok(Some(
        runtime
            .loop_service
            .enqueue(Arc::new(request), None)?
            .assigned
            .await?
            .turn,
    ))
}
async fn inject_runtime(
    runtime: Arc<Runtime>,
    message: ContextMessage,
) -> PromptServiceResult<Option<TurnHandle>> {
    enqueue_steer(runtime, message, StepRequestAdmission::ActiveOrNewTurn).await
}
fn publish_completed(runtime: &Runtime, prompt_id: &str, reason: &str) {
    runtime.event_bus.publish(DomainEvent::new(
        "prompt.completed",
        Map::from_iter([
            ("promptId".into(), Value::String(prompt_id.into())),
            ("finishedAt".into(), Value::String(now())),
            ("reason".into(), Value::String(reason.into())),
        ]),
    ));
}
fn publish_aborted(runtime: &Runtime, prompt_id: &str) {
    runtime.event_bus.publish(DomainEvent::new(
        "prompt.aborted",
        Map::from_iter([
            ("promptId".into(), Value::String(prompt_id.into())),
            ("abortedAt".into(), Value::String(now())),
        ]),
    ));
}
