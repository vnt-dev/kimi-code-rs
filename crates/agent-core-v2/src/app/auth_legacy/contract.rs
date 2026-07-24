use crate::_base::di::instantiation::ServiceIdentifier;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{error::Error, ops::Deref, sync::Arc};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedProviderStatus {
    Authenticated,
    Expired,
    Revoked,
    Unauthenticated,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManagedProviderSummary {
    pub name: String,
    pub status: ManagedProviderStatus,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthSummary {
    pub ready: bool,
    pub providers_count: u64,
    pub default_model: Option<String>,
    pub managed_provider: Option<ManagedProviderSummary>,
}
pub type AuthLegacyResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
#[async_trait]
pub trait AuthLegacyServiceContract: Send + Sync {
    async fn get(&self) -> AuthLegacyResult<AuthSummary>;
}
#[derive(Clone)]
pub struct AuthLegacyServiceHandle(pub Arc<dyn AuthLegacyServiceContract>);
impl Deref for AuthLegacyServiceHandle {
    type Target = dyn AuthLegacyServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const AUTH_LEGACY_SERVICE_ID: ServiceIdentifier<AuthLegacyServiceHandle> =
    ServiceIdentifier::new("authLegacyService");
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v1_wire_fields_are_preserved() {
        let summary = AuthSummary {
            ready: true,
            providers_count: 1,
            default_model: None,
            managed_provider: None,
        };
        assert_eq!(
            serde_json::to_value(summary).unwrap(),
            serde_json::json!({"ready":true,"providers_count":1,"default_model":null,"managed_provider":null})
        );
        assert_eq!(AUTH_LEGACY_SERVICE_ID.to_string(), "authLegacyService");
    }
}
