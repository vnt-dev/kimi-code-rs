use serde::{Deserialize, Serialize};

use crate::protocol::time::IsoDateTime;
use crate::protocol::validation::{
    absolute_url, literal_true, non_empty, optional_non_empty, positive_u64,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OAuthFlowStatus {
    Pending,
    Authenticated,
    Denied,
    Expired,
    Cancelled,
}

// Original: rest/oauth.ts, oauthLoginStartRequestSchema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthLoginStartRequest {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthFlowStartPending {
    #[serde(deserialize_with = "non_empty")]
    pub flow_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub provider: String,
    pub status: OAuthFlowStartPendingStatus,
    #[serde(deserialize_with = "absolute_url")]
    pub verification_uri: String,
    #[serde(deserialize_with = "absolute_url")]
    pub verification_uri_complete: String,
    #[serde(deserialize_with = "non_empty")]
    pub user_code: String,
    #[serde(deserialize_with = "positive_u64")]
    pub expires_in: u64,
    #[serde(deserialize_with = "positive_u64")]
    pub interval: u64,
    pub expires_at: IsoDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OAuthFlowStartPendingStatus {
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthFlowStartAuthenticated {
    #[serde(deserialize_with = "non_empty")]
    pub flow_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub provider: String,
    pub status: OAuthFlowStartAuthenticatedStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OAuthFlowStartAuthenticatedStatus {
    Authenticated,
}

// Original: rest/oauth.ts, oauthFlowStartSchema discriminated union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OAuthFlowStart {
    Pending(OAuthFlowStartPending),
    Authenticated(OAuthFlowStartAuthenticated),
}

// Original: rest/oauth.ts, oauthFlowSnapshotSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthFlowSnapshot {
    #[serde(deserialize_with = "non_empty")]
    pub flow_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub provider: String,
    pub status: OAuthFlowStatus,
    #[serde(deserialize_with = "absolute_url")]
    pub verification_uri: String,
    #[serde(deserialize_with = "absolute_url")]
    pub verification_uri_complete: String,
    #[serde(deserialize_with = "non_empty")]
    pub user_code: String,
    #[serde(deserialize_with = "positive_u64")]
    pub expires_in: u64,
    pub expires_at: IsoDateTime,
    #[serde(deserialize_with = "positive_u64")]
    pub interval: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<IsoDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

pub type OAuthLoginQuery = OAuthLoginStartRequest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthLoginCancelResponse {
    pub cancelled: bool,
    pub status: OAuthFlowStatus,
}

pub type OAuthLogoutRequest = OAuthLoginStartRequest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthLogoutResponse {
    #[serde(deserialize_with = "literal_true")]
    pub logged_out: bool,
    #[serde(deserialize_with = "non_empty")]
    pub provider: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_oauth_flow_variants_and_constraints() {
        let pending = serde_json::json!({
            "flow_id": "flow-1",
            "provider": "kimi-code-oauth",
            "status": "pending",
            "verification_uri": "https://example.com/device",
            "verification_uri_complete": "https://example.com/device?code=ABCD",
            "user_code": "ABCD",
            "expires_in": 600,
            "interval": 5,
            "expires_at": "2026-07-22T12:00:00Z"
        });
        assert!(matches!(
            serde_json::from_value::<OAuthFlowStart>(pending),
            Ok(OAuthFlowStart::Pending(OAuthFlowStartPending {
                expires_in: 600,
                ..
            }))
        ));

        let authenticated = serde_json::json!({
            "flow_id": "flow-2",
            "provider": "kimi-code-oauth",
            "status": "authenticated"
        });
        assert!(matches!(
            serde_json::from_value::<OAuthFlowStart>(authenticated),
            Ok(OAuthFlowStart::Authenticated(_))
        ));

        let invalid = serde_json::json!({
            "flow_id": "flow-3",
            "provider": "kimi-code-oauth",
            "status": "pending",
            "verification_uri": "not a url",
            "verification_uri_complete": "https://example.com/device",
            "user_code": "ABCD",
            "expires_in": 0,
            "interval": 5,
            "expires_at": "2026-07-22T12:00:00Z"
        });
        assert!(serde_json::from_value::<OAuthFlowStart>(invalid).is_err());
    }
}
