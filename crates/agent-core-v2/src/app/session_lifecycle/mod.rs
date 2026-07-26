//! Session lifecycle domain.
//! Original: `packages/agent-core-v2/src/app/sessionLifecycle`.

pub mod contract;
pub mod service;

pub use contract::*;
pub use service::{SessionLifecycleService, register_session_lifecycle_service};
