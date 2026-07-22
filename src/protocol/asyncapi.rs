use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use super::ws_control::{
    WS_OPERATIONS, WsMessageSchema, WsOperationDefinition, WsOperationDirection,
};

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

fn build_messages() -> Map<String, Value> {
    let mut messages = Map::new();
    for operation in WS_OPERATIONS {
        let id = message_id(operation.operation_type);
        messages.insert(
            id.clone(),
            async_api_message(
                operation.operation_type,
                operation.description,
                operation.message_schema,
            ),
        );
        if let Some(ack_schema) = operation.ack_schema {
            messages.insert(
                format!("{id}_ack"),
                async_api_message(
                    &format!("{}.ack", operation.operation_type),
                    &format!("Acknowledgement for {}.", operation.operation_type),
                    ack_schema,
                ),
            );
        }
    }
    messages
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

fn async_api_message(name: &str, summary: &str, schema: WsMessageSchema) -> Value {
    json!({
        "name": name,
        "title": title_from_name(name),
        "summary": summary,
        "contentType": "application/json",
        "payload": json_schema(schema)
    })
}

fn object_schema(required: &[&str], properties: IndexMap<&str, Value>) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn string_schema() -> Value {
    json!({"type": "string"})
}

fn payload_object_schema() -> Value {
    // MIGRATION-TODO:
    // Original: packages/protocol/src/asyncapi.ts, jsonSchema().
    // Rust adaptation: operation roots and references are complete, but nested
    // payload properties currently remain open objects because Rust has no Zod
    // runtime schema graph. Completion condition: derive or hand-author the
    // nested JSON Schema graph from the migrated serde types.
    json!({"type": "object"})
}

fn json_schema(schema: WsMessageSchema) -> Value {
    use WsMessageSchema as S;

    let control = |wire_type: &str| {
        object_schema(
            &["type", "id", "payload"],
            IndexMap::from([
                ("type", json!({"type": "string", "const": wire_type})),
                ("id", string_schema()),
                ("payload", payload_object_schema()),
            ]),
        )
    };
    let ack = || {
        object_schema(
            &["type", "id", "code", "msg", "payload"],
            IndexMap::from([
                ("type", json!({"type": "string", "const": "ack"})),
                ("id", string_schema()),
                ("code", json!({"type": "integer"})),
                ("msg", string_schema()),
                ("payload", payload_object_schema()),
            ]),
        )
    };

    match schema {
        S::ClientHello => control("client_hello"),
        S::Subscribe => control("subscribe"),
        S::Unsubscribe => control("unsubscribe"),
        S::WatchFsAdd => control("watch_fs_add"),
        S::WatchFsRemove => control("watch_fs_remove"),
        S::Abort => control("abort"),
        S::TerminalAttach => control("terminal_attach"),
        S::TerminalDetach => control("terminal_detach"),
        S::TerminalInput => control("terminal_input"),
        S::TerminalResize => control("terminal_resize"),
        S::TerminalClose => control("terminal_close"),
        S::Pong => object_schema(
            &["type", "payload"],
            IndexMap::from([
                ("type", json!({"type": "string", "const": "pong"})),
                ("payload", payload_object_schema()),
            ]),
        ),
        S::ClientHelloAck
        | S::SubscribeAck
        | S::UnsubscribeAck
        | S::WatchFsAck
        | S::AbortAck
        | S::TerminalAttachAck
        | S::TerminalDetachAck
        | S::TerminalInputAck
        | S::TerminalResizeAck
        | S::TerminalCloseAck => ack(),
        S::ServerHello => system_schema("server_hello", true),
        S::Ping => system_schema("ping", true),
        S::ResyncRequired => system_schema("resync_required", true),
        S::Error => system_schema("error", true),
        S::SessionEvent => object_schema(
            &["type", "seq", "timestamp", "payload"],
            IndexMap::from([
                ("type", string_schema()),
                ("seq", json!({"type": "integer", "minimum": 0})),
                ("timestamp", string_schema()),
                ("payload", payload_object_schema()),
            ]),
        ),
    }
}

fn system_schema(wire_type: &str, timestamp: bool) -> Value {
    let mut properties = IndexMap::from([
        ("type", json!({"type": "string", "const": wire_type})),
        ("payload", payload_object_schema()),
    ]);
    let required = if timestamp {
        properties.insert("timestamp", string_schema());
        vec!["type", "timestamp", "payload"]
    } else {
        vec!["type", "payload"]
    };
    object_schema(&required, properties)
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

fn title_from_name(name: &str) -> String {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + characters.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    }
}
