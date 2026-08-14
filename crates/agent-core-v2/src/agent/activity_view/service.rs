//! Event-folded agent activity view.
//!
//! Original:
//! `packages/agent-core-v2/src/agent/activityView/activityViewService.ts`.

use parking_lot::Mutex;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        lifecycle::{Disposable, DisposableStore, DisposeResult},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::{
        context_memory::PromptOrigin,
        full_compaction::{AGENT_FULL_COMPACTION_SERVICE_ID, AgentFullCompactionServiceHandle},
        loop_::{
            AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle, AgentLoopState, AgentLoopStatus,
            AssistantDeltaEvent, ThinkingDeltaEvent, ToolCallDeltaEvent, TurnEndReason,
            TurnEndedEvent, TurnStartedEvent, TurnStepInterruptedEvent, TurnStepStartedEvent,
        },
        task::{AGENT_TASK_SERVICE_ID, AgentTaskInfo, AgentTaskServiceHandle},
    },
    app::event::event_bus::{
        DomainEvent, DomainEventPayload, EVENT_BUS_SERVICE_ID, EventBusHandle, TypedEventBusExt,
    },
};

use super::{
    AGENT_ACTIVITY_VIEW_ID, ActivityEndingReason, ActivityLastTurnState, ActivityRetryState,
    ActivityStream, ActivityTurnState, ActivityViewLifecycle, AgentActivityState,
    AgentActivityViewContract, AgentActivityViewHandle, ApprovalRef, BackgroundRef, ToolCallRef,
    TurnPhase,
};

const FULL_COMPACTION_BACKGROUND_ID: &str = "full-compaction";

struct MutableTurn {
    turn_id: crate::agent::TurnId,
    origin: PromptOrigin,
    phase: TurnPhase,
    stream: Option<ActivityStream>,
    step: crate::agent::StepId,
    ending: bool,
    ending_reason: Option<ActivityEndingReason>,
    retry: Option<ActivityRetryState>,
    pending_approvals: IndexMap<String, ApprovalRef>,
    active_tool_calls: IndexMap<String, ToolCallRef>,
    since: i64,
}

impl MutableTurn {
    fn new(turn_id: crate::agent::TurnId, origin: PromptOrigin) -> Self {
        Self {
            turn_id,
            origin,
            phase: TurnPhase::Running,
            stream: None,
            step: crate::agent::StepId::new(0),
            ending: false,
            ending_reason: None,
            retry: None,
            pending_approvals: IndexMap::new(),
            active_tool_calls: IndexMap::new(),
            since: epoch_millis(),
        }
    }

    fn snapshot(&self) -> ActivityTurnState {
        ActivityTurnState {
            turn_id: self.turn_id,
            origin: self.origin.clone(),
            phase: self.phase,
            stream: self.stream,
            step: self.step,
            ending: self.ending,
            ending_reason: self.ending_reason,
            retry: self.retry.clone(),
            pending_approvals: self.pending_approvals.values().cloned().collect(),
            active_tool_calls: self.active_tool_calls.values().cloned().collect(),
            since: self.since,
        }
    }
}

struct FoldState {
    lifecycle: ActivityViewLifecycle,
    turn: Option<MutableTurn>,
    last_turn: Option<ActivityLastTurnState>,
    background: IndexMap<String, BackgroundRef>,
    current: AgentActivityState,
}

impl Default for FoldState {
    fn default() -> Self {
        Self {
            lifecycle: ActivityViewLifecycle::Ready,
            turn: None,
            last_turn: None,
            background: IndexMap::new(),
            current: AgentActivityState::default(),
        }
    }
}

pub struct AgentActivityView {
    event_bus: EventBusHandle,
    fold: Mutex<FoldState>,
    disposables: DisposableStore,
}

impl AgentActivityView {
    pub fn new(
        event_bus: EventBusHandle,
        loop_service: AgentLoopServiceHandle,
        tasks: AgentTaskServiceHandle,
        full_compaction: AgentFullCompactionServiceHandle,
    ) -> Arc<Self> {
        let service = Self::build(event_bus);
        service.seed_from_loop_status(loop_service.status());
        service.seed_from_task_infos(tasks.list(Some(true), None));
        service.seed_from_full_compaction_state(full_compaction.compacting().is_some());
        service.install_listeners();
        service
    }

    fn build(event_bus: EventBusHandle) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            fold: Mutex::new(FoldState::default()),
            disposables: DisposableStore::new(),
        })
    }

    fn seed_from_loop_status(&self, status: AgentLoopStatus) {
        if status.state != AgentLoopState::Running {
            return;
        }
        let Some(turn_id) = status.active_turn_id else {
            return;
        };
        self.fold.lock().turn = Some(MutableTurn::new(turn_id, PromptOrigin::User));
        self.publish();
    }

    fn seed_from_task_infos(&self, infos: Vec<AgentTaskInfo>) {
        if infos.is_empty() {
            return;
        }
        let mut fold = self.fold.lock();
        for info in infos {
            fold.background.insert(
                info.base.task_id.clone(),
                BackgroundRef {
                    kind: info.kind,
                    id: info.base.task_id,
                    since: info.base.started_at,
                },
            );
        }
        drop(fold);
        self.publish();
    }

    fn seed_from_full_compaction_state(&self, compacting: bool) {
        if !compacting {
            return;
        }
        self.fold.lock().background.insert(
            FULL_COMPACTION_BACKGROUND_ID.into(),
            BackgroundRef {
                kind: "compaction".into(),
                id: FULL_COMPACTION_BACKGROUND_ID.into(),
                since: epoch_millis(),
            },
        );
        self.publish();
    }

    fn install_listeners(self: &Arc<Self>) {
        self.subscribe_typed(|service, event: &TurnStartedEvent| {
            service.on_turn_started(event.turn_id, event.origin.clone());
        });
        self.subscribe_typed(|service, event: &TurnStepStartedEvent| {
            service.on_step_started(event.step);
        });
        self.subscribe_typed(|service, _: &AssistantDeltaEvent| {
            service.on_delta(ActivityStream::Assistant);
        });
        self.subscribe_typed(|service, _: &ThinkingDeltaEvent| {
            service.on_delta(ActivityStream::Thinking);
        });
        self.subscribe_typed(|service, _: &ToolCallDeltaEvent| {
            service.on_delta(ActivityStream::ToolCall);
        });
        self.subscribe("tool.call.started", |service, event| {
            let Some(tool_call_id) = string_field(&event.fields, "toolCallId") else {
                return;
            };
            let Some(name) = string_field(&event.fields, "name") else {
                return;
            };
            service.on_tool_call_started(tool_call_id, name);
        });
        self.subscribe("tool.result", |service, event| {
            if let Some(tool_call_id) = string_field(&event.fields, "toolCallId") {
                service.on_tool_result(tool_call_id);
            }
        });
        self.subscribe("turn.step.retrying", |service, event| {
            let Ok(event) =
                serde_json::from_value::<RetryingEvent>(Value::Object(event.fields().clone()))
            else {
                return;
            };
            service.mutate_turn(|turn| {
                turn.phase = TurnPhase::Retrying;
                turn.stream = None;
                turn.retry = Some(ActivityRetryState {
                    failed_attempt: event.failed_attempt,
                    next_attempt: event.next_attempt,
                    max_attempts: event.max_attempts,
                    delay_ms: event.delay_ms,
                    error_name: event.error_name,
                    status_code: event.status_code,
                });
            });
        });
        self.subscribe("turn.step.completed", |service, _| {
            service.mutate_turn(|turn| {
                turn.phase = TurnPhase::Running;
                turn.stream = None;
                turn.retry = None;
            });
        });
        self.subscribe_typed(|service, event: &TurnStepInterruptedEvent| {
            service.on_step_interrupted(event.turn_id, &event.reason);
        });
        self.subscribe_typed(|service, event: &TurnEndedEvent| {
            service.on_turn_ended(event.turn_id, event.reason);
        });
        self.subscribe("permission.approval.requested", |service, event| {
            if let Some(tool_call_id) = string_field(&event.fields, "toolCallId") {
                service.on_approval_requested(tool_call_id);
            }
        });
        self.subscribe("permission.approval.resolved", |service, event| {
            if let Some(tool_call_id) = string_field(&event.fields, "toolCallId") {
                service.on_approval_resolved(tool_call_id);
            }
        });
        self.subscribe("task.started", |service, event| {
            if let Some(info) = task_info(event) {
                let mut fold = service.fold.lock();
                fold.background.insert(
                    info.base.task_id.clone(),
                    BackgroundRef {
                        kind: info.kind,
                        id: info.base.task_id,
                        since: info.base.started_at,
                    },
                );
                drop(fold);
                service.publish();
            }
        });
        self.subscribe("task.terminated", |service, event| {
            let Some(info) = task_info(event) else {
                return;
            };
            if service
                .fold
                .lock()
                .background
                .shift_remove(&info.base.task_id)
                .is_some()
            {
                service.publish();
            }
        });
        self.subscribe("compaction.started", |service, _| {
            service.fold.lock().background.insert(
                FULL_COMPACTION_BACKGROUND_ID.into(),
                BackgroundRef {
                    kind: "compaction".into(),
                    id: FULL_COMPACTION_BACKGROUND_ID.into(),
                    since: epoch_millis(),
                },
            );
            service.publish();
        });
        self.subscribe("compaction.completed", |service, _| {
            service.on_full_compaction_ended();
        });
        self.subscribe("compaction.cancelled", |service, _| {
            service.on_full_compaction_ended();
        });
    }

    fn subscribe(
        self: &Arc<Self>,
        event_type: &str,
        handler: impl Fn(&Self, &DomainEvent) + Send + Sync + 'static,
    ) {
        let weak: Weak<Self> = Arc::downgrade(self);
        self.disposables.add(self.event_bus.subscribe_type(
            event_type,
            Arc::new(move |event| {
                if let Some(service) = weak.upgrade() {
                    handler(&service, event);
                }
            }),
        ));
    }

    fn subscribe_typed<T>(self: &Arc<Self>, handler: impl Fn(&Self, &T) + Send + Sync + 'static)
    where
        T: DomainEventPayload,
    {
        let weak: Weak<Self> = Arc::downgrade(self);
        self.disposables
            .add(self.event_bus.subscribe_typed(Arc::new(move |event| {
                if let Some(service) = weak.upgrade() {
                    handler(&service, event);
                }
            })));
    }

    fn on_full_compaction_ended(&self) {
        if self
            .fold
            .lock()
            .background
            .shift_remove(FULL_COMPACTION_BACKGROUND_ID)
            .is_some()
        {
            self.publish();
        }
    }

    fn on_turn_started(&self, turn_id: crate::agent::TurnId, origin: PromptOrigin) {
        let mut fold = self.fold.lock();
        fold.turn = Some(MutableTurn::new(turn_id, origin));
        fold.last_turn = None;
        drop(fold);
        self.publish();
    }

    fn on_turn_ended(&self, turn_id: crate::agent::TurnId, reason: TurnEndReason) {
        let mut fold = self.fold.lock();
        let at = epoch_millis();
        if fold
            .turn
            .as_ref()
            .is_none_or(|turn| turn.turn_id != turn_id)
        {
            fold.last_turn = Some(ActivityLastTurnState {
                turn_id,
                reason,
                duration_ms: None,
                at,
            });
        } else {
            let since = fold.turn.as_ref().expect("matching turn exists").since;
            fold.last_turn = Some(ActivityLastTurnState {
                turn_id,
                reason,
                duration_ms: Some(epoch_millis() - since),
                at,
            });
            fold.turn = None;
        }
        drop(fold);
        self.publish();
    }

    fn on_step_started(&self, step: crate::agent::StepId) {
        self.mutate_turn(|turn| {
            turn.step = step;
            turn.phase = TurnPhase::Running;
            turn.stream = None;
            turn.retry = None;
        });
    }

    fn on_step_interrupted(&self, turn_id: crate::agent::TurnId, reason: &str) {
        let ending_reason = match reason {
            "aborted" => ActivityEndingReason::Aborted,
            "max_steps" => ActivityEndingReason::MaxSteps,
            "error" => ActivityEndingReason::Error,
            _ => return,
        };
        self.mutate_turn(|turn| {
            if turn.turn_id == turn_id {
                turn.ending = true;
                turn.ending_reason = Some(ending_reason);
            }
        });
    }

    fn on_delta(&self, stream: ActivityStream) {
        self.mutate_turn(|turn| {
            turn.phase = TurnPhase::Streaming;
            turn.stream = Some(stream);
            turn.retry = None;
        });
    }

    fn on_tool_call_started(&self, tool_call_id: String, name: String) {
        self.mutate_turn(|turn| {
            turn.phase = TurnPhase::ToolCall;
            turn.stream = None;
            turn.retry = None;
            turn.active_tool_calls.insert(
                tool_call_id.clone(),
                ToolCallRef {
                    tool_call_id,
                    name,
                    since: epoch_millis(),
                },
            );
        });
    }

    fn on_tool_result(&self, tool_call_id: String) {
        self.mutate_turn(|turn| {
            turn.active_tool_calls.shift_remove(&tool_call_id);
            turn.phase = if turn.active_tool_calls.is_empty() {
                TurnPhase::Running
            } else {
                TurnPhase::ToolCall
            };
            turn.stream = None;
            turn.retry = None;
        });
    }

    fn on_approval_requested(&self, tool_call_id: String) {
        self.mutate_turn(|turn| {
            turn.pending_approvals.insert(
                tool_call_id.clone(),
                ApprovalRef {
                    approval_id: tool_call_id.clone(),
                    tool_call_id: Some(tool_call_id),
                    since: epoch_millis(),
                },
            );
        });
    }

    fn on_approval_resolved(&self, tool_call_id: String) {
        self.mutate_turn(|turn| {
            turn.pending_approvals.shift_remove(&tool_call_id);
        });
    }

    fn mutate_turn(&self, mutate: impl FnOnce(&mut MutableTurn)) {
        let mut fold = self.fold.lock();
        let Some(turn) = fold.turn.as_mut() else {
            return;
        };
        mutate(turn);
        drop(fold);
        self.publish();
    }

    fn publish(&self) {
        let next = {
            let mut fold = self.fold.lock();
            let next = AgentActivityState {
                lifecycle: fold.lifecycle,
                turn: fold.turn.as_ref().map(MutableTurn::snapshot),
                last_turn: fold.last_turn.clone(),
                background: fold.background.values().cloned().collect(),
            };
            if activity_equal(&fold.current, &next) {
                return;
            }
            fold.current = next.clone();
            next
        };
        let Value::Object(fields) =
            serde_json::to_value(next).expect("agent activity state serializes")
        else {
            unreachable!("agent activity state is an object");
        };
        self.event_bus
            .publish(DomainEvent::new("agent.activity.updated", fields));
    }
}

impl AgentActivityViewContract for AgentActivityView {
    fn state(&self) -> AgentActivityState {
        self.fold.lock().current.clone()
    }
}

impl Disposable for AgentActivityView {
    fn dispose(&self) -> DisposeResult {
        self.fold.lock().lifecycle = ActivityViewLifecycle::Disposed;
        self.publish();
        self.disposables.dispose()
    }
}

impl Drop for AgentActivityView {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

pub fn register_agent_activity_view_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_ACTIVITY_VIEW_ID,
        SyncDescriptor::new(|accessor| {
            let service: Arc<dyn AgentActivityViewContract> = AgentActivityView::new(
                (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_LOOP_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TASK_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_FULL_COMPACTION_SERVICE_ID)?).clone(),
            );
            Ok(AgentActivityViewHandle(service))
        })
        .disposable(),
        InstantiationType::Delayed,
        "activityView",
    );
}

fn activity_equal(left: &AgentActivityState, right: &AgentActivityState) -> bool {
    if left.lifecycle != right.lifecycle
        || left.turn.is_some() != right.turn.is_some()
        || left.last_turn.is_some() != right.last_turn.is_some()
        || left.background.len() != right.background.len()
    {
        return false;
    }
    if let (Some(left), Some(right)) = (&left.turn, &right.turn)
        && (left.turn_id != right.turn_id
            || left.phase != right.phase
            || left.stream != right.stream
            || left.step != right.step
            || left.ending != right.ending
            || left.ending_reason != right.ending_reason
            || left.pending_approvals.len() != right.pending_approvals.len()
            || left.active_tool_calls.len() != right.active_tool_calls.len()
            || left.retry.as_ref().map(|retry| retry.next_attempt)
                != right.retry.as_ref().map(|retry| retry.next_attempt))
    {
        return false;
    }
    if let (Some(left), Some(right)) = (&left.last_turn, &right.last_turn)
        && (left.turn_id != right.turn_id || left.reason != right.reason)
    {
        return false;
    }
    left.background
        .iter()
        .zip(&right.background)
        .all(|(left, right)| left.id == right.id && left.kind == right.kind)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetryingEvent {
    failed_attempt: u64,
    next_attempt: u64,
    max_attempts: u64,
    delay_ms: u64,
    #[serde(default)]
    error_name: Option<String>,
    #[serde(default)]
    status_code: Option<u16>,
}

#[derive(Deserialize)]
struct TaskInfoEvent {
    info: AgentTaskInfo,
}

fn task_info(event: &DomainEvent) -> Option<AgentTaskInfo> {
    serde_json::from_value::<TaskInfoEvent>(Value::Object(event.fields().clone()))
        .ok()
        .map(|event| event.info)
}

fn string_field(fields: &Map<String, Value>, key: &str) -> Option<String> {
    fields.get(key)?.as_str().map(str::to_owned)
}

fn epoch_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use crate::{
        _base::di::lifecycle::DisposableHandle,
        agent::{
            loop_::AgentLoopState,
            task::{AgentTaskInfoBase, AgentTaskStatus},
        },
        app::event::{event_bus::EventBusContract, event_bus_service::EventBusService},
    };
    use serde_json::json;

    use super::*;

    type Harness = (
        Arc<EventBusService>,
        Arc<AgentActivityView>,
        Arc<Mutex<Vec<DomainEvent>>>,
        DisposableHandle,
    );

    fn task(task_id: &str) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: task_id.into(),
                description: "sleep 60".into(),
                status: AgentTaskStatus::Running,
                detached: Some(true),
                started_at: 100,
                ended_at: None,
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::new(),
        }
    }

    fn publish(bus: &EventBusService, event_type: &str, fields: Value) {
        let Value::Object(fields) = fields else {
            panic!("test event fields must be an object");
        };
        bus.publish(DomainEvent::new(event_type, fields));
    }

    fn harness(
        seed_tasks: Vec<AgentTaskInfo>,
        compacting: bool,
        loop_status: Option<AgentLoopStatus>,
    ) -> Harness {
        let bus = Arc::new(EventBusService::new());
        let updates = Arc::new(Mutex::new(Vec::new()));
        let updates_for_listener = Arc::clone(&updates);
        let registration = bus.subscribe(Arc::new(move |event| {
            updates_for_listener.lock().push(event.clone());
        }));
        let service = AgentActivityView::build(EventBusHandle(bus.clone()));
        if let Some(status) = loop_status {
            service.seed_from_loop_status(status);
        }
        service.seed_from_task_infos(seed_tasks);
        service.seed_from_full_compaction_state(compacting);
        service.install_listeners();
        (bus, service, updates, registration)
    }

    fn state_with_turn() -> AgentActivityState {
        AgentActivityState {
            turn: Some(
                MutableTurn::new(crate::agent::TurnId::new(1), PromptOrigin::User).snapshot(),
            ),
            ..AgentActivityState::default()
        }
    }

    #[test]
    fn equality_deliberately_compares_only_source_projection_keys() {
        let left = state_with_turn();
        let mut right = left.clone();
        right.turn.as_mut().unwrap().since += 10;
        right.turn.as_mut().unwrap().origin = PromptOrigin::SystemTrigger {
            name: "different".into(),
        };
        assert!(activity_equal(&left, &right));

        right.turn.as_mut().unwrap().step = crate::agent::StepId::new(1);
        assert!(!activity_equal(&left, &right));
    }

    #[test]
    fn starts_empty_and_seeds_loop_tasks_and_compaction() {
        let (_, empty, _, _) = harness(Vec::new(), false, None);
        assert_eq!(empty.state(), AgentActivityState::default());

        let (_, seeded, updates, _) = harness(
            vec![task("bash-9")],
            true,
            Some(AgentLoopStatus {
                state: AgentLoopState::Running,
                active_turn_id: Some(crate::agent::TurnId::new(7)),
                pending_turn_ids: Vec::new(),
                has_pending_requests: false,
                active_trace_id: None,
            }),
        );
        let state = seeded.state();
        assert_eq!(
            state.turn.as_ref().unwrap().turn_id,
            crate::agent::TurnId::new(7)
        );
        assert_eq!(state.turn.as_ref().unwrap().origin, PromptOrigin::User);
        assert_eq!(
            state
                .background
                .iter()
                .map(|entry| (entry.kind.as_str(), entry.id.as_str()))
                .collect::<Vec<_>>(),
            [
                ("process", "bash-9"),
                ("compaction", FULL_COMPACTION_BACKGROUND_ID)
            ]
        );
        assert_eq!(
            updates
                .lock()
                .iter()
                .filter(|event| event.event_type == "agent.activity.updated")
                .count(),
            3
        );
    }

    #[test]
    fn folds_task_and_compaction_background_events() {
        let (bus, view, _, _) = harness(Vec::new(), false, None);
        publish(
            &bus,
            "task.started",
            json!({"info": serde_json::to_value(task("bash-1")).unwrap()}),
        );
        assert_eq!(
            view.state().background,
            [BackgroundRef {
                kind: "process".into(),
                id: "bash-1".into(),
                since: 100
            }]
        );
        publish(
            &bus,
            "task.terminated",
            json!({"info": serde_json::to_value(task("bash-1")).unwrap()}),
        );
        assert!(view.state().background.is_empty());

        publish(&bus, "compaction.started", json!({"trigger": "manual"}));
        assert_eq!(view.state().background[0].id, FULL_COMPACTION_BACKGROUND_ID);
        publish(&bus, "compaction.cancelled", json!({}));
        assert!(view.state().background.is_empty());
    }

    #[test]
    fn folds_complete_live_turn_detail_and_clears_previous_outcome() {
        let (bus, view, _, _) = harness(Vec::new(), false, None);
        publish(
            &bus,
            "turn.started",
            json!({"turnId": 1, "origin": {"kind": "user"}}),
        );
        publish(&bus, "turn.step.started", json!({"turnId": 1, "step": 2}));
        publish(&bus, "assistant.delta", json!({"turnId": 1, "delta": "a"}));
        let turn = view.state().turn.unwrap();
        assert_eq!(turn.step, crate::agent::StepId::new(2));
        assert_eq!(turn.phase, TurnPhase::Streaming);
        assert_eq!(turn.stream, Some(ActivityStream::Assistant));

        publish(
            &bus,
            "turn.step.retrying",
            json!({
                "failedAttempt": 1,
                "nextAttempt": 2,
                "maxAttempts": 3,
                "delayMs": 500,
                "errorName": "ProviderError",
                "statusCode": 429
            }),
        );
        assert_eq!(view.state().turn.unwrap().retry.unwrap().next_attempt, 2);
        publish(&bus, "turn.step.completed", json!({}));
        assert_eq!(view.state().turn.unwrap().phase, TurnPhase::Running);

        publish(
            &bus,
            "tool.call.started",
            json!({"toolCallId": "call-1", "name": "Read"}),
        );
        publish(
            &bus,
            "permission.approval.requested",
            json!({"toolCallId": "call-1"}),
        );
        let turn = view.state().turn.unwrap();
        assert_eq!(turn.active_tool_calls[0].name, "Read");
        assert_eq!(turn.pending_approvals[0].approval_id, "call-1");

        publish(&bus, "tool.result", json!({"toolCallId": "call-1"}));
        publish(
            &bus,
            "permission.approval.resolved",
            json!({"toolCallId": "call-1"}),
        );
        publish(
            &bus,
            "turn.step.interrupted",
            json!({"turnId": 1, "step": 2, "reason": "max_steps"}),
        );
        let turn = view.state().turn.unwrap();
        assert!(turn.active_tool_calls.is_empty());
        assert!(turn.pending_approvals.is_empty());
        assert!(turn.ending);
        assert_eq!(turn.ending_reason, Some(ActivityEndingReason::MaxSteps));

        publish(
            &bus,
            "turn.ended",
            json!({"turnId": 1, "reason": "cancelled"}),
        );
        assert!(view.state().turn.is_none());
        assert_eq!(
            view.state().last_turn.unwrap().reason,
            TurnEndReason::Cancelled
        );

        publish(
            &bus,
            "turn.started",
            json!({"turnId": 2, "origin": {"kind": "user"}}),
        );
        assert!(view.state().last_turn.is_none());
    }

    #[test]
    fn records_unseen_turn_outcomes_and_publishes_disposal() {
        let (bus, view, updates, _) = harness(Vec::new(), false, None);
        publish(
            &bus,
            "turn.ended",
            json!({"turnId": 99, "reason": "completed"}),
        );
        let last = view.state().last_turn.unwrap();
        assert_eq!(last.turn_id, crate::agent::TurnId::new(99));
        assert_eq!(last.duration_ms, None);

        view.dispose().unwrap();
        assert_eq!(view.state().lifecycle, ActivityViewLifecycle::Disposed);
        assert!(updates.lock().iter().any(|event| {
            event.event_type == "agent.activity.updated"
                && event.fields.get("lifecycle") == Some(&json!("disposed"))
        }));
    }

    #[test]
    fn registration_is_delayed_agent_scoped_with_source_domain() {
        register_agent_activity_view_service();
        let descriptor =
            crate::_base::di::scope::get_scoped_service_descriptors(LifecycleScope::Agent)
                .into_iter()
                .find(|entry| entry.id.to_string() == AGENT_ACTIVITY_VIEW_ID.to_string())
                .expect("activity view service is registered");
        assert!(descriptor.descriptor.supports_delayed_instantiation);
        assert_eq!(descriptor.domain, "activityView");
    }
}
