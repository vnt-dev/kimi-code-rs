//! Agent-scoped extensible fact event bus contract.
//!
//! Original: `packages/agent-core-v2/src/app/event/eventBus.ts`.

use std::{
    any::Any,
    fmt,
    ops::Deref,
    sync::{Arc, OnceLock},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::_base::di::{
    instantiation::ServiceIdentifier,
    lifecycle::{Disposable, DisposableHandle, DisposeResult},
};

pub trait DomainEventPayload: Serialize + DeserializeOwned + Send + Sync + 'static {
    const TYPE: &'static str;
}

trait ErasedDomainEventPayload: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn fields(&self) -> &Map<String, Value>;
}

struct TypedDomainEventPayload<T> {
    value: T,
    fields: OnceLock<Map<String, Value>>,
}

impl<T> ErasedDomainEventPayload for TypedDomainEventPayload<T>
where
    T: DomainEventPayload,
{
    fn as_any(&self) -> &dyn Any {
        &self.value
    }

    fn fields(&self) -> &Map<String, Value> {
        self.fields.get_or_init(|| {
            let Value::Object(fields) =
                serde_json::to_value(&self.value).expect("domain event payload is serializable")
            else {
                panic!("domain event payload must serialize to an object");
            };
            fields
        })
    }
}

struct DynamicDomainEventPayload(Map<String, Value>);

impl ErasedDomainEventPayload for DynamicDomainEventPayload {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fields(&self) -> &Map<String, Value> {
        &self.0
    }
}

#[derive(Clone)]
pub struct DomainEventFields(Arc<dyn ErasedDomainEventPayload>);

impl DomainEventFields {
    fn dynamic(fields: Map<String, Value>) -> Self {
        Self(Arc::new(DynamicDomainEventPayload(fields)))
    }

    fn typed<T>(payload: T) -> Self
    where
        T: DomainEventPayload,
    {
        Self(Arc::new(TypedDomainEventPayload {
            value: payload,
            fields: OnceLock::new(),
        }))
    }

    fn payload<T>(&self) -> Option<&T>
    where
        T: DomainEventPayload,
    {
        self.0.as_any().downcast_ref()
    }
}

impl Deref for DomainEventFields {
    type Target = Map<String, Value>;

    fn deref(&self) -> &Self::Target {
        self.0.fields()
    }
}

impl fmt::Debug for DomainEventFields {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(formatter)
    }
}

impl PartialEq for DomainEventFields {
    fn eq(&self, other: &Self) -> bool {
        self.deref() == other.deref()
    }
}

impl Serialize for DomainEventFields {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.deref().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DomainEventFields {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Map::deserialize(deserializer).map(Self::dynamic)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DomainEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(flatten)]
    pub fields: DomainEventFields,
}

impl DomainEvent {
    pub fn new(event_type: impl Into<String>, mut fields: Map<String, Value>) -> Self {
        fields.remove("type");
        Self {
            event_type: event_type.into(),
            fields: DomainEventFields::dynamic(fields),
        }
    }

    pub fn typed<T>(payload: T) -> Self
    where
        T: DomainEventPayload,
    {
        Self {
            event_type: T::TYPE.into(),
            fields: DomainEventFields::typed(payload),
        }
    }

    pub fn payload<T>(&self) -> Option<&T>
    where
        T: DomainEventPayload,
    {
        self.fields.payload()
    }

    pub fn fields(&self) -> &Map<String, Value> {
        &self.fields
    }

    pub fn with_payload<T, R>(&self, callback: impl FnOnce(&T) -> R) -> Result<R, serde_json::Error>
    where
        T: DomainEventPayload,
    {
        if let Some(payload) = self.payload::<T>() {
            return Ok(callback(payload));
        }
        serde_json::from_value::<T>(Value::Object(self.fields.deref().clone()))
            .map(|payload| callback(&payload))
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
pub type TypedDomainEventHandler<T> = Arc<dyn Fn(&T) + Send + Sync>;

pub trait EventBusContract: Disposable + Send + Sync {
    fn publish(&self, event: DomainEvent);
    /// Subscribes to future events only.
    fn subscribe(&self, handler: DomainEventHandler) -> DisposableHandle;
    /// Replays the unfinished turn, then continues with future events.
    fn subscribe_with_replay(&self, handler: DomainEventHandler) -> DisposableHandle;
    fn subscribe_type(&self, event_type: &str, handler: DomainEventHandler) -> DisposableHandle;
}

pub trait TypedEventBusExt: EventBusContract {
    fn publish_typed<T>(&self, payload: T)
    where
        T: DomainEventPayload,
    {
        self.publish(DomainEvent::typed(payload));
    }

    fn subscribe_typed<T>(&self, handler: TypedDomainEventHandler<T>) -> DisposableHandle
    where
        T: DomainEventPayload,
    {
        self.subscribe_type(
            T::TYPE,
            Arc::new(move |event| {
                let _ = event.with_payload::<T, _>(|payload| handler(payload));
            }),
        )
    }
}

impl<T> TypedEventBusExt for T where T: EventBusContract + ?Sized {}

#[derive(Clone)]
pub struct EventBusHandle(pub Arc<dyn EventBusContract>);

impl Deref for EventBusHandle {
    type Target = dyn EventBusContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for EventBusHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const EVENT_BUS_SERVICE_ID: ServiceIdentifier<EventBusHandle> =
    ServiceIdentifier::new("eventBus");

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[derive(Debug, Deserialize)]
    struct TestEvent {
        value: u64,
    }

    static SERIALIZE_COUNT: AtomicUsize = AtomicUsize::new(0);

    impl Serialize for TestEvent {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            SERIALIZE_COUNT.fetch_add(1, Ordering::Relaxed);
            Map::from_iter([("value".into(), Value::from(self.value))]).serialize(serializer)
        }
    }

    impl DomainEventPayload for TestEvent {
        const TYPE: &'static str = "test.typed";
    }

    #[test]
    fn typed_payload_stays_in_rust_until_json_is_requested() {
        SERIALIZE_COUNT.store(0, Ordering::Relaxed);
        let event = DomainEvent::typed(TestEvent { value: 7 });
        assert_eq!(event.payload::<TestEvent>().unwrap().value, 7);
        assert_eq!(SERIALIZE_COUNT.load(Ordering::Relaxed), 0);

        assert_eq!(event.fields["value"], 7);
        assert_eq!(event.fields["value"], 7);
        assert_eq!(SERIALIZE_COUNT.load(Ordering::Relaxed), 1);
        assert_eq!(
            event.into_value(),
            serde_json::json!({"type": "test.typed", "value": 7})
        );
        assert_eq!(SERIALIZE_COUNT.load(Ordering::Relaxed), 1);
    }
}
