use serde::{Deserialize, Serialize};
use serde_json::Value;

// Original: packages/protocol/src/envelope.ts, Envelope<T>
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub code: i64,
    pub msg: String,
    pub data: Option<T>,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

// Original: envelope.ts, okEnvelope()
pub fn ok_envelope<T>(data: T, request_id: impl Into<String>) -> Envelope<T> {
    Envelope {
        code: 0,
        msg: "success".to_owned(),
        data: Some(data),
        request_id: request_id.into(),
        details: None,
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
        details: None,
        stack,
    }
}
