//! `capability` domain (L3) — `CapabilityServiceContract` contract.
//!
//! Manages the built-in product capabilities (`kimi-cu`, `kimi-webbridge`):
//! layered readiness detection and idempotent install orchestration. Entries
//! are hardcoded in a closed registry — install sources are fixed official
//! CDN URLs, never client-supplied.
//!
//! Original: `packages/agent-core-v2/src/app/capability/capability.ts`.
//!
//! Rust adaptation: named `contract` to avoid a `capability::capability` module.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::di::instantiation::ServiceIdentifier;

use super::types::CapabilityStatus;

pub type CapabilityServiceError = Box<dyn Error + Send + Sync>;
pub type CapabilityServiceResult<T> = Result<T, CapabilityServiceError>;

#[async_trait]
pub trait CapabilityServiceContract: Send + Sync {
    async fn list_capabilities(&self) -> CapabilityServiceResult<Vec<CapabilityStatus>>;

    async fn get_capability(&self, id: &str) -> CapabilityServiceResult<CapabilityStatus>;

    async fn install_capability(&self, id: &str) -> CapabilityServiceResult<CapabilityStatus>;
}

#[derive(Clone)]
pub struct CapabilityServiceHandle(pub Arc<dyn CapabilityServiceContract>);

impl Deref for CapabilityServiceHandle {
    type Target = dyn CapabilityServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const CAPABILITY_SERVICE_ID: ServiceIdentifier<CapabilityServiceHandle> =
    ServiceIdentifier::new("capabilityService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_the_original() {
        assert_eq!(CAPABILITY_SERVICE_ID.to_string(), "capabilityService");
    }
}
