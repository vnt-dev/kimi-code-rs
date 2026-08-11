//! Agent-scoped full-stream and per-type event bus.
//!
//! Original: `packages/agent-core-v2/src/app/event/eventBusService.ts`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::_base::{
    di::{
        descriptors::SyncDescriptor,
        lifecycle::{Disposable, DisposableHandle, DisposeResult},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    errors::unexpected_error::safely_call_listener,
    event::Emitter,
};

use super::event_bus::{
    DomainEvent, DomainEventHandler, EVENT_BUS_SERVICE_ID, EventBusContract, EventBusHandle,
};

pub struct EventBusService {
    all: Arc<Emitter<SequencedDomainEvent>>,
    per_type: Mutex<HashMap<String, Arc<Emitter<DomainEvent>>>>,
    replay: Mutex<ReplayState>,
}

#[derive(Clone)]
struct SequencedDomainEvent {
    seq: u64,
    event: DomainEvent,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ReplayPhase {
    #[default]
    Idle,
    Pending,
    Active(crate::agent::TurnId),
}

#[derive(Default)]
struct ReplayState {
    next_seq: u64,
    phase: ReplayPhase,
    pending_prompt_id: Option<String>,
    events: Vec<SequencedDomainEvent>,
}

#[derive(Clone, Copy)]
enum ReplayCleanup {
    None,
    Through(u64),
}

struct ReplayDeliveryState {
    replaying: bool,
    replay_watermark: u64,
    queued: Vec<SequencedDomainEvent>,
}

struct ReplayDelivery {
    handler: DomainEventHandler,
    state: Mutex<ReplayDeliveryState>,
}

impl ReplayDelivery {
    fn new(handler: DomainEventHandler) -> Self {
        Self {
            handler,
            state: Mutex::new(ReplayDeliveryState {
                replaying: true,
                replay_watermark: 0,
                queued: Vec::new(),
            }),
        }
    }

    fn accept(&self, event: &SequencedDomainEvent) {
        let mut state = self.state.lock().unwrap();
        if event.seq <= state.replay_watermark {
            return;
        }
        if state.replaying {
            state.queued.push(event.clone());
            return;
        }
        drop(state);
        safely_call_listener(|| (self.handler)(&event.event));
    }

    fn replay(&self, mut events: Vec<SequencedDomainEvent>) {
        events.sort_by_key(|event| event.seq);
        events.dedup_by_key(|event| event.seq);
        let mut watermark = 0;
        for event in events {
            safely_call_listener(|| (self.handler)(&event.event));
            watermark = event.seq;
        }

        loop {
            let queued = {
                let mut state = self.state.lock().unwrap();
                state.replay_watermark = watermark;
                if state.queued.is_empty() {
                    state.replaying = false;
                    return;
                }
                std::mem::take(&mut state.queued)
            };
            let mut queued = queued;
            queued.sort_by_key(|event| event.seq);
            queued.dedup_by_key(|event| event.seq);
            for event in queued {
                if event.seq <= watermark {
                    continue;
                }
                safely_call_listener(|| (self.handler)(&event.event));
                watermark = event.seq;
            }
        }
    }
}

impl Default for EventBusService {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBusService {
    pub fn new() -> Self {
        Self {
            all: Arc::new(Emitter::new()),
            per_type: Mutex::new(HashMap::new()),
            replay: Mutex::new(ReplayState::default()),
        }
    }
}

impl ReplayState {
    fn record(&mut self, event: DomainEvent) -> (SequencedDomainEvent, ReplayCleanup) {
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .expect("event bus sequence exhausted");
        let published = SequencedDomainEvent {
            seq: self.next_seq,
            event,
        };
        let mut cleanup = ReplayCleanup::None;

        match published.event.event_type.as_str() {
            "prompt.submitted" => {
                if self.phase == ReplayPhase::Idle {
                    self.events.clear();
                    self.phase = ReplayPhase::Pending;
                    self.pending_prompt_id = event_prompt_id(&published.event).map(str::to_owned);
                }
                self.events.push(published.clone());
            }
            "turn.started" => {
                let turn_id = event_turn_id(&published.event).unwrap_or_default();
                if matches!(self.phase, ReplayPhase::Idle | ReplayPhase::Active(_)) {
                    self.events.clear();
                }
                self.phase = ReplayPhase::Active(turn_id);
                self.pending_prompt_id = None;
                self.events.push(published.clone());
            }
            "turn.ended" => {
                if self.phase != ReplayPhase::Idle {
                    self.events.push(published.clone());
                }
                if matches!(
                    self.phase,
                    ReplayPhase::Active(turn_id)
                        if event_turn_id(&published.event).is_none_or(|ended| ended == turn_id)
                ) {
                    cleanup = ReplayCleanup::Through(published.seq);
                }
            }
            "prompt.completed" | "prompt.aborted" if self.phase == ReplayPhase::Pending => {
                self.events.push(published.clone());
                let terminal_prompt_id = event_prompt_id(&published.event);
                if self.pending_prompt_id.as_deref().is_none()
                    || terminal_prompt_id.is_none()
                    || self.pending_prompt_id.as_deref() == terminal_prompt_id
                {
                    cleanup = ReplayCleanup::Through(published.seq);
                }
            }
            _ if self.phase != ReplayPhase::Idle => self.events.push(published.clone()),
            _ => {}
        }

        (published, cleanup)
    }

    fn cleanup(&mut self, cleanup: ReplayCleanup) {
        let ReplayCleanup::Through(seq) = cleanup else {
            return;
        };
        self.events.retain(|event| event.seq > seq);
        (self.phase, self.pending_prompt_id) = replay_position(&self.events);
        if self.phase == ReplayPhase::Idle {
            self.events.clear();
            self.pending_prompt_id = None;
        }
    }
}

fn replay_position(events: &[SequencedDomainEvent]) -> (ReplayPhase, Option<String>) {
    let mut phase = ReplayPhase::Idle;
    let mut pending_prompt_id = None;
    for published in events {
        match published.event.event_type.as_str() {
            "prompt.submitted" if phase == ReplayPhase::Idle => {
                phase = ReplayPhase::Pending;
                pending_prompt_id = event_prompt_id(&published.event).map(str::to_owned);
            }
            "turn.started" => {
                phase = ReplayPhase::Active(event_turn_id(&published.event).unwrap_or_default());
                pending_prompt_id = None;
            }
            "turn.ended" => {
                phase = ReplayPhase::Idle;
                pending_prompt_id = None;
            }
            "prompt.completed" | "prompt.aborted" if phase == ReplayPhase::Pending => {
                let terminal_prompt_id = event_prompt_id(&published.event);
                if pending_prompt_id.as_deref().is_none()
                    || terminal_prompt_id.is_none()
                    || pending_prompt_id.as_deref() == terminal_prompt_id
                {
                    phase = ReplayPhase::Idle;
                    pending_prompt_id = None;
                }
            }
            _ => {}
        }
    }
    (phase, pending_prompt_id)
}

fn event_turn_id(event: &DomainEvent) -> Option<crate::agent::TurnId> {
    event
        .fields
        .get("turnId")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn event_prompt_id(event: &DomainEvent) -> Option<&str> {
    event
        .fields
        .get("promptId")
        .and_then(serde_json::Value::as_str)
}

impl EventBusContract for EventBusService {
    fn publish(&self, event: DomainEvent) {
        let (published, cleanup) = self.replay.lock().unwrap().record(event);
        self.all.fire(&published);
        let typed = self
            .per_type
            .lock()
            .unwrap()
            .get(&published.event.event_type)
            .cloned();
        if let Some(typed) = typed {
            typed.fire(&published.event);
        }
        self.replay.lock().unwrap().cleanup(cleanup);
    }

    fn subscribe(&self, handler: DomainEventHandler) -> DisposableHandle {
        self.all
            .event()
            .subscribe(move |published| handler(&published.event))
    }

    fn subscribe_with_replay(&self, handler: DomainEventHandler) -> DisposableHandle {
        let delivery = Arc::new(ReplayDelivery::new(handler));
        let (subscription, events) = {
            let replay = self.replay.lock().unwrap();
            let delivery_for_live = Arc::clone(&delivery);
            let subscription = self
                .all
                .event()
                .subscribe(move |event| delivery_for_live.accept(event));
            (subscription, replay.events.clone())
        };
        delivery.replay(events);
        subscription
    }

    fn subscribe_type(&self, event_type: &str, handler: DomainEventHandler) -> DisposableHandle {
        let emitter = self
            .per_type
            .lock()
            .unwrap()
            .entry(event_type.into())
            .or_insert_with(|| Arc::new(Emitter::new()))
            .clone();
        emitter.event().subscribe(move |event| handler(event))
    }
}

impl Disposable for EventBusService {
    fn dispose(&self) -> DisposeResult {
        self.all.dispose()?;
        let emitters = std::mem::take(&mut *self.per_type.lock().unwrap());
        for emitter in emitters.into_values() {
            emitter.dispose()?;
        }
        Ok(())
    }
}

impl Drop for EventBusService {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

/// Registers the eager Agent-scoped domain event bus.
pub fn register_event_bus_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        EVENT_BUS_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn EventBusContract> = Arc::new(EventBusService::new());
            Ok(EventBusHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "event",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Barrier, atomic::AtomicBool},
        thread,
    };

    use serde::{Deserialize, Serialize};
    use serde_json::{Map, Value};

    use super::*;
    use crate::app::event::event_bus::{DomainEventPayload, TypedEventBusExt};

    #[test]
    fn full_stream_precedes_matching_type_and_disposal_stops_delivery() {
        let bus = EventBusService::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let all_seen = Arc::clone(&seen);
        let _all = bus.subscribe(Arc::new(move |event| {
            all_seen
                .lock()
                .unwrap()
                .push(format!("all:{}", event.event_type));
        }));
        let typed_seen = Arc::clone(&seen);
        let typed = bus.subscribe_type(
            "test.a",
            Arc::new(move |event| {
                typed_seen
                    .lock()
                    .unwrap()
                    .push(format!("typed:{}", event.fields["x"]));
            }),
        );
        bus.publish(DomainEvent::new(
            "test.a",
            Map::from_iter([("x".into(), Value::from(1))]),
        ));
        bus.publish(DomainEvent::new("test.b", Map::new()));
        typed.dispose().unwrap();
        bus.publish(DomainEvent::new(
            "test.a",
            Map::from_iter([("x".into(), Value::from(2))]),
        ));
        assert_eq!(
            *seen.lock().unwrap(),
            ["all:test.a", "typed:1", "all:test.b", "all:test.a"]
        );
    }

    #[test]
    fn publishing_without_subscribers_and_disposing_are_safe() {
        let bus = EventBusService::new();
        bus.publish(DomainEvent::new("none", serde_json::Map::new()));
        bus.dispose().unwrap();
        bus.dispose().unwrap();
        bus.publish(DomainEvent::new("after", serde_json::Map::new()));
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct TestTypedEvent {
        x: u64,
    }

    impl DomainEventPayload for TestTypedEvent {
        const TYPE: &'static str = "test.typed";
    }

    #[test]
    fn typed_subscription_accepts_rust_and_dynamic_payloads() {
        let bus = EventBusService::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_by_handler = Arc::clone(&seen);
        let _subscription = bus.subscribe_typed::<TestTypedEvent>(Arc::new(move |event| {
            seen_by_handler.lock().unwrap().push(event.x);
        }));

        bus.publish_typed(TestTypedEvent { x: 1 });
        bus.publish(DomainEvent::new(
            "test.typed",
            Map::from_iter([("x".into(), Value::from(2))]),
        ));

        assert_eq!(*seen.lock().unwrap(), [1, 2]);
    }

    fn turn_event(event_type: &str, turn_id: crate::agent::TurnId) -> DomainEvent {
        DomainEvent::new(
            event_type,
            Map::from_iter([("turnId".into(), Value::from(turn_id.get()))]),
        )
    }

    #[test]
    fn replay_subscription_backfills_only_the_current_turn() {
        let bus = EventBusService::new();
        bus.publish(DomainEvent::new(
            "prompt.submitted",
            Map::from_iter([("promptId".into(), Value::String("p1".into()))]),
        ));
        bus.publish(turn_event("turn.started", crate::agent::TurnId::new(1)));
        bus.publish(DomainEvent::new(
            "assistant.delta",
            Map::from_iter([
                ("turnId".into(), Value::from(1)),
                ("delta".into(), Value::String("before".into())),
            ]),
        ));

        let ordinary = Arc::new(Mutex::new(Vec::new()));
        let ordinary_sink = Arc::clone(&ordinary);
        let _ordinary = bus.subscribe(Arc::new(move |event| {
            ordinary_sink.lock().unwrap().push(event.event_type.clone());
        }));
        let replayed = Arc::new(Mutex::new(Vec::new()));
        let replayed_sink = Arc::clone(&replayed);
        let _replay = bus.subscribe_with_replay(Arc::new(move |event| {
            replayed_sink.lock().unwrap().push(event.event_type.clone());
        }));

        assert!(ordinary.lock().unwrap().is_empty());
        assert_eq!(
            *replayed.lock().unwrap(),
            ["prompt.submitted", "turn.started", "assistant.delta"]
        );

        bus.publish(turn_event(
            "turn.step.started",
            crate::agent::TurnId::new(1),
        ));
        assert_eq!(*ordinary.lock().unwrap(), ["turn.step.started"]);
        assert_eq!(
            *replayed.lock().unwrap(),
            [
                "prompt.submitted",
                "turn.started",
                "assistant.delta",
                "turn.step.started"
            ]
        );

        bus.publish(turn_event("turn.ended", crate::agent::TurnId::new(1)));
        let after_end = Arc::new(Mutex::new(Vec::new()));
        let after_end_sink = Arc::clone(&after_end);
        let _after_end = bus.subscribe_with_replay(Arc::new(move |event| {
            after_end_sink
                .lock()
                .unwrap()
                .push(event.event_type.clone());
        }));
        assert!(after_end.lock().unwrap().is_empty());

        bus.publish(turn_event("turn.started", crate::agent::TurnId::new(2)));
        let next_turn = Arc::new(Mutex::new(Vec::new()));
        let next_turn_sink = Arc::clone(&next_turn);
        let _next_turn = bus.subscribe_with_replay(Arc::new(move |event| {
            next_turn_sink
                .lock()
                .unwrap()
                .push(event.event_type.clone());
        }));
        assert_eq!(*next_turn.lock().unwrap(), ["turn.started"]);
    }

    #[test]
    fn pre_turn_terminal_prompt_event_clears_replay() {
        let bus = EventBusService::new();
        bus.publish(DomainEvent::new("prompt.submitted", Map::new()));
        bus.publish(DomainEvent::new("prompt.aborted", Map::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let _subscription = bus.subscribe_with_replay(Arc::new(move |event| {
            sink.lock().unwrap().push(event.event_type.clone());
        }));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn pre_turn_terminal_event_only_clears_its_matching_prompt() {
        let bus = EventBusService::new();
        bus.publish(DomainEvent::new(
            "prompt.submitted",
            Map::from_iter([("promptId".into(), Value::String("p1".into()))]),
        ));
        bus.publish(DomainEvent::new(
            "prompt.aborted",
            Map::from_iter([("promptId".into(), Value::String("other".into()))]),
        ));

        let before_match = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&before_match);
        let _subscription = bus.subscribe_with_replay(Arc::new(move |event| {
            sink.lock().unwrap().push(event.event_type.clone());
        }));
        assert_eq!(
            *before_match.lock().unwrap(),
            ["prompt.submitted", "prompt.aborted"]
        );

        bus.publish(DomainEvent::new(
            "prompt.completed",
            Map::from_iter([("promptId".into(), Value::String("p1".into()))]),
        ));
        let after_match = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&after_match);
        let _subscription = bus.subscribe_with_replay(Arc::new(move |event| {
            sink.lock().unwrap().push(event.event_type.clone());
        }));
        assert!(after_match.lock().unwrap().is_empty());
    }

    #[test]
    fn replay_handoff_queues_reentrant_publications_without_duplicates() {
        let bus = Arc::new(EventBusService::new());
        bus.publish(turn_event("turn.started", crate::agent::TurnId::new(7)));
        bus.publish(DomainEvent::new(
            "assistant.delta",
            Map::from_iter([("delta".into(), Value::String("before".into()))]),
        ));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let bus_for_handler = Arc::clone(&bus);
        let published = Arc::new(AtomicBool::new(false));
        let published_for_handler = Arc::clone(&published);
        let _subscription = bus.subscribe_with_replay(Arc::new(move |event| {
            sink.lock().unwrap().push(event.event_type.clone());
            if event.event_type == "turn.started"
                && !published_for_handler.swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                bus_for_handler.publish(DomainEvent::new(
                    "thinking.delta",
                    Map::from_iter([("delta".into(), Value::String("during".into()))]),
                ));
            }
        }));

        assert_eq!(
            *seen.lock().unwrap(),
            ["turn.started", "assistant.delta", "thinking.delta"]
        );
    }

    #[test]
    fn concurrent_replay_handoff_delivers_every_event_once() {
        let bus = Arc::new(EventBusService::new());
        bus.publish(turn_event("turn.started", crate::agent::TurnId::new(9)));
        let barrier = Arc::new(Barrier::new(2));
        let publisher_bus = Arc::clone(&bus);
        let publisher_barrier = Arc::clone(&barrier);
        let publisher = thread::spawn(move || {
            for index in 0..100 {
                publisher_bus.publish(DomainEvent::new(
                    "assistant.delta",
                    Map::from_iter([("index".into(), Value::from(index))]),
                ));
                if index == 49 {
                    publisher_barrier.wait();
                }
            }
        });
        barrier.wait();

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let _subscription = bus.subscribe_with_replay(Arc::new(move |event| {
            if event.event_type == "assistant.delta" {
                sink.lock()
                    .unwrap()
                    .push(event.fields["index"].as_u64().unwrap());
            }
        }));
        publisher.join().unwrap();

        let mut values = seen.lock().unwrap().clone();
        values.sort_unstable();
        assert_eq!(values, (0..100).collect::<Vec<_>>());
    }
}
