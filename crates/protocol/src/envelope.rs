use serde::{Deserialize, Serialize};

use super::display::OptionalJsonValue;
use super::validation::{optional_non_null, required_nullable};

// Original: packages/protocol/src/envelope.ts, Envelope<T>
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct Envelope<T> {
    pub code: i64,
    pub msg: String,
    #[serde(deserialize_with = "required_nullable")]
    pub data: Option<T>,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub details: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub stack: Option<String>,
}

// Original: envelope.ts, okEnvelope()
pub fn ok_envelope<T>(data: T, request_id: impl Into<String>) -> Envelope<T> {
    Envelope {
        code: 0,
        msg: "success".to_owned(),
        data: Some(data),
        request_id: request_id.into(),
        details: OptionalJsonValue::Absent,
        stack: None,
    }
}

// Original: envelope.ts, errEnvelope()
pub fn err_envelope(
    code: impl Into<i64>,
    msg: impl Into<String>,
    request_id: impl Into<String>,
    stack: Option<String>,
) -> Envelope<()> {
    Envelope {
        code: code.into(),
        msg: msg.into(),
        data: None,
        request_id: request_id.into(),
        details: OptionalJsonValue::Absent,
        stack,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_required_nullable_data_and_optional_unknown_details() {
        assert!(
            serde_json::from_value::<Envelope<serde_json::Value>>(serde_json::json!({
                "code": 0, "msg": "success", "request_id": "req"
            }))
            .is_err()
        );
        let envelope: Envelope<serde_json::Value> = serde_json::from_value(serde_json::json!({
            "code": 1, "msg": "failed", "data": null, "request_id": "req",
            "details": null
        }))
        .unwrap();
        assert_eq!(envelope.details.as_value(), Some(&serde_json::Value::Null));
        assert_eq!(
            serde_json::to_string(&err_envelope(1, "failed", "req", None)).unwrap(),
            r#"{"code":1,"msg":"failed","data":null,"request_id":"req"}"#
        );
    }
}
