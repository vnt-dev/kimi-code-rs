//! Provider configuration service contract.
//!
//! Original: `packages/agent-core-v2/src/kosong/provider/provider.ts`,
//! `IProviderService`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::{
    di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposeResult},
    },
    event::Event,
};

use super::config::{ProviderConfig, ProvidersChangedEvent, ProvidersSection};

#[derive(Debug, thiserror::Error)]
pub enum ProviderServiceError {
    #[error(transparent)]
    Config(#[from] crate::app::config::ConfigServiceError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type ProviderServiceResult<T> = Result<T, ProviderServiceError>;

#[async_trait]
pub trait ProviderServiceContract: Disposable + Send + Sync {
    async fn ready(&self) -> ProviderServiceResult<()>;
    fn on_did_change_providers(&self) -> Event<ProvidersChangedEvent>;
    fn get(&self, name: &str) -> Option<ProviderConfig>;
    fn list(&self) -> ProvidersSection;
    async fn set(&self, name: &str, config: ProviderConfig) -> ProviderServiceResult<()>;
    async fn delete(&self, name: &str) -> ProviderServiceResult<()>;
}

#[derive(Clone)]
pub struct ProviderServiceHandle(pub Arc<dyn ProviderServiceContract>);

impl Deref for ProviderServiceHandle {
    type Target = dyn ProviderServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for ProviderServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const PROVIDER_SERVICE_ID: ServiceIdentifier<ProviderServiceHandle> =
    ServiceIdentifier::new("providerService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_matches_the_original_decorator() {
        assert_eq!(PROVIDER_SERVICE_ID.to_string(), "providerService");
    }
}
