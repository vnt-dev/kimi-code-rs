//! Process-wide event service contract.
//!
//! Original: `packages/agent-core-v2/src/app/event/event.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::_base::{
    di::{instantiation::ServiceIdentifier, lifecycle::DisposableHandle},
    event::{Event, Listener},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GlobalDomainEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Value,
}

pub trait EventServiceContract: Send + Sync {
    fn on_did_publish(&self) -> Event<GlobalDomainEvent>;
    fn publish(&self, event: GlobalDomainEvent);
    fn subscribe(&self, handler: Listener<GlobalDomainEvent>) -> DisposableHandle;
}

pub const EVENT_SERVICE_ID: ServiceIdentifier<dyn EventServiceContract> =
    ServiceIdentifier::new("eventService");
