//! Ownership-driven prompt scheduler actor.
//!
//! `SchedulerState` always moves together with `SchedulerActor`.  While running,
//! exactly one task owns it; while idle, the controller parks the whole Actor as
//! an opaque value and never accesses its state.  No worker receives a mutable
//! reference: hook execution, loop assignment, turn monitoring and steer
//! assignment report results through `SchedulerEvent`.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use chrono::Utc;
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    _base::{
        di::{instantiation_service::InstantiationService, lifecycle::DisposableStore},
        errors::errors::{Error2, Error2Options},
        utils::abort::{abort_error, user_cancellation_reason},
    },
    agent::{
        context_memory::{
            AgentContextMemoryServiceHandle, ContextMessage, PromptOrigin, UndoPrecheck,
            format_undo_unavailable_message, new_message_id, precheck_undo,
            to_protocol_message_content,
        },
        full_compaction::{AGENT_FULL_COMPACTION_SERVICE_ID, AgentFullCompactionServiceHandle},
        loop_::{
            AgentLoopServiceHandle, AgentLoopState, LiveUserMessage, LoopRunResult, LoopValue,
            StepRequestAdmission, TurnHandle, TurnSeed,
        },
        media::extract_image_compression_captions,
        system_reminder::AgentSystemReminderServiceHandle,
    },
    app::event::event_bus::{DomainEvent, EventBusHandle, TypedEventBusExt},
    kosong::contract::message::{ContentPart, Message, Role},
    wire::contract::WireServiceHandle,
};

use super::{
    contract::{
        AgentPromptHooks, PromptCompletion, PromptCompletionFuture, PromptCompletionState,
        PromptHandle, PromptHandleContract, PromptInput, PromptLaunchedFuture, PromptQueueSnapshot,
        PromptServiceResult, PromptSnapshot, PromptState, PromptSubmitContext,
        PromptSubmittedEvent, PromptSubmittedStatus,
    },
    errors::{PROMPT_NOT_FOUND, REQUEST_INVALID, SESSION_UNDO_UNAVAILABLE},
    step_requests::{PromptStepRequest, SteerStepRequest},
};

const COMMAND_CAPACITY: usize = 64;
const ACTOR_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

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

pub(super) struct PromptRecord {
    snapshot: Mutex<PromptSnapshot>,
    user_message: LiveUserMessage,
    launched: Deferred<Option<TurnHandle>>,
    completion: Deferred<PromptCompletion>,
    cancellation: CancellationToken,
}

impl PromptRecord {
    fn snapshot(&self) -> PromptSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    fn set_state(&self, state: PromptState) {
        self.snapshot.lock().unwrap().state = state;
    }

    /// Terminal settlement is idempotent because clear/shutdown may race with a
    /// worker event that was already queued before cancellation became visible.
    fn set_terminal_state(&self, state: PromptState) -> bool {
        let mut snapshot = self.snapshot.lock().unwrap();
        if matches!(
            snapshot.state,
            PromptState::Completed
                | PromptState::Failed
                | PromptState::Cancelled
                | PromptState::Blocked
        ) {
            return false;
        }
        snapshot.state = state;
        true
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

fn prompt_handle(record: Arc<PromptRecord>) -> PromptHandle {
    PromptHandle(Arc::new(RecordHandle(record)))
}

pub(super) struct SchedulerRuntime {
    pub(super) context: AgentContextMemoryServiceHandle,
    pub(super) reminders: AgentSystemReminderServiceHandle,
    pub(super) instantiation: Arc<InstantiationService>,
    full_compaction: Mutex<Option<AgentFullCompactionServiceHandle>>,
    pub(super) loop_service: AgentLoopServiceHandle,
    pub(super) wire: WireServiceHandle,
    event_bus: EventBusHandle,
    hooks: Arc<AgentPromptHooks>,
    disposables: Arc<DisposableStore>,
    pub(super) shutdown: CancellationToken,
    pub(super) tasks: TaskTracker,
    controller: Mutex<Weak<SchedulerController>>,
}

impl SchedulerRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        context: AgentContextMemoryServiceHandle,
        reminders: AgentSystemReminderServiceHandle,
        instantiation: Arc<InstantiationService>,
        loop_service: AgentLoopServiceHandle,
        wire: WireServiceHandle,
        event_bus: EventBusHandle,
        hooks: Arc<AgentPromptHooks>,
        disposables: Arc<DisposableStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            context,
            reminders,
            instantiation,
            full_compaction: Mutex::new(None),
            loop_service,
            wire,
            event_bus,
            hooks,
            disposables,
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
            controller: Mutex::new(Weak::new()),
        })
    }
}

struct EnqueueAck {
    record: Arc<PromptRecord>,
    wait_for_launch: bool,
}

type SteerReply = oneshot::Sender<PromptServiceResult<Vec<PromptHandle>>>;
type ReservedSteer = (
    String,
    Vec<Arc<PromptRecord>>,
    ContextMessage,
    Vec<ContentPart>,
);

enum SchedulerCommand {
    Enqueue {
        input: Box<PromptInput>,
        reply: oneshot::Sender<PromptServiceResult<EnqueueAck>>,
    },
    Steer {
        prompt_ids: Vec<String>,
        reply: SteerReply,
    },
    Abort {
        prompt_id: String,
        reason: Option<Arc<dyn Error + Send + Sync>>,
        reply: oneshot::Sender<bool>,
    },
    Clear {
        reply: oneshot::Sender<PromptServiceResult<()>>,
    },
}

enum LaunchOutcome {
    Started(TurnHandle),
    Blocked {
        message: Box<ContextMessage>,
        captions: Vec<String>,
    },
    Failed,
    Cancelled,
}

enum SteerJobOutcome {
    Assigned(Option<TurnHandle>),
    Failed(Box<dyn Error + Send + Sync>),
    Cancelled,
}

enum SchedulerEvent {
    LaunchFinished {
        record: Arc<PromptRecord>,
        outcome: LaunchOutcome,
    },
    TurnSettled {
        record: Arc<PromptRecord>,
        result: LoopRunResult,
    },
    SteerFinished {
        operation_id: u64,
        outcome: SteerJobOutcome,
    },
    CompactionFinished,
}

#[derive(Clone)]
pub(super) struct SchedulerClient {
    commands: mpsc::Sender<SchedulerCommand>,
    snapshot: watch::Receiver<PromptQueueSnapshot>,
    runtime: Arc<SchedulerRuntime>,
    controller: Arc<SchedulerController>,
}

impl SchedulerClient {
    pub(super) async fn enqueue(&self, input: PromptInput) -> PromptServiceResult<PromptHandle> {
        ensure_running(&self.runtime)?;
        let (reply, response) = oneshot::channel();
        self.send(SchedulerCommand::Enqueue {
            input: Box::new(input),
            reply,
        })
        .await?;
        let ack = await_reply(&self.runtime, response).await??;
        let handle = prompt_handle(Arc::clone(&ack.record));

        // Historical behavior waits only when this enqueue itself opened the
        // launch slot.  A queued prompt (including one blocked by compaction)
        // must return its Pending handle immediately.
        if ack.wait_for_launch {
            tokio::select! {
                _ = ack.record.launched.future.clone() => {}
                _ = ack.record.completion.future.clone() => {}
                _ = self.runtime.shutdown.cancelled() => {}
            }
        }
        Ok(handle)
    }

    pub(super) fn list(&self) -> PromptQueueSnapshot {
        self.snapshot.borrow().clone()
    }

    pub(super) async fn steer(
        &self,
        prompt_ids: &[String],
    ) -> PromptServiceResult<Vec<PromptHandle>> {
        ensure_running(&self.runtime)?;
        let (reply, response) = oneshot::channel();
        self.send(SchedulerCommand::Steer {
            prompt_ids: prompt_ids.to_vec(),
            reply,
        })
        .await?;
        await_reply(&self.runtime, response).await?
    }

    pub(super) async fn abort(
        &self,
        prompt_id: &str,
        reason: Option<Arc<dyn Error + Send + Sync>>,
    ) -> bool {
        let (reply, response) = oneshot::channel();
        if self
            .send(SchedulerCommand::Abort {
                prompt_id: prompt_id.to_owned(),
                reason,
                reply,
            })
            .await
            .is_err()
        {
            return false;
        }
        await_reply(&self.runtime, response).await.unwrap_or(false)
    }

    pub(super) async fn clear(&self) -> PromptServiceResult<()> {
        let (reply, response) = oneshot::channel();
        self.send(SchedulerCommand::Clear { reply }).await?;
        await_reply(&self.runtime, response).await?
    }

    async fn send(&self, command: SchedulerCommand) -> PromptServiceResult<()> {
        // Start before sending so a dormant mailbox cannot fill under a burst.
        // Start again after sending to close the race where the runner entered
        // Parking between the first check and the channel send.
        self.controller.ensure_runner();
        let result = tokio::select! {
            biased;
            _ = self.runtime.shutdown.cancelled() => Err(shutdown_error()),
            result = self.commands.send(command) => result.map_err(|_| shutdown_error()),
        };
        if result.is_ok() {
            self.controller.ensure_runner();
        }
        result
    }
}

async fn await_reply<T>(
    runtime: &SchedulerRuntime,
    response: oneshot::Receiver<T>,
) -> PromptServiceResult<T> {
    tokio::select! {
        biased;
        result = response => result.map_err(|_| shutdown_error()),
        _ = runtime.shutdown.cancelled() => Err(shutdown_error()),
    }
}

pub(super) fn start_scheduler(runtime: Arc<SchedulerRuntime>) -> SchedulerClient {
    start_scheduler_with_idle_timeout(runtime, ACTOR_IDLE_TIMEOUT)
}

fn start_scheduler_with_idle_timeout(
    runtime: Arc<SchedulerRuntime>,
    idle_timeout: Duration,
) -> SchedulerClient {
    let (commands, command_rx) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_rx) = mpsc::unbounded_channel();
    let (snapshot_tx, snapshot) = watch::channel(PromptQueueSnapshot::default());
    let controller = Arc::new(SchedulerController {
        runtime: Arc::clone(&runtime),
        idle_timeout,
        state: Mutex::new(ControllerState {
            runner: RunnerState::Initializing,
            next_generation: 1,
        }),
    });
    *runtime.controller.lock().unwrap() = Arc::downgrade(&controller);
    let actor = SchedulerActor {
        runtime: Arc::clone(&runtime),
        state: SchedulerState::default(),
        command_rx,
        event_rx,
        events,
        snapshot_tx,
        next_steer_operation_id: 1,
        controller: Arc::downgrade(&controller),
    };
    controller.install(actor);
    SchedulerClient {
        commands,
        snapshot,
        runtime,
        controller,
    }
}

/// The controller owns a dormant Actor as a value and moves that value into a
/// Tokio task only while work exists.  Its mutex protects runner lifecycle, not
/// scheduler data: `SchedulerState` is never inspected or mutated through it.
struct SchedulerController {
    runtime: Arc<SchedulerRuntime>,
    idle_timeout: Duration,
    state: Mutex<ControllerState>,
}

struct ControllerState {
    runner: RunnerState,
    next_generation: u64,
}

enum RunnerState {
    Initializing,
    Dormant(Box<SchedulerActor>),
    Running {
        generation: u64,
    },
    Parking {
        generation: u64,
        restart_requested: bool,
    },
    Closed,
}

impl SchedulerController {
    fn install(&self, actor: SchedulerActor) {
        let mut state = self.state.lock().unwrap();
        debug_assert!(matches!(state.runner, RunnerState::Initializing));
        state.runner = RunnerState::Dormant(Box::new(actor));
    }

    fn ensure_runner(self: &Arc<Self>) {
        if self.runtime.shutdown.is_cancelled() {
            return;
        }
        let launch = {
            let mut state = self.state.lock().unwrap();
            match &mut state.runner {
                RunnerState::Dormant(_) => {
                    let generation = state.next_generation;
                    state.next_generation += 1;
                    let previous =
                        std::mem::replace(&mut state.runner, RunnerState::Running { generation });
                    let RunnerState::Dormant(actor) = previous else {
                        unreachable!("dormant runner must contain the parked actor")
                    };
                    Some((actor, generation))
                }
                RunnerState::Parking {
                    restart_requested, ..
                } => {
                    *restart_requested = true;
                    None
                }
                RunnerState::Initializing | RunnerState::Running { .. } | RunnerState::Closed => {
                    None
                }
            }
        };
        if let Some((actor, generation)) = launch {
            self.runtime.tasks.spawn(actor.run(generation));
        }
    }

    fn begin_parking(&self, generation: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if matches!(
            state.runner,
            RunnerState::Running {
                generation: current
            } if current == generation
        ) {
            state.runner = RunnerState::Parking {
                generation,
                restart_requested: false,
            };
            true
        } else {
            false
        }
    }

    fn cancel_parking(&self, generation: u64) {
        let mut state = self.state.lock().unwrap();
        if matches!(
            state.runner,
            RunnerState::Parking {
                generation: current,
                ..
            } if current == generation
        ) {
            state.runner = RunnerState::Running { generation };
        }
    }

    /// Atomically either parks the Actor or hands ownership back to the current
    /// task when a sender requested a restart during the parking handshake.
    fn finish_parking(
        &self,
        generation: u64,
        actor: SchedulerActor,
    ) -> Result<(), Box<SchedulerActor>> {
        let mut state = self.state.lock().unwrap();
        match &state.runner {
            RunnerState::Parking {
                generation: current,
                restart_requested,
            } if *current == generation && *restart_requested => {
                state.runner = RunnerState::Running { generation };
                Err(Box::new(actor))
            }
            RunnerState::Parking {
                generation: current,
                restart_requested: false,
            } if *current == generation => {
                state.runner = RunnerState::Dormant(Box::new(actor));
                Ok(())
            }
            RunnerState::Closed => Ok(()),
            _ => Err(Box::new(actor)),
        }
    }

    fn close_generation(&self, generation: u64) {
        let mut state = self.state.lock().unwrap();
        if matches!(
            state.runner,
            RunnerState::Running {
                generation: current
            } | RunnerState::Parking {
                generation: current,
                ..
            } if current == generation
        ) {
            state.runner = RunnerState::Closed;
        }
    }

    fn close(&self) {
        // A dormant Actor is quiescent by construction, so dropping it here is
        // sufficient.  A running Actor still owns its value and observes the
        // shutdown token before settling all live records.
        self.state.lock().unwrap().runner = RunnerState::Closed;
    }

    #[cfg(test)]
    fn is_dormant(&self) -> bool {
        matches!(self.state.lock().unwrap().runner, RunnerState::Dormant(_))
    }
}

#[derive(Default)]
struct SchedulerState {
    active: Option<(Arc<PromptRecord>, TurnHandle)>,
    pending: VecDeque<Arc<PromptRecord>>,
    steered: HashMap<String, Vec<Arc<PromptRecord>>>,
    launching: Option<Arc<PromptRecord>>,
    steer_in_flight: Option<SteerOperation>,
    pending_steers: VecDeque<PendingSteer>,
}

impl SchedulerState {
    fn is_quiescent(&self) -> bool {
        self.active.is_none()
            && self.pending.is_empty()
            && self.steered.is_empty()
            && self.launching.is_none()
            && self.steer_in_flight.is_none()
            && self.pending_steers.is_empty()
    }
}

struct PendingSteer {
    prompt_ids: Vec<String>,
    reply: SteerReply,
}

struct SteerOperation {
    id: u64,
    active_id: String,
    selected: Vec<Arc<PromptRecord>>,
    cancellation: CancellationToken,
    reply: SteerReply,
}

#[derive(Clone)]
struct SteerMaterializedEvent {
    active_prompt_id: String,
    prompt_ids: Vec<String>,
    content: Vec<ContentPart>,
    user_messages: Vec<LiveUserMessage>,
}

struct SchedulerActor {
    runtime: Arc<SchedulerRuntime>,
    // This field is deliberately not behind Arc/Mutex: it is moved between the
    // runner task and the dormant controller slot, and mutated only in `run`.
    state: SchedulerState,
    command_rx: mpsc::Receiver<SchedulerCommand>,
    event_rx: mpsc::UnboundedReceiver<SchedulerEvent>,
    events: mpsc::UnboundedSender<SchedulerEvent>,
    snapshot_tx: watch::Sender<PromptQueueSnapshot>,
    next_steer_operation_id: u64,
    controller: Weak<SchedulerController>,
}

impl SchedulerActor {
    async fn run(mut self, generation: u64) {
        loop {
            // Internal events cannot be backpressured by a full public command
            // queue.  `biased` also prevents completion/shutdown starvation.
            // The unbounded side is safe here because at most one launch, one
            // active monitor and one serial steer operation can produce events.
            tokio::select! {
                biased;
                _ = self.runtime.shutdown.cancelled() => {
                    self.shutdown_all();
                    self.publish_snapshot();
                    if let Some(controller) = self.controller.upgrade() {
                        controller.close_generation(generation);
                    }
                    return;
                }
                Some(event) = self.event_rx.recv() => self.handle_event(event),
                Some(command) = self.command_rx.recv() => self.handle_command(command),
                _ = tokio::time::sleep(
                    self.controller
                        .upgrade()
                        .map_or(ACTOR_IDLE_TIMEOUT, |controller| controller.idle_timeout)
                ), if self.state.is_quiescent() => {
                    let Some(controller) = self.controller.upgrade() else {
                        self.shutdown_all();
                        self.publish_snapshot();
                        return;
                    };
                    if !controller.begin_parking(generation) {
                        if self.runtime.shutdown.is_cancelled() {
                            self.shutdown_all();
                            self.publish_snapshot();
                            controller.close_generation(generation);
                            return;
                        }
                        continue;
                    }

                    // Once Parking is visible, every sender requests a restart.
                    // Drain work queued just before that transition; work queued
                    // after it is covered by `restart_requested`.
                    if let Ok(event) = self.event_rx.try_recv() {
                        controller.cancel_parking(generation);
                        self.handle_event(event);
                        continue;
                    }
                    if let Ok(command) = self.command_rx.try_recv() {
                        controller.cancel_parking(generation);
                        self.handle_command(command);
                        continue;
                    }

                    match controller.finish_parking(generation, self) {
                        Ok(()) => return,
                        Err(actor) => self = *actor,
                    }
                }
                else => {
                    self.shutdown_all();
                    self.publish_snapshot();
                    if let Some(controller) = self.controller.upgrade() {
                        controller.close_generation(generation);
                    }
                    return;
                }
            }
        }
    }

    fn handle_command(&mut self, command: SchedulerCommand) {
        match command {
            SchedulerCommand::Enqueue { input, reply } => {
                let result = self.make_record(*input).map(|record| {
                    self.runtime.event_bus.publish_typed(PromptSubmittedEvent {
                        user_message: record.user_message.clone(),
                        status: PromptSubmittedStatus::Queued,
                    });
                    self.state.pending.push_back(Arc::clone(&record));
                    self.drive();
                    let wait_for_launch = self
                        .state
                        .launching
                        .as_ref()
                        .is_some_and(|launching| Arc::ptr_eq(launching, &record));
                    EnqueueAck {
                        record,
                        wait_for_launch,
                    }
                });
                self.publish_snapshot();
                let _ = reply.send(result);
            }
            SchedulerCommand::Steer { prompt_ids, reply } => {
                self.state
                    .pending_steers
                    .push_back(PendingSteer { prompt_ids, reply });
                self.drive_steer();
                self.drive();
                self.publish_snapshot();
            }
            SchedulerCommand::Abort {
                prompt_id,
                reason,
                reply,
            } => {
                let accepted = self.abort_prompt(&prompt_id, reason);
                self.drive();
                self.publish_snapshot();
                let _ = reply.send(accepted);
            }
            SchedulerCommand::Clear { reply } => {
                self.clear_all();
                let result = self
                    .runtime
                    .context
                    .clear()
                    .map(|_| ())
                    .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>);
                self.publish_snapshot();
                let _ = reply.send(result);
            }
        }
    }

    fn handle_event(&mut self, event: SchedulerEvent) {
        match event {
            SchedulerEvent::LaunchFinished { record, outcome } => {
                self.finish_launch(record, outcome)
            }
            SchedulerEvent::TurnSettled { record, result } => self.finish_turn(record, result),
            SchedulerEvent::SteerFinished {
                operation_id,
                outcome,
            } => self.finish_steer(operation_id, outcome),
            SchedulerEvent::CompactionFinished => {}
        }
        self.drive_steer();
        self.drive();
        self.publish_snapshot();
    }

    fn make_record(&self, input: PromptInput) -> PromptServiceResult<Arc<PromptRecord>> {
        let id = input
            .id
            .or_else(|| input.message.id.clone())
            .unwrap_or_else(new_message_id);
        // RPC clients may supply an id to correlate their local queue row with
        // lifecycle events.  Reject a live collision instead of letting two
        // records respond to the same abort/steer command.
        if self.contains_prompt_id(&id) {
            return Err(coded(
                REQUEST_INVALID,
                format!("prompt id '{id}' is already in use"),
            ));
        }
        let mut message = input.message;
        message.id = Some(id.clone());
        let created_at = now();
        let user_message = LiveUserMessage {
            prompt_id: id.clone(),
            user_message_id: id.clone(),
            created_at: created_at.clone(),
            content: to_protocol_message_content(&message)?,
            origin: message.origin.clone().unwrap_or(PromptOrigin::User),
        };
        Ok(Arc::new(PromptRecord {
            snapshot: Mutex::new(PromptSnapshot {
                id: id.clone(),
                user_message_id: id,
                created_at,
                state: PromptState::Pending,
                message,
            }),
            user_message,
            launched: Deferred::new(),
            completion: Deferred::new(),
            cancellation: self.runtime.shutdown.child_token(),
        }))
    }

    fn contains_prompt_id(&self, prompt_id: &str) -> bool {
        self.state
            .active
            .as_ref()
            .is_some_and(|(record, _)| record.snapshot().id == prompt_id)
            || self
                .state
                .launching
                .as_ref()
                .is_some_and(|record| record.snapshot().id == prompt_id)
            || self
                .state
                .pending
                .iter()
                .any(|record| record.snapshot().id == prompt_id)
            || self
                .state
                .steered
                .values()
                .flatten()
                .any(|record| record.snapshot().id == prompt_id)
            || self
                .state
                .steer_in_flight
                .as_ref()
                .is_some_and(|operation| {
                    operation
                        .selected
                        .iter()
                        .any(|record| record.snapshot().id == prompt_id)
                })
    }

    fn drive(&mut self) {
        if self.state.active.is_some() || self.state.launching.is_some() {
            return;
        }
        if full_compaction(&self.runtime, &self.events, &self.controller)
            .is_some_and(|service| service.compacting().is_some())
            && self.runtime.loop_service.status().state != AgentLoopState::Running
        {
            return;
        }
        let Some(record) = self.state.pending.pop_front() else {
            return;
        };
        self.state.launching = Some(Arc::clone(&record));
        let runtime = Arc::clone(&self.runtime);
        let events = self.events.clone();
        let controller = self.controller.clone();
        self.runtime.tasks.spawn(async move {
            let outcome = launch_job(Arc::clone(&runtime), Arc::clone(&record)).await;
            send_scheduler_event(
                &events,
                &controller,
                SchedulerEvent::LaunchFinished { record, outcome },
            );
        });
    }

    fn finish_launch(&mut self, record: Arc<PromptRecord>, outcome: LaunchOutcome) {
        // Pointer identity, rather than a reusable prompt id, proves the event
        // belongs to the exact launch slot.  Late results after clear/abort are
        // therefore harmless and cannot overwrite a newer active prompt.
        if self
            .state
            .launching
            .as_ref()
            .is_none_or(|launching| !Arc::ptr_eq(launching, &record))
        {
            if let LaunchOutcome::Started(turn) = &outcome {
                // Identity rejection protects Actor state, while cancelling the
                // returned turn also closes the assignment-after-clear window.
                let _ = self.runtime.loop_service.cancel(
                    Some(turn.0.id()),
                    Some(LoopValue::Error(Arc::new(user_cancellation_reason()))),
                );
            }
            return;
        }
        self.state.launching = None;
        if record.cancellation.is_cancelled() {
            if let LaunchOutcome::Started(turn) = &outcome {
                let _ = self.runtime.loop_service.cancel(
                    Some(turn.0.id()),
                    Some(LoopValue::Error(Arc::new(user_cancellation_reason()))),
                );
            }
            cancel_record(&self.runtime, &record);
            return;
        }
        match outcome {
            LaunchOutcome::Started(turn) => {
                record.set_state(PromptState::Running);
                record.launched.resolve(Some(turn.clone()));
                self.state.active = Some((Arc::clone(&record), turn.clone()));
                let events = self.events.clone();
                let controller = self.controller.clone();
                self.runtime.tasks.spawn(async move {
                    let result = turn.0.result().await;
                    send_scheduler_event(
                        &events,
                        &controller,
                        SchedulerEvent::TurnSettled { record, result },
                    );
                });
            }
            LaunchOutcome::Blocked { message, captions } => {
                append_prompt(&self.runtime, &message, &captions);
                settle_record(
                    &self.runtime,
                    &record,
                    PromptState::Blocked,
                    PromptCompletionState::Blocked,
                    None,
                );
            }
            LaunchOutcome::Failed => fail_record(&self.runtime, &record),
            LaunchOutcome::Cancelled => cancel_record(&self.runtime, &record),
        }
    }

    fn finish_turn(&mut self, record: Arc<PromptRecord>, result: LoopRunResult) {
        if self
            .state
            .active
            .as_ref()
            .is_none_or(|(active, _)| !Arc::ptr_eq(active, &record))
        {
            return;
        }
        self.state.active = None;
        let id = record.snapshot().id;
        let children = self.state.steered.remove(&id).unwrap_or_default();
        let (prompt_state, completion_state, reason) = match &result {
            LoopRunResult::Completed { .. } => (
                PromptState::Completed,
                PromptCompletionState::Completed,
                "completed",
            ),
            LoopRunResult::Failed { .. } => {
                (PromptState::Failed, PromptCompletionState::Failed, "failed")
            }
            LoopRunResult::Cancelled { .. } => (
                PromptState::Cancelled,
                PromptCompletionState::Cancelled,
                "cancelled",
            ),
        };
        if record.set_terminal_state(prompt_state) {
            record.completion.resolve(PromptCompletion {
                prompt_id: id.clone(),
                result: Some(result.clone()),
                state: completion_state,
            });
        }
        for child in children {
            let child_id = child.snapshot().id;
            if child.set_terminal_state(prompt_state) {
                child.completion.resolve(PromptCompletion {
                    prompt_id: child_id.clone(),
                    result: Some(result.clone()),
                    state: completion_state,
                });
                // Steered prompts have their own public lifecycle.  Publishing
                // their terminal event prevents clients that missed the
                // materialization event from retaining a phantom queue item.
                if completion_state == PromptCompletionState::Cancelled {
                    publish_aborted(&self.runtime, &child_id);
                } else {
                    publish_completed(&self.runtime, &child_id, reason);
                }
            }
        }
        if completion_state == PromptCompletionState::Cancelled {
            publish_aborted(&self.runtime, &id);
        } else {
            publish_completed(&self.runtime, &id, reason);
        }
    }

    fn drive_steer(&mut self) {
        if self.state.steer_in_flight.is_some() {
            return;
        }
        while let Some(request) = self.state.pending_steers.pop_front() {
            match self.reserve_steer(request.prompt_ids) {
                Ok((active_id, selected, message, content)) => {
                    let id = self.next_steer_operation_id;
                    self.next_steer_operation_id += 1;
                    let cancellation = self.runtime.shutdown.child_token();
                    let worker_cancellation = cancellation.clone();
                    let runtime = Arc::clone(&self.runtime);
                    let events = self.events.clone();
                    let controller = self.controller.clone();
                    let materialized_event = SteerMaterializedEvent {
                        active_prompt_id: active_id.clone(),
                        prompt_ids: selected.iter().map(|record| record.snapshot().id).collect(),
                        content: content.clone(),
                        user_messages: selected
                            .iter()
                            .map(|record| record.user_message.clone())
                            .collect(),
                    };
                    self.state.steer_in_flight = Some(SteerOperation {
                        id,
                        active_id,
                        selected,
                        cancellation,
                        reply: request.reply,
                    });
                    self.runtime.tasks.spawn(async move {
                        let outcome =
                            steer_job(runtime, message, materialized_event, worker_cancellation)
                                .await;
                        send_scheduler_event(
                            &events,
                            &controller,
                            SchedulerEvent::SteerFinished {
                                operation_id: id,
                                outcome,
                            },
                        );
                    });
                    return;
                }
                Err(error) => {
                    let _ = request.reply.send(Err(error));
                }
            }
        }
    }

    fn reserve_steer(&mut self, prompt_ids: Vec<String>) -> PromptServiceResult<ReservedSteer> {
        if prompt_ids.is_empty() {
            return Err(coded(REQUEST_INVALID, "prompt_ids must not be empty"));
        }
        let active_id = self
            .state
            .active
            .as_ref()
            .ok_or_else(|| coded(PROMPT_NOT_FOUND, "no active prompt to steer into"))?
            .0
            .snapshot()
            .id;
        let wanted = prompt_ids.iter().collect::<HashSet<_>>();
        if wanted.len() != prompt_ids.len()
            || self
                .state
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
        self.state.pending.retain(|record| {
            if wanted.contains(&record.snapshot().id) {
                selected.push(Arc::clone(record));
                false
            } else {
                true
            }
        });
        let selected_messages = selected
            .iter()
            .map(|record| record.snapshot().message)
            .collect::<Vec<_>>();
        let content = selected_messages
            .iter()
            .flat_map(|message| message.message.content.clone())
            .collect::<Vec<_>>();
        let attachments = selected_messages
            .into_iter()
            .flat_map(|message| message.attachments)
            .collect();
        let message = ContextMessage {
            message: Message::new(Role::User, content.clone(), Vec::new()),
            id: None,
            provider_message_id: None,
            origin: Some(PromptOrigin::User),
            is_error: None,
            note: None,
            attachments,
        };
        Ok((active_id, selected, message, content))
    }

    fn finish_steer(&mut self, operation_id: u64, outcome: SteerJobOutcome) {
        let Some(operation) = self.state.steer_in_flight.take() else {
            return;
        };
        if operation.id != operation_id {
            self.state.steer_in_flight = Some(operation);
            return;
        }
        if operation.cancellation.is_cancelled() {
            if let SteerJobOutcome::Assigned(Some(turn)) = &outcome {
                let _ = self.runtime.loop_service.cancel(
                    Some(turn.0.id()),
                    Some(LoopValue::Error(Arc::new(user_cancellation_reason()))),
                );
            }
            for record in &operation.selected {
                cancel_record(&self.runtime, record);
            }
            let _ = operation.reply.send(Err(shutdown_error()));
            return;
        }
        match outcome {
            SteerJobOutcome::Assigned(Some(turn))
                if self
                    .state
                    .active
                    .as_ref()
                    .is_some_and(|(record, active_turn)| {
                        record.snapshot().id == operation.active_id
                            && active_turn.0.id() == turn.0.id()
                    }) =>
            {
                for record in &operation.selected {
                    record.set_state(PromptState::Steered);
                    record.launched.resolve(Some(turn.clone()));
                }
                self.state
                    .steered
                    .entry(operation.active_id.clone())
                    .or_default()
                    .extend(operation.selected.iter().cloned());
                let handles = operation.selected.into_iter().map(prompt_handle).collect();
                let _ = operation.reply.send(Ok(handles));
            }
            SteerJobOutcome::Cancelled => {
                for record in &operation.selected {
                    cancel_record(&self.runtime, record);
                }
                let _ = operation.reply.send(Err(shutdown_error()));
            }
            SteerJobOutcome::Failed(error) => {
                for record in &operation.selected {
                    fail_record(&self.runtime, record);
                }
                let _ = operation.reply.send(Err(error));
            }
            SteerJobOutcome::Assigned(_) => {
                for record in &operation.selected {
                    fail_record(&self.runtime, record);
                }
                let _ = operation
                    .reply
                    .send(Err(coded(PROMPT_NOT_FOUND, "no active turn to steer into")));
            }
        }
    }

    fn abort_prompt(
        &mut self,
        prompt_id: &str,
        reason: Option<Arc<dyn Error + Send + Sync>>,
    ) -> bool {
        if let Some((record, turn)) = self.state.active.as_ref()
            && record.snapshot().id == prompt_id
        {
            record.cancellation.cancel();
            let cancellation = reason.unwrap_or_else(|| Arc::new(user_cancellation_reason()));
            let _ = self
                .runtime
                .loop_service
                .cancel(Some(turn.0.id()), Some(LoopValue::Error(cancellation)));
            return true;
        }
        if let Some(record) = self.state.launching.as_ref()
            && record.snapshot().id == prompt_id
        {
            // Keep the launch slot reserved until the worker acknowledges
            // cancellation.  Otherwise a new launch could overlap the receipt
            // abort window and leave an orphan turn in the loop queue.
            record.cancellation.cancel();
            return true;
        }
        if let Some(index) = self
            .state
            .pending
            .iter()
            .position(|record| record.snapshot().id == prompt_id)
        {
            let record = self
                .state
                .pending
                .remove(index)
                .expect("pending prompt index must remain valid");
            record.cancellation.cancel();
            cancel_record(&self.runtime, &record);
            return true;
        }
        if let Some(operation) = self.state.steer_in_flight.as_ref()
            && operation
                .selected
                .iter()
                .any(|record| record.snapshot().id == prompt_id)
        {
            // A steer request materializes one combined step, so cancelling one
            // reserved child must cancel the whole in-flight operation.
            operation.cancellation.cancel();
            return true;
        }
        false
    }

    fn clear_all(&mut self) {
        let error = || coded(PROMPT_NOT_FOUND, "prompt queue was cleared");
        for pending in self.state.pending_steers.drain(..) {
            let _ = pending.reply.send(Err(error()));
        }
        if let Some(operation) = self.state.steer_in_flight.take() {
            operation.cancellation.cancel();
            for record in operation.selected {
                record.cancellation.cancel();
                cancel_record(&self.runtime, &record);
            }
            let _ = operation.reply.send(Err(error()));
        }
        let pending = self.state.pending.drain(..).collect::<Vec<_>>();
        for record in pending {
            record.cancellation.cancel();
            cancel_record(&self.runtime, &record);
        }
        if let Some(record) = self.state.launching.take() {
            record.cancellation.cancel();
            cancel_record(&self.runtime, &record);
        }
        if let Some((record, turn)) = self.state.active.take() {
            record.cancellation.cancel();
            let _ = self.runtime.loop_service.cancel(
                Some(turn.0.id()),
                Some(LoopValue::Error(Arc::new(user_cancellation_reason()))),
            );
            cancel_record(&self.runtime, &record);
        }
        for records in self.state.steered.drain().map(|(_, records)| records) {
            for record in records {
                record.cancellation.cancel();
                cancel_record(&self.runtime, &record);
            }
        }
    }

    fn shutdown_all(&mut self) {
        // Closing is atomic from the scheduler's point of view: every owned
        // record is settled before the actor exits.  Worker events that arrive
        // later have no receiver and cannot mutate state.
        self.clear_all();
    }

    fn publish_snapshot(&self) {
        let snapshot = PromptQueueSnapshot {
            active: self
                .state
                .active
                .as_ref()
                .map(|(record, _)| record.snapshot()),
            pending: self
                .state
                .pending
                .iter()
                .map(|record| record.snapshot())
                .collect(),
        };
        self.snapshot_tx.send_replace(snapshot);
    }
}

async fn launch_job(runtime: Arc<SchedulerRuntime>, record: Arc<PromptRecord>) -> LaunchOutcome {
    let snapshot = record.snapshot();
    let (message, captions) = extract_message(&snapshot.message);
    let mut hook = PromptSubmitContext {
        prompt_message: message,
        is_steer: false,
        block: false,
    };
    let hook_result = tokio::select! {
        biased;
        _ = record.cancellation.cancelled() => return LaunchOutcome::Cancelled,
        result = runtime.hooks.on_before_submit_prompt.run(&mut hook, None) => result,
    };
    if hook_result.is_err() {
        return LaunchOutcome::Failed;
    }
    if hook.block {
        return LaunchOutcome::Blocked {
            message: Box::new(hook.prompt_message),
            captions,
        };
    }
    let receipt = match runtime.loop_service.enqueue(
        Arc::new(PromptStepRequest::new(
            hook.prompt_message,
            captions,
            runtime.reminders.0.clone(),
            record.user_message.clone(),
        )),
        None,
    ) {
        Ok(receipt) => receipt,
        Err(_) => return LaunchOutcome::Failed,
    };
    tokio::select! {
        biased;
        _ = record.cancellation.cancelled() => {
            // Cancellation between enqueue and assignment must retract the
            // receipt; cancelling only the future could leave an orphan turn.
            receipt.abort(Some(LoopValue::Error(Arc::new(user_cancellation_reason()))));
            LaunchOutcome::Cancelled
        }
        assignment = receipt.assigned.clone() => match assignment {
            Ok(_assignment) if record.cancellation.is_cancelled() => {
                receipt.abort(Some(LoopValue::Error(Arc::new(user_cancellation_reason()))));
                LaunchOutcome::Cancelled
            }
            Ok(assignment) => LaunchOutcome::Started(assignment.turn),
            Err(_) if record.cancellation.is_cancelled() => LaunchOutcome::Cancelled,
            Err(_) => LaunchOutcome::Failed,
        }
    }
}

async fn steer_job(
    runtime: Arc<SchedulerRuntime>,
    message: ContextMessage,
    materialized_event: SteerMaterializedEvent,
    cancellation: CancellationToken,
) -> SteerJobOutcome {
    let (message, captions) = extract_message(&message);
    let wire = runtime.wire.clone();
    let event_bus = runtime.event_bus.clone();
    let request = SteerStepRequest::new(
        message,
        captions,
        runtime.reminders.0.clone(),
        Arc::new(move |materialized| {
            let seed = TurnSeed {
                input: materialized.message.content.clone(),
                origin: materialized.origin.clone().unwrap_or(PromptOrigin::User),
                user_message: None,
            };
            if let Ok(operation) = crate::agent::loop_::steer_turn(seed) {
                let _ = wire.dispatch([operation]);
            }
            // Assignment only means that the active turn accepted this step.
            // The queue-to-conversation transition belongs here: immediately
            // before the request is appended to context and can affect the
            // next model invocation.  Publishing earlier races with the
            // assistant output that is still being streamed.
            event_bus.publish(DomainEvent::new(
                "prompt.steered",
                Map::from_iter([
                    (
                        "activePromptId".into(),
                        Value::String(materialized_event.active_prompt_id.clone()),
                    ),
                    (
                        "promptIds".into(),
                        serde_json::to_value(&materialized_event.prompt_ids).unwrap_or(Value::Null),
                    ),
                    (
                        "content".into(),
                        serde_json::to_value(&materialized_event.content).unwrap_or(Value::Null),
                    ),
                    (
                        "userMessages".into(),
                        serde_json::to_value(&materialized_event.user_messages)
                            .unwrap_or(Value::Null),
                    ),
                    ("steeredAt".into(), Value::String(now())),
                ]),
            ));
        }),
        Arc::new(|| {}),
        Some(StepRequestAdmission::ActiveTurnOnly),
    );
    let receipt = match runtime.loop_service.enqueue(Arc::new(request), None) {
        Ok(receipt) => receipt,
        Err(error) => return SteerJobOutcome::Failed(Box::new(error)),
    };
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            receipt.abort(Some(LoopValue::Error(Arc::new(user_cancellation_reason()))));
            SteerJobOutcome::Cancelled
        }
        assignment = receipt.assigned.clone() => match assignment {
            Ok(_assignment) if cancellation.is_cancelled() => {
                receipt.abort(Some(LoopValue::Error(Arc::new(user_cancellation_reason()))));
                SteerJobOutcome::Cancelled
            }
            Ok(assignment) => SteerJobOutcome::Assigned(Some(assignment.turn)),
            Err(_error) if cancellation.is_cancelled() => SteerJobOutcome::Cancelled,
            Err(error) => SteerJobOutcome::Failed(Box::new(error)),
        }
    }
}

pub(super) async fn inject_runtime(
    runtime: Arc<SchedulerRuntime>,
    message: ContextMessage,
) -> PromptServiceResult<Option<TurnHandle>> {
    ensure_running(&runtime)?;
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
                user_message: None,
            };
            if let Ok(operation) = crate::agent::loop_::steer_turn(seed) {
                let _ = wire.dispatch([operation]);
            }
        }),
        Arc::new(|| {}),
        Some(StepRequestAdmission::ActiveOrNewTurn),
    );
    let receipt = runtime.loop_service.enqueue(Arc::new(request), None)?;
    let assignment = tokio::select! {
        biased;
        _ = runtime.shutdown.cancelled() => {
            receipt.abort(Some(LoopValue::Error(Arc::new(user_cancellation_reason()))));
            return Err(shutdown_error());
        }
        assignment = receipt.assigned.clone() => assignment?,
    };
    Ok(Some(assignment.turn))
}

pub(super) fn undo(runtime: &SchedulerRuntime, count: u32) -> PromptServiceResult<usize> {
    if count == 0 {
        return Ok(0);
    }
    let check = precheck_undo(&runtime.context.get(), count);
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
    Ok(runtime.context.undo(count)?.removed_count)
}

pub(super) fn begin_shutdown(runtime: &Arc<SchedulerRuntime>) {
    runtime.shutdown.cancel();
    if let Some(controller) = runtime.controller.lock().unwrap().upgrade() {
        controller.close();
    }
    runtime.tasks.close();
}

fn full_compaction(
    runtime: &Arc<SchedulerRuntime>,
    events: &mpsc::UnboundedSender<SchedulerEvent>,
    controller: &Weak<SchedulerController>,
) -> Option<AgentFullCompactionServiceHandle> {
    let events = events.clone();
    let controller = controller.clone();
    resolve_full_compaction(
        &runtime.instantiation,
        &runtime.full_compaction,
        &runtime.disposables,
        move |_| {
            // Compaction callbacks are synchronous, so they cannot await a
            // bounded sender.  The event only wakes `drive`; it carries no data.
            send_scheduler_event(&events, &controller, SchedulerEvent::CompactionFinished);
        },
    )
}

fn send_scheduler_event(
    events: &mpsc::UnboundedSender<SchedulerEvent>,
    controller: &Weak<SchedulerController>,
    event: SchedulerEvent,
) {
    if events.send(event).is_ok()
        && let Some(controller) = controller.upgrade()
    {
        controller.ensure_runner();
    }
}

fn resolve_full_compaction(
    instantiation: &InstantiationService,
    cache: &Mutex<Option<AgentFullCompactionServiceHandle>>,
    disposables: &DisposableStore,
    on_did_finish: impl Fn(&crate::agent::full_compaction::FullCompactionTask) + Send + Sync + 'static,
) -> Option<AgentFullCompactionServiceHandle> {
    let mut cached = cache.lock().unwrap();
    if let Some(service) = cached.as_ref() {
        return Some(service.clone());
    }
    let service = (*instantiation.get(AGENT_FULL_COMPACTION_SERVICE_ID).ok()?).clone();
    disposables.add(service.on_did_finish_compaction().subscribe(on_did_finish));
    *cached = Some(service.clone());
    Some(service)
}

fn settle_record(
    runtime: &SchedulerRuntime,
    record: &Arc<PromptRecord>,
    prompt_state: PromptState,
    completion_state: PromptCompletionState,
    result: Option<LoopRunResult>,
) {
    if !record.set_terminal_state(prompt_state) {
        return;
    }
    let id = record.snapshot().id;
    record.launched.resolve(None);
    record.completion.resolve(PromptCompletion {
        prompt_id: id.clone(),
        result,
        state: completion_state,
    });
    if completion_state == PromptCompletionState::Cancelled {
        publish_aborted(runtime, &id);
    } else {
        publish_completed(
            runtime,
            &id,
            match completion_state {
                PromptCompletionState::Completed => "completed",
                PromptCompletionState::Failed => "failed",
                PromptCompletionState::Cancelled => "cancelled",
                PromptCompletionState::Blocked => "blocked",
            },
        );
    }
}

fn cancel_record(runtime: &SchedulerRuntime, record: &Arc<PromptRecord>) {
    settle_record(
        runtime,
        record,
        PromptState::Cancelled,
        PromptCompletionState::Cancelled,
        None,
    );
}

fn fail_record(runtime: &SchedulerRuntime, record: &Arc<PromptRecord>) {
    settle_record(
        runtime,
        record,
        PromptState::Failed,
        PromptCompletionState::Failed,
        None,
    );
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

fn append_prompt(runtime: &SchedulerRuntime, message: &ContextMessage, captions: &[String]) {
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

fn publish_completed(runtime: &SchedulerRuntime, prompt_id: &str, reason: &str) {
    runtime.event_bus.publish(DomainEvent::new(
        "prompt.completed",
        Map::from_iter([
            ("promptId".into(), Value::String(prompt_id.into())),
            ("finishedAt".into(), Value::String(now())),
            ("reason".into(), Value::String(reason.into())),
        ]),
    ));
}

fn publish_aborted(runtime: &SchedulerRuntime, prompt_id: &str) {
    runtime.event_bus.publish(DomainEvent::new(
        "prompt.aborted",
        Map::from_iter([
            ("promptId".into(), Value::String(prompt_id.into())),
            ("abortedAt".into(), Value::String(now())),
        ]),
    ));
}

fn ensure_running(runtime: &SchedulerRuntime) -> PromptServiceResult<()> {
    if runtime.shutdown.is_cancelled() {
        Err(shutdown_error())
    } else {
        Ok(())
    }
}

fn shutdown_error() -> Box<dyn Error + Send + Sync> {
    Box::new(abort_error(Some("Agent prompt service shut down")))
}

fn coded(code: &str, message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(Error2::new(code, message))
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use futures_util::{FutureExt, future, stream};

    use super::*;
    use crate::{
        _base::{
            di::{
                lifecycle::{Disposable, DisposableHandle, disposable_none},
                service_collection::ServiceCollection,
            },
            event::{Emitter, Event},
            utils::abort::AbortController,
        },
        agent::{
            context_memory::{
                AgentContextMemoryService, AgentContextMemoryServiceContract,
                AgentContextMemoryServiceHandle,
            },
            full_compaction::{
                AgentFullCompactionHooks, AgentFullCompactionServiceContract, CompactionSource,
                FullCompactionError, FullCompactionInput, FullCompactionTask,
            },
            loop_::{
                AgentLoopHooks, AgentLoopServiceContract, AgentLoopStatus, EnqueueReceipt,
                LoopErrorHandler, LoopErrorHandlerRegistrationOptions, LoopRunOptions,
                StepAssignment, StepEnqueueOptions, StepHandle, StepHandleContract, StepRequest,
                StepResult, StepResultFuture, StepState, TurnHandleContract, TurnReadyFuture,
                TurnResultFuture, TurnState,
            },
            system_reminder::{AgentSystemReminderService, AgentSystemReminderServiceHandle},
        },
        app::event::{
            event_bus::{EventBusContract, EventBusHandle},
            event_bus_service::EventBusService,
        },
        hooks::HookRegisterOptions,
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        wire::wire_service::{DomainEventPublisher, WireBlobService, WireService},
    };

    #[derive(Default)]
    struct MemoryLog;

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
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

    struct FakeStep {
        id: String,
        turn_id: crate::agent::TurnId,
        controller: AbortController,
    }

    impl StepHandleContract for FakeStep {
        fn id(&self) -> &str {
            &self.id
        }

        fn turn_id(&self) -> crate::agent::TurnId {
            self.turn_id
        }

        fn state(&self) -> StepState {
            StepState::Completed
        }

        fn signal(&self) -> crate::_base::utils::abort::AbortSignal {
            self.controller.signal()
        }

        fn result(&self) -> StepResultFuture {
            future::ready(StepResult::Completed).boxed().shared()
        }

        fn cancel(&self, _: Option<LoopValue>) -> bool {
            false
        }
    }

    struct FakeTurn {
        id: crate::agent::TurnId,
        state: Mutex<TurnState>,
        controller: AbortController,
        result: Deferred<LoopRunResult>,
    }

    impl FakeTurn {
        fn new(id: crate::agent::TurnId) -> Self {
            Self {
                id,
                state: Mutex::new(TurnState::Running),
                controller: AbortController::new(),
                result: Deferred::new(),
            }
        }

        fn settle(&self, result: LoopRunResult) {
            *self.state.lock().unwrap() = match result {
                LoopRunResult::Completed { .. } => TurnState::Completed,
                LoopRunResult::Failed { .. } => TurnState::Failed,
                LoopRunResult::Cancelled { .. } => TurnState::Cancelled,
            };
            self.result.resolve(result);
        }
    }

    impl TurnHandleContract for FakeTurn {
        fn id(&self) -> crate::agent::TurnId {
            self.id
        }

        fn state(&self) -> Option<TurnState> {
            Some(*self.state.lock().unwrap())
        }

        fn signal(&self) -> crate::_base::utils::abort::AbortSignal {
            self.controller.signal()
        }

        fn ready(&self) -> TurnReadyFuture {
            future::ready(Ok(())).boxed().shared()
        }

        fn result(&self) -> TurnResultFuture {
            self.result.future.clone()
        }

        fn cancel(&self, reason: Option<LoopValue>) -> bool {
            if matches!(
                *self.state.lock().unwrap(),
                TurnState::Completed | TurnState::Failed | TurnState::Cancelled
            ) {
                return false;
            }
            self.settle(LoopRunResult::Cancelled {
                steps: 0,
                reason: reason
                    .unwrap_or_else(|| LoopValue::Error(Arc::new(user_cancellation_reason()))),
            });
            true
        }
    }

    struct FakeLoop {
        hooks: AgentLoopHooks,
        next_id: AtomicU64,
        active: Mutex<Option<Arc<FakeTurn>>>,
        requests: Mutex<Vec<Arc<dyn StepRequest>>>,
    }

    impl FakeLoop {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                hooks: AgentLoopHooks::default(),
                next_id: AtomicU64::new(1),
                active: Mutex::new(None),
                requests: Mutex::new(Vec::new()),
            })
        }

        fn complete(&self, turn_id: crate::agent::TurnId) {
            let turn = self.active.lock().unwrap().clone().unwrap();
            assert_eq!(turn.id, turn_id);
            turn.settle(LoopRunResult::Completed {
                steps: 1,
                truncated: false,
            });
        }

        fn materialize_last_request(&self) {
            self.requests
                .lock()
                .unwrap()
                .last()
                .expect("a queued request should exist")
                .on_will_materialize();
        }
    }

    #[async_trait]
    impl AgentLoopServiceContract for FakeLoop {
        fn enqueue(
            &self,
            request: Arc<dyn StepRequest>,
            _: Option<StepEnqueueOptions>,
        ) -> Result<EnqueueReceipt, LoopValue> {
            let turn = if request.admission() == StepRequestAdmission::ActiveTurnOnly {
                self.active
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("steer tests require an active turn")
            } else {
                let turn = Arc::new(FakeTurn::new(crate::agent::TurnId::new(
                    self.next_id.fetch_add(1, Ordering::SeqCst),
                )));
                *self.active.lock().unwrap() = Some(Arc::clone(&turn));
                turn
            };
            let turn_handle = TurnHandle(turn.clone());
            self.requests.lock().unwrap().push(request);
            let step = StepHandle(Arc::new(FakeStep {
                id: format!("step-{}", turn.id),
                turn_id: turn.id,
                controller: AbortController::new(),
            }));
            let assignment = future::ready(Ok(StepAssignment {
                turn: turn_handle,
                step,
            }))
            .boxed()
            .shared();
            let abort_turn = Arc::clone(&turn);
            Ok(EnqueueReceipt::new(
                assignment,
                Arc::new(move |reason| abort_turn.cancel(reason)),
            ))
        }

        async fn run(&self, _: LoopRunOptions) -> LoopRunResult {
            unreachable!("the scheduler uses enqueue assignments")
        }

        fn status(&self) -> AgentLoopStatus {
            let active = self.active.lock().unwrap().clone();
            let running = active
                .as_ref()
                .is_some_and(|turn| turn.state() == Some(TurnState::Running));
            AgentLoopStatus {
                state: if running {
                    AgentLoopState::Running
                } else {
                    AgentLoopState::Idle
                },
                active_turn_id: active.filter(|_| running).map(|turn| turn.id),
                pending_turn_ids: Vec::new(),
                has_pending_requests: false,
                active_trace_id: None,
            }
        }

        fn cancel(&self, turn_id: Option<crate::agent::TurnId>, reason: Option<LoopValue>) -> bool {
            self.active
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|turn| turn_id.is_none_or(|id| id == turn.id) && turn.cancel(reason))
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

    fn prompt(id: &str) -> PromptInput {
        PromptInput {
            id: Some(id.into()),
            message: ContextMessage {
                message: Message::new(
                    Role::User,
                    vec![ContentPart::Text {
                        text: id.to_owned(),
                    }],
                    Vec::new(),
                ),
                id: None,
                provider_message_id: None,
                origin: Some(PromptOrigin::User),
                is_error: None,
                note: None,
                attachments: Vec::new(),
            },
        }
    }

    fn scheduler_fixture() -> (
        Arc<SchedulerRuntime>,
        SchedulerClient,
        Arc<FakeLoop>,
        Arc<AgentPromptHooks>,
    ) {
        let events = Arc::new(EventBusService::new());
        let publisher: Arc<dyn DomainEventPublisher> = events.clone();
        let wire = Arc::new(WireService::new(
            "agents/prompt-actor-test",
            AppendLogStoreHandle(Arc::new(MemoryLog)),
            Arc::new(IdentityBlobs),
            publisher,
        ));
        let event_contract: Arc<dyn EventBusContract> = events.clone();
        let context_contract: Arc<dyn AgentContextMemoryServiceContract> = Arc::new(
            AgentContextMemoryService::new(Arc::clone(&wire), Arc::clone(&event_contract)),
        );
        let context = AgentContextMemoryServiceHandle(context_contract);
        let reminders = AgentSystemReminderServiceHandle(Arc::new(
            AgentSystemReminderService::from_handle(context.clone()),
        ));
        let loop_service = FakeLoop::new();
        let loop_contract: Arc<dyn AgentLoopServiceContract> = loop_service.clone();
        let hooks = Arc::new(AgentPromptHooks::default());
        let runtime = SchedulerRuntime::new(
            context,
            reminders,
            Arc::new(InstantiationService::new(ServiceCollection::new())),
            AgentLoopServiceHandle(loop_contract),
            WireServiceHandle(wire),
            EventBusHandle(event_contract),
            Arc::clone(&hooks),
            Arc::new(DisposableStore::new()),
        );
        let scheduler =
            start_scheduler_with_idle_timeout(Arc::clone(&runtime), Duration::from_millis(20));
        (runtime, scheduler, loop_service, hooks)
    }

    async fn wait_for_active(scheduler: &SchedulerClient, prompt_id: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if scheduler
                    .list()
                    .active
                    .as_ref()
                    .is_some_and(|active| active.id == prompt_id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("prompt should become active");
    }

    async fn wait_for_dormant(scheduler: &SchedulerClient) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if scheduler.controller.is_dormant() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("scheduler actor should park after becoming idle");
    }

    #[tokio::test]
    async fn actor_starts_lazily_parks_when_idle_and_resumes_with_the_same_mailbox() {
        let (runtime, scheduler, loop_service, _) = scheduler_fixture();

        // Construction keeps the complete Actor parked and creates no runner.
        assert!(scheduler.controller.is_dormant());
        assert_eq!(scheduler.list(), PromptQueueSnapshot::default());

        let first = scheduler.enqueue(prompt("first-run")).await.unwrap();
        assert_eq!(first.snapshot().state, PromptState::Running);
        loop_service.complete(crate::agent::TurnId::new(1));
        assert_eq!(
            first.completion().await.state,
            PromptCompletionState::Completed
        );
        wait_for_dormant(&scheduler).await;

        // The command sender and watch receiver are stable across parking.  A
        // new runner takes ownership of the already-existing Actor value.
        let second = scheduler.enqueue(prompt("second-run")).await.unwrap();
        assert_eq!(second.snapshot().state, PromptState::Running);
        assert_eq!(scheduler.list().active.unwrap().id, "second-run");
        loop_service.complete(crate::agent::TurnId::new(2));
        assert_eq!(
            second.completion().await.state,
            PromptCompletionState::Completed
        );

        begin_shutdown(&runtime);
        runtime.tasks.wait().await;
    }

    #[tokio::test]
    async fn commands_racing_with_idle_parking_are_not_lost() {
        let (runtime, scheduler, _, _) = scheduler_fixture();

        for _ in 0..10 {
            scheduler.clear().await.unwrap();
            // Aim the burst at the idle boundary.  Whether it observes Running,
            // Parking or Dormant, every reply must still be delivered.
            tokio::time::sleep(Duration::from_millis(18)).await;
            let commands = (0..16).map(|_| {
                let scheduler = scheduler.clone();
                tokio::spawn(async move { scheduler.clear().await })
            });
            let replies = tokio::time::timeout(
                Duration::from_secs(1),
                futures_util::future::join_all(commands),
            )
            .await
            .expect("parking race must not strand a command");
            for reply in replies {
                reply.unwrap().unwrap();
            }
            wait_for_dormant(&scheduler).await;
        }

        begin_shutdown(&runtime);
        runtime.tasks.wait().await;
    }

    #[tokio::test]
    async fn actor_preserves_running_pending_fifo_and_watch_snapshot() {
        let (runtime, scheduler, loop_service, _) = scheduler_fixture();
        let first = scheduler.enqueue(prompt("first")).await.unwrap();
        assert_eq!(first.snapshot().state, PromptState::Running);
        assert_eq!(scheduler.list().active.unwrap().id, "first");

        let second = scheduler.enqueue(prompt("second")).await.unwrap();
        assert_eq!(second.snapshot().state, PromptState::Pending);
        assert_eq!(scheduler.list().pending[0].id, "second");

        loop_service.complete(crate::agent::TurnId::new(1));
        wait_for_active(&scheduler, "second").await;
        assert_eq!(second.snapshot().state, PromptState::Running);
        assert_eq!(
            first.completion().await.state,
            PromptCompletionState::Completed
        );

        begin_shutdown(&runtime);
        runtime.tasks.wait().await;
    }

    #[tokio::test]
    async fn pending_abort_is_idempotent_and_clear_settles_active() {
        let (runtime, scheduler, _, _) = scheduler_fixture();
        let active = scheduler.enqueue(prompt("active")).await.unwrap();
        let pending = scheduler.enqueue(prompt("pending")).await.unwrap();

        assert!(scheduler.abort("pending", None).await);
        assert!(!scheduler.abort("pending", None).await);
        assert_eq!(
            pending.completion().await.state,
            PromptCompletionState::Cancelled
        );

        scheduler.clear().await.unwrap();
        assert_eq!(
            active.completion().await.state,
            PromptCompletionState::Cancelled
        );
        assert_eq!(scheduler.list(), PromptQueueSnapshot::default());

        begin_shutdown(&runtime);
        runtime.tasks.wait().await;
    }

    #[tokio::test]
    async fn steered_child_settles_with_its_parent_turn() {
        let (runtime, scheduler, loop_service, _) = scheduler_fixture();
        let steered_events = Arc::new(Mutex::new(Vec::new()));
        let captured_events = Arc::clone(&steered_events);
        let _subscription = runtime.event_bus.subscribe_type(
            "prompt.steered",
            Arc::new(move |event| captured_events.lock().unwrap().push(event.clone())),
        );
        let parent = scheduler.enqueue(prompt("parent")).await.unwrap();
        let child = scheduler.enqueue(prompt("child")).await.unwrap();

        let steered = scheduler.steer(&["child".into()]).await.unwrap();
        assert_eq!(steered[0].snapshot().state, PromptState::Steered);
        assert_eq!(child.snapshot().state, PromptState::Steered);
        assert!(
            steered_events.lock().unwrap().is_empty(),
            "assignment must not move a prompt into the conversation"
        );

        loop_service.materialize_last_request();
        {
            let events = steered_events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].fields["promptIds"], serde_json::json!(["child"]));
            assert_eq!(events[0].fields["userMessages"][0]["promptId"], "child");
        }

        loop_service.complete(crate::agent::TurnId::new(1));
        assert_eq!(
            parent.completion().await.state,
            PromptCompletionState::Completed
        );
        assert_eq!(
            child.completion().await.state,
            PromptCompletionState::Completed
        );

        begin_shutdown(&runtime);
        runtime.tasks.wait().await;
    }

    #[tokio::test]
    async fn launching_abort_cancels_a_pending_hook_and_settles_the_handle() {
        let (runtime, scheduler, _, hooks) = scheduler_fixture();
        let entered = Arc::new(tokio::sync::Notify::new());
        let hook_entered = Arc::clone(&entered);
        let _registration = hooks
            .on_before_submit_prompt
            .register(
                "pending-hook",
                Arc::new(move |_, _| {
                    let hook_entered = Arc::clone(&hook_entered);
                    Box::pin(async move {
                        hook_entered.notify_one();
                        future::pending::<()>().await;
                        Ok(())
                    })
                }),
                HookRegisterOptions::default(),
            )
            .unwrap();
        let enqueue = tokio::spawn({
            let scheduler = scheduler.clone();
            async move { scheduler.enqueue(prompt("launching")).await }
        });
        entered.notified().await;

        assert!(scheduler.abort("launching", None).await);
        let handle = enqueue.await.unwrap().unwrap();
        assert_eq!(handle.snapshot().state, PromptState::Cancelled);
        assert_eq!(
            handle.completion().await.state,
            PromptCompletionState::Cancelled
        );

        begin_shutdown(&runtime);
        runtime.tasks.wait().await;
    }

    #[tokio::test]
    async fn hook_block_and_failure_settle_without_reaching_the_loop() {
        let (blocked_runtime, blocked_scheduler, _, blocked_hooks) = scheduler_fixture();
        let _blocked_registration = blocked_hooks
            .on_before_submit_prompt
            .register(
                "block-hook",
                Arc::new(|context, _| {
                    context.block = true;
                    Box::pin(async { Ok(()) })
                }),
                HookRegisterOptions::default(),
            )
            .unwrap();
        let blocked = blocked_scheduler.enqueue(prompt("blocked")).await.unwrap();
        assert_eq!(blocked.snapshot().state, PromptState::Blocked);
        assert_eq!(
            blocked.completion().await.state,
            PromptCompletionState::Blocked
        );
        begin_shutdown(&blocked_runtime);
        blocked_runtime.tasks.wait().await;

        let (failed_runtime, failed_scheduler, _, failed_hooks) = scheduler_fixture();
        let _failed_registration = failed_hooks
            .on_before_submit_prompt
            .register(
                "failed-hook",
                Arc::new(|_, _| {
                    Box::pin(async {
                        Err(Box::new(std::io::Error::other("hook failed"))
                            as crate::_base::lifecycle::lifecycle_machine::BoxError)
                    })
                }),
                HookRegisterOptions::default(),
            )
            .unwrap();
        let failed = failed_scheduler.enqueue(prompt("failed")).await.unwrap();
        assert_eq!(failed.snapshot().state, PromptState::Failed);
        assert_eq!(
            failed.completion().await.state,
            PromptCompletionState::Failed
        );
        begin_shutdown(&failed_runtime);
        failed_runtime.tasks.wait().await;
    }

    struct TestFullCompaction {
        hooks: AgentFullCompactionHooks,
        finished: Emitter<FullCompactionTask>,
    }

    impl TestFullCompaction {
        fn new() -> Self {
            Self {
                hooks: AgentFullCompactionHooks::default(),
                finished: Emitter::new(),
            }
        }
    }

    impl crate::_base::di::lifecycle::Disposable for TestFullCompaction {
        fn dispose(&self) -> crate::_base::di::lifecycle::DisposeResult {
            self.finished.dispose()
        }
    }

    impl AgentFullCompactionServiceContract for TestFullCompaction {
        fn compacting(&self) -> Option<FullCompactionTask> {
            None
        }

        fn begin(&self, _input: FullCompactionInput) -> Result<bool, FullCompactionError> {
            Ok(false)
        }

        fn hooks(&self) -> &AgentFullCompactionHooks {
            &self.hooks
        }

        fn on_did_finish_compaction(&self) -> Event<FullCompactionTask> {
            self.finished.event()
        }
    }

    #[test]
    fn compaction_finish_listener_is_installed_once_and_disposed_with_prompt_service() {
        let compaction = Arc::new(TestFullCompaction::new());
        let contract: Arc<dyn AgentFullCompactionServiceContract> = compaction.clone();
        let mut services = ServiceCollection::new();
        services.set_instance(
            AGENT_FULL_COMPACTION_SERVICE_ID,
            Arc::new(AgentFullCompactionServiceHandle(contract)),
        );
        let instantiation = InstantiationService::new(services);
        let cache = Mutex::new(None);
        let disposables = DisposableStore::new();
        let wake_count = Arc::new(AtomicUsize::new(0));

        for _ in 0..2 {
            let wake_count = Arc::clone(&wake_count);
            assert!(
                resolve_full_compaction(&instantiation, &cache, &disposables, move |_| {
                    wake_count.fetch_add(1, Ordering::SeqCst);
                })
                .is_some()
            );
        }

        let task = FullCompactionTask::new(
            AbortController::new(),
            future::pending().boxed().shared(),
            CompactionSource::Manual,
            0,
            Arc::new(Mutex::new(None)),
        );
        compaction.finished.fire(&task);
        assert_eq!(wake_count.load(Ordering::SeqCst), 1);

        disposables.dispose().unwrap();
        compaction.finished.fire(&task);
        assert_eq!(wake_count.load(Ordering::SeqCst), 1);
    }
}
