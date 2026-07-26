//! Side-question (`btw`) session service contract.
//!
//! Original: `packages/agent-core-v2/src/session/btw/btw.ts`.

use std::{ops::Deref, sync::Arc};

use futures_util::future::BoxFuture;

use crate::_base::{di::instantiation::ServiceIdentifier, lifecycle::lifecycle_machine::BoxError};

pub trait SessionBtwServiceContract: Send + Sync {
    fn start(&self) -> BoxFuture<'static, Result<String, BoxError>>;
}

#[derive(Clone)]
pub struct SessionBtwServiceHandle(pub Arc<dyn SessionBtwServiceContract>);

impl Deref for SessionBtwServiceHandle {
    type Target = dyn SessionBtwServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_BTW_SERVICE_ID: ServiceIdentifier<SessionBtwServiceHandle> =
    ServiceIdentifier::new("sessionBtwService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_source_decorator() {
        assert_eq!(SESSION_BTW_SERVICE_ID.to_string(), "sessionBtwService");
    }
}
