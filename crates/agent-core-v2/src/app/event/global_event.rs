//! Process-wide event service contract.
//!
//! Original: `packages/agent-core-v2/src/app/event/event.ts`.

use std::{ops::Deref, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::_base::{
    di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposableHandle, DisposeResult},
    },
    event::{Event, Listener},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GlobalDomainEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Value,
}

pub trait EventServiceContract: Disposable + Send + Sync {
    fn on_did_publish(&self) -> Event<GlobalDomainEvent>;
    fn publish(&self, event: GlobalDomainEvent);
    fn subscribe(&self, handler: Listener<GlobalDomainEvent>) -> DisposableHandle;
}

#[derive(Clone)]
pub struct EventServiceHandle(pub Arc<dyn EventServiceContract>);

impl Deref for EventServiceHandle {
    type Target = dyn EventServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for EventServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

// Original: IEventService decorator identity.
pub const EVENT_SERVICE_ID: ServiceIdentifier<EventServiceHandle> =
    ServiceIdentifier::new("eventService");
