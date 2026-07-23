//! Agent-scoped full-stream and per-type event bus.
//!
//! Original: `packages/agent-core-v2/src/app/event/eventBusService.ts`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::_base::{
    di::lifecycle::{Disposable, DisposableHandle, DisposeResult},
    event::Emitter,
};

use super::event_bus::{DomainEvent, DomainEventHandler, EventBusContract};

pub struct EventBusService {
    all: Arc<Emitter<DomainEvent>>,
    per_type: Mutex<HashMap<String, Arc<Emitter<DomainEvent>>>>,
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
        }
    }
}

impl EventBusContract for EventBusService {
    fn publish(&self, event: DomainEvent) {
        self.all.fire(&event);
        let typed = self
            .per_type
            .lock()
            .unwrap()
            .get(&event.event_type)
            .cloned();
        if let Some(typed) = typed {
            typed.fire(&event);
        }
    }

    fn subscribe(&self, handler: DomainEventHandler) -> DisposableHandle {
        self.all.event().subscribe(move |event| handler(event))
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

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;

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
}
