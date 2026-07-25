//! `/init` session service contract.
//!
//! Original: `session/sessionInit/sessionInit.ts`, `ISessionInitService`.
//!
//! The concrete implementation belongs in `service.rs` once the lifecycle,
//! subagent, profile, reminder, and wire service boundaries it coordinates are
//! all available.  This contract intentionally preserves the source split:
//! generating `AGENTS.md` is asynchronous, while cancelling an in-flight
//! operation is immediate and idempotent.

use std::{ops::Deref, sync::Arc};

use futures_util::future::BoxFuture;

use crate::_base::{di::instantiation::ServiceIdentifier, lifecycle::lifecycle_machine::BoxError};

pub trait SessionInitServiceContract: Send + Sync {
    /// Original: `ISessionInitService.generateAgentsMd()`.
    ///
    /// Drives the `/init` operation and completes only after the generated
    /// `AGENTS.md` has been added back to the main agent's context.
    fn generate_agents_md(&self) -> BoxFuture<'static, Result<(), BoxError>>;

    /// Original: `ISessionInitService.cancelInit()`.
    ///
    /// This is a no-op while no `/init` run is active.
    fn cancel_init(&self);
}

#[derive(Clone)]
pub struct SessionInitServiceHandle(pub Arc<dyn SessionInitServiceContract>);

impl Deref for SessionInitServiceHandle {
    type Target = dyn SessionInitServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_INIT_SERVICE_ID: ServiceIdentifier<SessionInitServiceHandle> =
    ServiceIdentifier::new("sessionInitService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_source_decorator() {
        assert_eq!(SESSION_INIT_SERVICE_ID.to_string(), "sessionInitService");
    }
}
