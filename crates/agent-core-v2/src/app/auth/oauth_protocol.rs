//! Validated v1 OAuth wire DTOs.
//!
//! Original: `packages/agent-core-v2/src/app/auth/oauthProtocol.ts`.

use std::{fmt, num::NonZeroU64};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use url::Url;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    pub fn new(value: impl Into<String>) -> Result<Self, NonEmptyStringError> {
        let value = value.into();
        if value.is_empty() {
            Err(NonEmptyStringError)
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for NonEmptyString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("string must not be empty")]
pub struct NonEmptyStringError;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OAuthFlowStatus {
    Pending,
    Authenticated,
    Denied,
    Expired,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthFlowStartPending {
    pub flow_id: NonEmptyString,
    pub provider: NonEmptyString,
    pub verification_uri: Url,
    pub verification_uri_complete: Url,
    pub user_code: NonEmptyString,
    pub expires_in: NonZeroU64,
    pub interval: NonZeroU64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthFlowStartAuthenticated {
    pub flow_id: NonEmptyString,
    pub provider: NonEmptyString,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum OAuthFlowStart {
    Pending(Box<OAuthFlowStartPending>),
    Authenticated(OAuthFlowStartAuthenticated),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthFlowSnapshot {
    pub flow_id: NonEmptyString,
    pub provider: NonEmptyString,
    pub status: OAuthFlowStatus,
    pub verification_uri: Url,
    pub verification_uri_complete: Url,
    pub user_code: NonEmptyString,
    pub expires_in: NonZeroU64,
    pub expires_at: DateTime<Utc>,
    pub interval: NonZeroU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthLoginCancelResponse {
    pub cancelled: bool,
    pub status: OAuthFlowStatus,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AlwaysTrue;

impl Serialize for AlwaysTrue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for AlwaysTrue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(de::Error::custom("expected true"))
        }
    }
}

impl fmt::Display for NonEmptyString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthLogoutResponse {
    pub logged_out: AlwaysTrue,
    pub provider: NonEmptyString,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderRefreshChange {
    pub provider_id: NonEmptyString,
    pub provider_name: NonEmptyString,
    pub added: u64,
    pub removed: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderRefreshFailure {
    pub provider: NonEmptyString,
    pub reason: NonEmptyString,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RefreshOAuthProviderModelsResponse {
    pub changed: Vec<ProviderRefreshChange>,
    pub unchanged: Vec<NonEmptyString>,
    pub failed: Vec<ProviderRefreshFailure>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn pending_and_authenticated_start_shapes_round_trip_by_status() {
        let pending = json!({
            "flow_id": "flow",
            "provider": "kimi",
            "status": "pending",
            "verification_uri": "https://example.test/device",
            "verification_uri_complete": "https://example.test/device?code=ABCD",
            "user_code": "ABCD",
            "expires_in": 600,
            "interval": 5,
            "expires_at": "2026-07-23T10:00:00Z"
        });
        let parsed: OAuthFlowStart = serde_json::from_value(pending.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), pending);

        let authenticated = json!({
            "flow_id": "flow",
            "provider": "kimi",
            "status": "authenticated"
        });
        let parsed: OAuthFlowStart = serde_json::from_value(authenticated.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), authenticated);
    }

    #[test]
    fn rejects_the_same_boundary_values_as_the_zod_schemas() {
        let invalid = json!({
            "flow_id": "",
            "provider": "kimi",
            "status": "pending",
            "verification_uri": "not a url",
            "verification_uri_complete": "https://example.test",
            "user_code": "",
            "expires_in": 0,
            "interval": 0,
            "expires_at": "invalid"
        });
        assert!(serde_json::from_value::<OAuthFlowStart>(invalid).is_err());
        assert!(
            serde_json::from_value::<OAuthLogoutResponse>(json!({
                "logged_out": false,
                "provider": "kimi"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<RefreshOAuthProviderModelsResponse>(json!({
                "changed": [], "unchanged": [""], "failed": []
            }))
            .is_err()
        );
    }
}
