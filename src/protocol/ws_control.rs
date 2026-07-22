use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};

use super::time::IsoDateTime;
use super::validation::{optional_non_null, positive_u64};

pub const WS_PROTOCOL_VERSION: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCursor {
    pub seq: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub epoch: Option<String>,
}

fn optional_non_empty<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        Err(serde::de::Error::custom("must not be empty"))
    } else {
        Ok(Some(value))
    }
}

pub type CursorsBySession = IndexMap<String, SessionCursor>;

// Original: ws-control.ts, wsEventEnvelopeSchema().
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WsEventEnvelope<T> {
    #[serde(rename = "type")]
    pub event_type: String,
    pub seq: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub epoch: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub volatile: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub offset: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub session_id: Option<String>,
    pub timestamp: IsoDateTime,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WsControlEnvelope<T> {
    #[serde(rename = "type")]
    pub control_type: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub id: Option<String>,
    pub payload: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WsAckType {
    Ack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WsAckEnvelope<T> {
    #[serde(rename = "type")]
    pub ack_type: WsAckType,
    pub id: String,
    pub code: i64,
    pub msg: String,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHelloCapabilities {
    pub event_batching: bool,
    pub compression: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHelloPayload {
    pub ws_connection_id: String,
    #[serde(deserialize_with = "positive_u64")]
    pub protocol_version: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_positive"
    )]
    pub heartbeat_ms: Option<u64>,
    #[serde(deserialize_with = "positive_u64")]
    pub max_event_buffer_size: u64,
    pub capabilities: ServerHelloCapabilities,
}

fn optional_positive<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    positive_u64(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerHelloType {
    ServerHello,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHelloMessage {
    #[serde(rename = "type")]
    pub message_type: ServerHelloType,
    pub timestamp: IsoDateTime,
    pub payload: ServerHelloPayload,
}

pub type AgentFilter = IndexMap<String, Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHelloPayload {
    pub client_id: String,
    pub subscriptions: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub cursors: Option<CursorsBySession>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_agent_filter"
    )]
    pub agent_filter: Option<AgentFilter>,
}

fn optional_agent_filter<'de, D>(deserializer: D) -> Result<Option<AgentFilter>, D::Error>
where
    D: Deserializer<'de>,
{
    let filter = AgentFilter::deserialize(deserializer)?;
    if filter.values().any(Vec::is_empty) {
        Err(serde::de::Error::custom(
            "agent filter values must not be empty",
        ))
    } else {
        Ok(Some(filter))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientHelloType {
    ClientHello,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHelloMessage {
    #[serde(rename = "type")]
    pub message_type: ClientHelloType,
    pub id: String,
    pub payload: ClientHelloPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHelloAckPayload {
    pub accepted_subscriptions: Vec<String>,
    pub resync_required: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub cursors: Option<CursorsBySession>,
}

pub type HelloAckPayload = ClientHelloAckPayload;
pub type ClientHelloAckMessage = WsAckEnvelope<ClientHelloAckPayload>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchFsConfig {
    pub paths: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub recursive: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribePayload {
    pub session_ids: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub cursors: Option<CursorsBySession>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub watch_fs: Option<IndexMap<String, WatchFsConfig>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_agent_filter"
    )]
    pub agent_filter: Option<AgentFilter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscribeType {
    Subscribe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeMessage {
    #[serde(rename = "type")]
    pub message_type: SubscribeType,
    pub id: String,
    pub payload: SubscribePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeAckPayload {
    pub accepted: Vec<String>,
    pub not_found: Vec<String>,
    pub resync_required: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub cursors: Option<CursorsBySession>,
}

pub type SubscribeAckMessage = WsAckEnvelope<SubscribeAckPayload>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribePayload {
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnsubscribeType {
    Unsubscribe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribeMessage {
    #[serde(rename = "type")]
    pub message_type: UnsubscribeType,
    pub id: String,
    pub payload: UnsubscribePayload,
}

pub type UnsubscribeAckPayload = SubscribeAckPayload;
pub type UnsubscribeAckMessage = WsAckEnvelope<UnsubscribeAckPayload>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchFsAddPayload {
    pub session_id: String,
    pub paths: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub recursive: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchFsRemovePayload {
    pub session_id: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchFsAddType {
    WatchFsAdd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchFsRemoveType {
    WatchFsRemove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchFsAddMessage {
    #[serde(rename = "type")]
    pub message_type: WatchFsAddType,
    pub id: String,
    pub payload: WatchFsAddPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchFsRemoveMessage {
    #[serde(rename = "type")]
    pub message_type: WatchFsRemoveType,
    pub id: String,
    pub payload: WatchFsRemovePayload,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchFsAckPayload {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub watched_paths: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub current_count: Option<u64>,
}

pub type WatchFsAckMessage = WsAckEnvelope<WatchFsAckPayload>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbortPayload {
    pub session_id: String,
    pub prompt_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AbortType {
    Abort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbortMessage {
    #[serde(rename = "type")]
    pub message_type: AbortType,
    pub id: String,
    pub payload: AbortPayload,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbortAckPayload {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub aborted: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub at_seq: Option<u64>,
}

pub type AbortAckMessage = WsAckEnvelope<AbortAckPayload>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_hello_subscription_and_generic_envelopes() {
        let hello: ServerHelloMessage = serde_json::from_value(serde_json::json!({
            "type": "server_hello", "timestamp": "2026-06-04T10:30:00Z",
            "payload": {"ws_connection_id": "conn", "protocol_version": 2,
                "max_event_buffer_size": 1000,
                "capabilities": {"event_batching": false, "compression": false}}
        }))
        .unwrap();
        assert_eq!(hello.payload.protocol_version, WS_PROTOCOL_VERSION);

        assert!(
            serde_json::from_value::<ClientHelloMessage>(serde_json::json!({
                "type": "client_hello", "id": "c1", "payload": {
                    "client_id": "client", "subscriptions": [],
                    "agent_filter": {"sess": []}
                }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WsAckEnvelope<serde_json::Value>>(serde_json::json!({
                "type": "not_ack", "id": "c1", "code": 0, "msg": "ok", "payload": {}
            }))
            .is_err()
        );
    }
}
