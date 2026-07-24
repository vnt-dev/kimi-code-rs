pub mod event_bus;
pub mod event_bus_service;
pub mod event_service;
pub mod global_event;

pub use event_service::{EventService, register_event_service};
pub use global_event::{
    EVENT_SERVICE_ID, EventServiceContract, EventServiceHandle, GlobalDomainEvent,
};
