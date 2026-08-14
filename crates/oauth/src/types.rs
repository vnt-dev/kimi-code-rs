use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct TokenInfo {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub scope: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenInfoWire {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(deserialize_with = "lenient_int::i64")]
    pub expires_at: i64,
    pub scope: String,
    pub token_type: String,
    #[serde(deserialize_with = "lenient_int::u64")]
    pub expires_in: u64,
}

// Accepts both `100` and the legacy `100.0` float spelling written by older
// versions (truncated), since these values are persisted on disk.
mod lenient_int {
    use serde::{Deserialize, Deserializer, de::Error};

    pub fn i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
    where
        D: Deserializer<'de>,
    {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::Number(number) => number
                .as_i64()
                .or_else(|| number.as_f64().map(|value| value.trunc() as i64))
                .ok_or_else(|| D::Error::custom("expected an integer")),
            _ => Err(D::Error::custom("expected an integer")),
        }
    }

    pub fn u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::Number(number) => number
                .as_u64()
                .or_else(|| {
                    number
                        .as_f64()
                        .filter(|value| *value >= 0.0)
                        .map(|value| value.trunc() as u64)
                })
                .ok_or_else(|| D::Error::custom("expected a non-negative integer")),
            _ => Err(D::Error::custom("expected a non-negative integer")),
        }
    }
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
        expires_in: number_or_zero(value.get("expires_in")).max(0) as u64,
    }
}

fn string_or_default(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn number_or_zero(value: Option<&Value>) -> i64 {
    value
        .and_then(Value::as_f64)
        .map(|number| number.trunc() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq)]
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
            expires_at: 1_700_000_000,
            scope: "read write".to_owned(),
            token_type: "Bearer".to_owned(),
            expires_in: 3_600,
        };
        assert_eq!(
            serde_json::to_value(token_to_wire(&token)).expect("wire"),
            serde_json::json!({
                "access_token": "access",
                "refresh_token": "refresh",
                "expires_at": 1_700_000_000,
                "scope": "read write",
                "token_type": "Bearer",
                "expires_in": 3_600
            })
        );
        let partial = serde_json::json!({ "access_token": "a", "refresh_token": "r" });
        let restored = token_from_wire(partial.as_object().expect("object"));
        assert_eq!(restored.access_token, "a");
        assert_eq!(restored.refresh_token, "r");
        assert_eq!(restored.expires_at, 0);
        assert_eq!(restored.expires_in, 0);
        assert_eq!(restored.scope, "");
        assert_eq!(restored.token_type, "");
    }

    #[test]
    fn wire_deserialization_accepts_legacy_float_spelling() {
        let wire: TokenInfoWire = serde_json::from_value(serde_json::json!({
            "access_token": "a",
            "refresh_token": "r",
            "expires_at": 1_700_000_000.0,
            "scope": "",
            "token_type": "Bearer",
            "expires_in": 3_600.0
        }))
        .expect("legacy float wire");
        assert_eq!(wire.expires_at, 1_700_000_000);
        assert_eq!(wire.expires_in, 3_600);

        let legacy = serde_json::json!({
            "access_token": "a",
            "refresh_token": "r",
            "expires_at": 1_700_000_000.0,
            "expires_in": 3_600.0
        });
        let restored = token_from_wire(legacy.as_object().expect("object"));
        assert_eq!(restored.expires_at, 1_700_000_000);
        assert_eq!(restored.expires_in, 3_600);
    }
}
