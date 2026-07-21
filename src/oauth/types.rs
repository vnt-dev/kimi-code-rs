use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct TokenInfo {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: f64,
    pub scope: String,
    pub token_type: String,
    pub expires_in: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenInfoWire {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: f64,
    pub scope: String,
    pub token_type: String,
    pub expires_in: f64,
}

pub fn token_to_wire(token: &TokenInfo) -> TokenInfoWire {
    TokenInfoWire {
        access_token: token.access_token.clone(),
        refresh_token: token.refresh_token.clone(),
        expires_at: token.expires_at,
        scope: token.scope.clone(),
        token_type: token.token_type.clone(),
        expires_in: token.expires_in,
    }
}

pub fn token_from_wire(value: &serde_json::Map<String, Value>) -> TokenInfo {
    TokenInfo {
        access_token: string_or_default(value.get("access_token")),
        refresh_token: string_or_default(value.get("refresh_token")),
        expires_at: number_or_zero(value.get("expires_at")),
        scope: string_or_default(value.get("scope")),
        token_type: string_or_default(value.get("token_type")),
        expires_in: number_or_zero(value.get("expires_in")),
    }
}

fn string_or_default(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn number_or_zero(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(0.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthorization {
    pub user_code: String,
    pub device_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: Option<u64>,
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthFlowConfig {
    pub name: String,
    pub oauth_host: String,
    pub client_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_wire_conversion_preserves_snake_case_and_defaults_partial_values() {
        let token = TokenInfo {
            access_token: "access".to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_at: 1_700_000_000.0,
            scope: "read write".to_owned(),
            token_type: "Bearer".to_owned(),
            expires_in: 3_600.0,
        };
        assert_eq!(
            serde_json::to_value(token_to_wire(&token)).expect("wire"),
            serde_json::json!({
                "access_token": "access",
                "refresh_token": "refresh",
                "expires_at": 1_700_000_000.0,
                "scope": "read write",
                "token_type": "Bearer",
                "expires_in": 3_600.0
            })
        );
        let partial = serde_json::json!({ "access_token": "a", "refresh_token": "r" });
        let restored = token_from_wire(partial.as_object().expect("object"));
        assert_eq!(restored.access_token, "a");
        assert_eq!(restored.refresh_token, "r");
        assert_eq!(restored.expires_at, 0.0);
        assert_eq!(restored.expires_in, 0.0);
        assert_eq!(restored.scope, "");
        assert_eq!(restored.token_type, "");
    }
}
