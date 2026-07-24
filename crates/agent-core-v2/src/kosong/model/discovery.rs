//! Provider-model discovery contract and wire payloads.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/discovery.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::_base::di::instantiation::ServiceIdentifier;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderRefreshChange {
    pub provider_id: String,
    pub provider_name: String,
    pub added: u64,
    pub removed: u64,
}

impl ProviderRefreshChange {
    // Original: providerRefreshChangeSchema.
    pub fn validate(&self) -> Result<(), DiscoveryValidationError> {
        require_non_empty("provider_id", &self.provider_id)?;
        require_non_empty("provider_name", &self.provider_name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderRefreshFailure {
    pub provider: String,
    pub reason: String,
}

impl ProviderRefreshFailure {
    // Original: providerRefreshFailureSchema.
    pub fn validate(&self) -> Result<(), DiscoveryValidationError> {
        require_non_empty("provider", &self.provider)?;
        require_non_empty("reason", &self.reason)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefreshProviderModelsResponse {
    pub changed: Vec<ProviderRefreshChange>,
    pub unchanged: Vec<String>,
    pub failed: Vec<ProviderRefreshFailure>,
}

impl RefreshProviderModelsResponse {
    // Original: refreshProviderModelsResponseSchema.
    pub fn validate(&self) -> Result<(), DiscoveryValidationError> {
        for change in &self.changed {
            change.validate()?;
        }
        for provider in &self.unchanged {
            require_non_empty("unchanged[]", provider)?;
        }
        for failure in &self.failed {
            failure.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefreshProviderModelsScope {
    All,
    OAuth,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshProviderModelsOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<RefreshProviderModelsScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{field} must be a non-empty string")]
pub struct DiscoveryValidationError {
    field: &'static str,
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), DiscoveryValidationError> {
    if value.is_empty() {
        return Err(DiscoveryValidationError { field });
    }
    Ok(())
}

pub type ProviderDiscoveryResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[async_trait]
pub trait ProviderDiscoveryServiceContract: Send + Sync {
    // Original: IProviderDiscoveryService.refreshProviderModels().
    async fn refresh_provider_models(
        &self,
        options: Option<RefreshProviderModelsOptions>,
    ) -> ProviderDiscoveryResult<RefreshProviderModelsResponse>;
}

#[derive(Clone)]
pub struct ProviderDiscoveryServiceHandle(pub Arc<dyn ProviderDiscoveryServiceContract>);

impl Deref for ProviderDiscoveryServiceHandle {
    type Target = dyn ProviderDiscoveryServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

// Original: IProviderDiscoveryService decorator identity.
pub const PROVIDER_DISCOVERY_SERVICE_ID: ServiceIdentifier<ProviderDiscoveryServiceHandle> =
    ServiceIdentifier::new("providerDiscovery");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_schema_required_nonempty_wire_fields() {
        let response = RefreshProviderModelsResponse {
            changed: vec![ProviderRefreshChange {
                provider_id: "kimi".into(),
                provider_name: "Kimi".into(),
                added: 2,
                removed: 1,
            }],
            unchanged: vec!["openai".into()],
            failed: vec![ProviderRefreshFailure {
                provider: "anthropic".into(),
                reason: "unavailable".into(),
            }],
        };
        assert_eq!(response.validate(), Ok(()));
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            serde_json::json!({
                "changed": [{"provider_id": "kimi", "provider_name": "Kimi", "added": 2, "removed": 1}],
                "unchanged": ["openai"],
                "failed": [{"provider": "anthropic", "reason": "unavailable"}]
            })
        );

        let invalid = RefreshProviderModelsResponse {
            unchanged: vec![String::new()],
            ..RefreshProviderModelsResponse::default()
        };
        assert_eq!(
            invalid.validate().unwrap_err().to_string(),
            "unchanged[] must be a non-empty string"
        );
    }

    #[test]
    fn options_preserve_the_lowercase_scope_wire_format() {
        let options = RefreshProviderModelsOptions {
            scope: Some(RefreshProviderModelsScope::OAuth),
            provider_id: Some("kimi".into()),
        };
        assert_eq!(
            serde_json::to_value(options).unwrap(),
            serde_json::json!({"scope": "oauth", "providerId": "kimi"})
        );
        assert_eq!(
            PROVIDER_DISCOVERY_SERVICE_ID.to_string(),
            "providerDiscovery"
        );
    }
}
