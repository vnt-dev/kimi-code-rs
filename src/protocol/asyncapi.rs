use serde_json::{Map, Value, json};

use super::ws_control::{WS_OPERATIONS, WsOperationDefinition, WsOperationDirection};

const ASYNCAPI_VERSION: &str = "3.1.0";
const DEFAULT_TITLE: &str = "Kimi Code WebSocket API";
const DEFAULT_VERSION: &str = "0.1.0";
const DEFAULT_SERVER_HOST: &str = "localhost";
const DEFAULT_WS_PATH: &str = "/api/v1/ws";
const CHANNEL_ID: &str = "kimiCodeWebSocket";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ServerProtocol {
    #[default]
    Ws,
    Wss,
}

impl ServerProtocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ws => "ws",
            Self::Wss => "wss",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AsyncApiDocumentOptions {
    pub title: Option<String>,
    pub version: Option<String>,
    pub server_host: Option<String>,
    pub server_protocol: Option<ServerProtocol>,
    pub ws_path: Option<String>,
}

// Original: asyncapi.ts, createAsyncApiDocument().
pub fn create_async_api_document(options: AsyncApiDocumentOptions) -> Value {
    let title = options.title.as_deref().unwrap_or(DEFAULT_TITLE);
    let version = options.version.as_deref().unwrap_or(DEFAULT_VERSION);
    let server_host = options
        .server_host
        .as_deref()
        .unwrap_or(DEFAULT_SERVER_HOST);
    let server_protocol = options.server_protocol.unwrap_or_default().as_str();
    let ws_path = options.ws_path.as_deref().unwrap_or(DEFAULT_WS_PATH);
    let messages = build_messages();
    let channel_messages = messages
        .keys()
        .map(|id| {
            (
                id.clone(),
                json!({"$ref": format!("#/components/messages/{id}")}),
            )
        })
        .collect::<Map<String, Value>>();

    json!({
        "asyncapi": ASYNCAPI_VERSION,
        "info": {
            "title": title,
            "version": version,
            "description": "WebSocket protocol for Kimi Code daemon control frames, acknowledgements, system frames, and session event streaming."
        },
        "defaultContentType": "application/json",
        "servers": {
            "local": {
                "host": server_host,
                "protocol": server_protocol,
                "pathname": ws_path,
                "description": "Kimi Code daemon WebSocket endpoint."
            }
        },
        "channels": {
            (CHANNEL_ID): {
                "address": ws_path,
                "servers": [{"$ref": "#/servers/local"}],
                "messages": channel_messages
            }
        },
        "operations": {
            "receiveClientMessages": {
                "action": "receive",
                "channel": {"$ref": format!("#/channels/{CHANNEL_ID}")},
                "messages": operation_message_refs(WsOperationDirection::ClientToServer)
            },
            "sendServerMessages": {
                "action": "send",
                "channel": {"$ref": format!("#/channels/{CHANNEL_ID}")},
                "messages": operation_message_refs(WsOperationDirection::ServerToClient)
                    .into_iter()
                    .chain(ack_message_refs())
                    .collect::<Vec<_>>()
            }
        },
        "components": {"messages": messages}
    })
}

// Generated from the original `createAsyncApiDocument()` using zod 4.3.6.
// The operation registry remains native Rust; this asset preserves Zod's
// complete draft-7 payload schemas, including nested unions and constraints.
const ASYNCAPI_MESSAGES_JSON: &str = include_str!("asyncapi_messages.json");

fn build_messages() -> Map<String, Value> {
    serde_json::from_str(ASYNCAPI_MESSAGES_JSON)
        .expect("generated AsyncAPI message schemas must be valid JSON")
}

fn operation_message_refs(direction: WsOperationDirection) -> Vec<Value> {
    WS_OPERATIONS
        .iter()
        .filter(|operation| operation.direction == direction)
        .map(operation_ref)
        .collect()
}

fn operation_ref(operation: &WsOperationDefinition) -> Value {
    json!({"$ref": format!(
        "#/components/messages/{}",
        message_id(operation.operation_type)
    )})
}

fn ack_message_refs() -> impl Iterator<Item = Value> {
    WS_OPERATIONS
        .iter()
        .filter(|operation| operation.ack_schema.is_some())
        .map(|operation| {
            json!({"$ref": format!(
                "#/components/messages/{}_ack",
                message_id(operation.operation_type)
            )})
        })
}

fn message_id(message_type: &str) -> String {
    let mut id = String::new();
    let mut last_was_separator = true;
    for character in message_type.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character);
            last_was_separator = false;
        } else if !last_was_separator {
            id.push('_');
            last_was_separator = true;
        }
    }
    while id.ends_with('_') {
        id.pop();
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_asyncapi_document_from_operation_registry() {
        let document = create_async_api_document(AsyncApiDocumentOptions {
            version: Some("1.2.3".into()),
            server_host: Some("127.0.0.1:14567".into()),
            ws_path: Some("/api/v1/ws".into()),
            ..Default::default()
        });
        assert_eq!(document["asyncapi"], "3.1.0");
        assert_eq!(document["info"]["version"], "1.2.3");
        assert_eq!(document["servers"]["local"]["host"], "127.0.0.1:14567");
        assert_eq!(
            document["channels"][CHANNEL_ID]["messages"]["subscribe_ack"]["$ref"],
            "#/components/messages/subscribe_ack"
        );
        assert_eq!(
            document["components"]["messages"]["client_hello"]["payload"]["type"],
            "object"
        );
        assert_eq!(
            document["components"]["messages"]["terminal_resize"]["payload"]["properties"]["payload"]
                ["properties"]["cols"]["exclusiveMinimum"],
            0
        );
        assert!(
            document["components"]["messages"]["session_event"]["payload"]["properties"]["payload"]
                ["allOf"][0]["oneOf"]
                .as_array()
                .is_some_and(|variants| variants.len() > 40)
        );
    }
}
