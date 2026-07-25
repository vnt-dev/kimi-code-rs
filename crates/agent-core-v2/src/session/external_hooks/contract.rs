//! Session-scoped external-hook observer contract.
//!
//! Original: `packages/agent-core-v2/src/session/externalHooks/externalHooks.ts`,
//! `ISessionExternalHooksService`.

use std::{ops::Deref, sync::Arc};

use crate::_base::di::{
    instantiation::ServiceIdentifier,
    lifecycle::{Disposable, DisposeResult},
};

/// Marker contract whose behavior is installed during eager construction.
pub trait SessionExternalHooksServiceContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct SessionExternalHooksServiceHandle(pub Arc<dyn SessionExternalHooksServiceContract>);

impl Deref for SessionExternalHooksServiceHandle {
    type Target = dyn SessionExternalHooksServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for SessionExternalHooksServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const SESSION_EXTERNAL_HOOKS_SERVICE_ID: ServiceIdentifier<SessionExternalHooksServiceHandle> =
    ServiceIdentifier::new("sessionExternalHooksService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_source_contract() {
        assert_eq!(
            SESSION_EXTERNAL_HOOKS_SERVICE_ID.to_string(),
            "sessionExternalHooksService"
        );
    }
}
