//! Agent-scoped extensible fact event bus contract.
//!
//! Original: `packages/agent-core-v2/src/app/event/eventBus.ts`.

use std::{fmt, ops::Deref, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::_base::di::{instantiation::ServiceIdentifier, lifecycle::DisposableHandle};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DomainEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl DomainEvent {
    pub fn new(event_type: impl Into<String>, mut fields: Map<String, Value>) -> Self {
        fields.remove("type");
        Self {
            event_type: event_type.into(),
            fields,
        }
    }

    pub fn into_value(self) -> Value {
        serde_json::to_value(self).expect("DomainEvent is always JSON serializable")
    }
}

impl TryFrom<Value> for DomainEvent {
    type Error = DomainEventError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|_| DomainEventError)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DomainEventError;

impl fmt::Display for DomainEventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("domain event must be an object with a string type")
    }
}

impl std::error::Error for DomainEventError {}

pub type DomainEventHandler = std::sync::Arc<dyn Fn(&DomainEvent) + Send + Sync>;

pub trait EventBusContract: Send + Sync {
    fn publish(&self, event: DomainEvent);
    fn subscribe(&self, handler: DomainEventHandler) -> DisposableHandle;
    fn subscribe_type(&self, event_type: &str, handler: DomainEventHandler) -> DisposableHandle;
}

#[derive(Clone)]
pub struct EventBusHandle(pub Arc<dyn EventBusContract>);

impl Deref for EventBusHandle {
    type Target = dyn EventBusContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const EVENT_BUS_SERVICE_ID: ServiceIdentifier<EventBusHandle> =
    ServiceIdentifier::new("eventBus");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_round_trips_flattened_payload_and_rejects_missing_type() {
        let event = DomainEvent::new("test.a", Map::from_iter([("x".into(), Value::from(1))]));
        let value = event.clone().into_value();
        assert_eq!(value, serde_json::json!({"type": "test.a", "x": 1}));
        assert_eq!(DomainEvent::try_from(value).unwrap(), event);
        assert!(DomainEvent::try_from(serde_json::json!({"x": 1})).is_err());
        assert_eq!(EVENT_BUS_SERVICE_ID.to_string(), "eventBus");
    }
}
