use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcRequest {
    pub id: String,
    pub command: String,
    #[serde(default = "empty_args")]
    pub args: Value,
}

fn empty_args() -> Value {
    Value::Object(Default::default())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponseError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponse {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcResponseError>,
}

impl RpcResponse {
    pub fn success(id: impl Into<String>, result: Value) -> Self {
        Self {
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(RpcResponseError {
                code: code.into(),
                message: message.into(),
                details,
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ServerFrame {
    Ready {
        #[serde(rename = "connectionId")]
        connection_id: String,
    },
    Event {
        event: String,
        payload: Value,
    },
}
