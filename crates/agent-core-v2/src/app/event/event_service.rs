//! Process-wide event service implementation.
//!
//! Original: `packages/agent-core-v2/src/app/event/eventService.ts`.

use std::sync::Arc;

use crate::_base::{
    di::lifecycle::{Disposable, DisposableHandle, DisposeResult},
    event::{Emitter, Event, Listener},
};

use super::global_event::{EventServiceContract, GlobalDomainEvent};

pub struct EventService {
    emitter: Arc<Emitter<GlobalDomainEvent>>,
}

impl Default for EventService {
    fn default() -> Self {
        Self::new()
    }
}

impl EventService {
    pub fn new() -> Self {
        Self {
            emitter: Arc::new(Emitter::new()),
        }
    }
}

impl EventServiceContract for EventService {
    fn on_did_publish(&self) -> Event<GlobalDomainEvent> {
        self.emitter.event()
    }

    fn publish(&self, event: GlobalDomainEvent) {
        self.emitter.fire(&event);
    }

    fn subscribe(&self, handler: Listener<GlobalDomainEvent>) -> DisposableHandle {
        self.emitter.event().subscribe(move |event| handler(event))
    }
}

impl Disposable for EventService {
    fn dispose(&self) -> DisposeResult {
        self.emitter.dispose()
    }
}

impl Drop for EventService {
    fn drop(&mut self) {
        let _ = self.emitter.dispose();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn publish_event_stream_subscribe_and_unsubscribe_share_one_emitter() {
        let service = EventService::new();
        let received = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&received);
        let subscription = service.subscribe(Arc::new(move |event| {
            first
                .lock()
                .unwrap()
                .push(format!("sub:{}", event.event_type));
        }));
        let second = Arc::clone(&received);
        let event_subscription = service.on_did_publish().subscribe(move |event| {
            second
                .lock()
                .unwrap()
                .push(format!("event:{}", event.event_type));
        });
        service.publish(GlobalDomainEvent {
            event_type: "a".into(),
            payload: serde_json::Value::Null,
        });
        subscription.dispose().unwrap();
        event_subscription.dispose().unwrap();
        service.publish(GlobalDomainEvent {
            event_type: "b".into(),
            payload: serde_json::Value::Null,
        });
        assert_eq!(*received.lock().unwrap(), ["sub:a", "event:a"]);
    }
}
