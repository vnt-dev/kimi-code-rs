use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};

use super::display::OptionalJsonValue;
use super::events::Event;
use super::time::IsoDateTime;
use super::validation::{
    OptionalNullable, literal_true, non_empty, optional_non_null, positive_u64,
};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAttachPayload {
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub terminal_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub since_seq: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAttachType {
    TerminalAttach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAttachMessage {
    #[serde(rename = "type")]
    pub message_type: TerminalAttachType,
    pub id: String,
    pub payload: TerminalAttachPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAttachAckPayload {
    #[serde(deserialize_with = "literal_true")]
    pub attached: bool,
    pub replayed: u64,
}

pub type TerminalAttachAckMessage = WsAckEnvelope<TerminalAttachAckPayload>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalTargetPayload {
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub terminal_id: String,
}

pub type TerminalDetachPayload = TerminalTargetPayload;
pub type TerminalClosePayload = TerminalTargetPayload;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDetachType {
    TerminalDetach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDetachMessage {
    #[serde(rename = "type")]
    pub message_type: TerminalDetachType,
    pub id: String,
    pub payload: TerminalDetachPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDetachAckPayload {
    #[serde(deserialize_with = "literal_true")]
    pub detached: bool,
}

pub type TerminalDetachAckMessage = WsAckEnvelope<TerminalDetachAckPayload>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputPayload {
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub terminal_id: String,
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalInputType {
    TerminalInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputMessage {
    #[serde(rename = "type")]
    pub message_type: TerminalInputType,
    pub id: String,
    pub payload: TerminalInputPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputAckPayload {
    #[serde(deserialize_with = "literal_true")]
    pub accepted: bool,
}

pub type TerminalInputAckMessage = WsAckEnvelope<TerminalInputAckPayload>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResizePayload {
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub terminal_id: String,
    #[serde(deserialize_with = "positive_u64")]
    pub cols: u64,
    #[serde(deserialize_with = "positive_u64")]
    pub rows: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalResizeType {
    TerminalResize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResizeMessage {
    #[serde(rename = "type")]
    pub message_type: TerminalResizeType,
    pub id: String,
    pub payload: TerminalResizePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResizeAckPayload {
    #[serde(deserialize_with = "literal_true")]
    pub resized: bool,
}

pub type TerminalResizeAckMessage = WsAckEnvelope<TerminalResizeAckPayload>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalCloseType {
    TerminalClose,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCloseMessage {
    #[serde(rename = "type")]
    pub message_type: TerminalCloseType,
    pub id: String,
    pub payload: TerminalClosePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCloseAckPayload {
    #[serde(deserialize_with = "literal_true")]
    pub closed: bool,
}

pub type TerminalCloseAckMessage = WsAckEnvelope<TerminalCloseAckPayload>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingPayload {
    pub nonce: String,
}

pub type PongPayload = PingPayload;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PingType {
    Ping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingMessage {
    #[serde(rename = "type")]
    pub message_type: PingType,
    pub timestamp: IsoDateTime,
    pub payload: PingPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PongType {
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PongMessage {
    #[serde(rename = "type")]
    pub message_type: PongType,
    pub payload: PongPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncRequiredReason {
    BufferOverflow,
    SessionRecreated,
    EpochChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncRequiredPayload {
    pub session_id: String,
    pub reason: ResyncRequiredReason,
    pub current_seq: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub epoch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncRequiredType {
    ResyncRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncRequiredMessage {
    #[serde(rename = "type")]
    pub message_type: ResyncRequiredType,
    pub timestamp: IsoDateTime,
    pub payload: ResyncRequiredPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WsErrorPayload {
    pub code: i64,
    pub msg: String,
    pub fatal: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub details: OptionalJsonValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WsErrorType {
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WsErrorMessage {
    #[serde(rename = "type")]
    pub message_type: WsErrorType,
    pub timestamp: IsoDateTime,
    pub payload: WsErrorPayload,
}

pub type SessionEventMessage = WsEventEnvelope<Event>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOutputPayload {
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutputType {
    TerminalOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOutputMessage {
    #[serde(rename = "type")]
    pub message_type: TerminalOutputType,
    #[serde(deserialize_with = "positive_u64")]
    pub seq: u64,
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub terminal_id: String,
    pub timestamp: IsoDateTime,
    pub payload: TerminalOutputPayload,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalExitPayload {
    #[serde(default, skip_serializing_if = "OptionalNullable::is_absent")]
    pub exit_code: OptionalNullable<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalExitType {
    TerminalExit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalExitMessage {
    #[serde(rename = "type")]
    pub message_type: TerminalExitType,
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub terminal_id: String,
    pub timestamp: IsoDateTime,
    pub payload: TerminalExitPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClientControlMessage {
    ClientHello(ClientHelloMessage),
    Subscribe(SubscribeMessage),
    Unsubscribe(UnsubscribeMessage),
    WatchFsAdd(WatchFsAddMessage),
    WatchFsRemove(WatchFsRemoveMessage),
    Abort(AbortMessage),
    TerminalAttach(TerminalAttachMessage),
    TerminalDetach(TerminalDetachMessage),
    TerminalInput(TerminalInputMessage),
    TerminalResize(TerminalResizeMessage),
    TerminalClose(TerminalCloseMessage),
    Pong(PongMessage),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerSystemMessage {
    ServerHello(ServerHelloMessage),
    Ping(PingMessage),
    ResyncRequired(ResyncRequiredMessage),
    Error(WsErrorMessage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsOperationDirection {
    ClientToServer,
    ServerToClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WsOperationKind {
    Control,
    System,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WsMessageSchema {
    ClientHello,
    ClientHelloAck,
    Subscribe,
    SubscribeAck,
    Unsubscribe,
    UnsubscribeAck,
    WatchFsAdd,
    WatchFsRemove,
    WatchFsAck,
    Abort,
    AbortAck,
    TerminalAttach,
    TerminalAttachAck,
    TerminalDetach,
    TerminalDetachAck,
    TerminalInput,
    TerminalInputAck,
    TerminalResize,
    TerminalResizeAck,
    TerminalClose,
    TerminalCloseAck,
    Pong,
    ServerHello,
    Ping,
    ResyncRequired,
    Error,
    SessionEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WsOperationDefinition {
    pub operation_type: &'static str,
    pub direction: WsOperationDirection,
    pub kind: WsOperationKind,
    pub message_schema: WsMessageSchema,
    pub ack_schema: Option<WsMessageSchema>,
    pub description: &'static str,
}

const fn client_operation(
    operation_type: &'static str,
    message_schema: WsMessageSchema,
    ack_schema: Option<WsMessageSchema>,
    description: &'static str,
) -> WsOperationDefinition {
    WsOperationDefinition {
        operation_type,
        direction: WsOperationDirection::ClientToServer,
        kind: WsOperationKind::Control,
        message_schema,
        ack_schema,
        description,
    }
}

pub const CLIENT_CONTROL_OPERATIONS: [WsOperationDefinition; 12] = [
    client_operation(
        "client_hello",
        WsMessageSchema::ClientHello,
        Some(WsMessageSchema::ClientHelloAck),
        "Start a client session and optionally subscribe to existing daemon sessions.",
    ),
    client_operation(
        "subscribe",
        WsMessageSchema::Subscribe,
        Some(WsMessageSchema::SubscribeAck),
        "Subscribe the connection to one or more session event streams.",
    ),
    client_operation(
        "unsubscribe",
        WsMessageSchema::Unsubscribe,
        Some(WsMessageSchema::UnsubscribeAck),
        "Remove one or more session event stream subscriptions.",
    ),
    client_operation(
        "watch_fs_add",
        WsMessageSchema::WatchFsAdd,
        Some(WsMessageSchema::WatchFsAck),
        "Add filesystem watch paths for a subscribed session.",
    ),
    client_operation(
        "watch_fs_remove",
        WsMessageSchema::WatchFsRemove,
        Some(WsMessageSchema::WatchFsAck),
        "Remove filesystem watch paths for a subscribed session.",
    ),
    client_operation(
        "abort",
        WsMessageSchema::Abort,
        Some(WsMessageSchema::AbortAck),
        "Abort a running prompt in a session.",
    ),
    client_operation(
        "terminal_attach",
        WsMessageSchema::TerminalAttach,
        Some(WsMessageSchema::TerminalAttachAck),
        "Attach this connection to a terminal stream.",
    ),
    client_operation(
        "terminal_detach",
        WsMessageSchema::TerminalDetach,
        Some(WsMessageSchema::TerminalDetachAck),
        "Detach this connection from a terminal stream.",
    ),
    client_operation(
        "terminal_input",
        WsMessageSchema::TerminalInput,
        Some(WsMessageSchema::TerminalInputAck),
        "Write raw input bytes to a terminal.",
    ),
    client_operation(
        "terminal_resize",
        WsMessageSchema::TerminalResize,
        Some(WsMessageSchema::TerminalResizeAck),
        "Resize a terminal.",
    ),
    client_operation(
        "terminal_close",
        WsMessageSchema::TerminalClose,
        Some(WsMessageSchema::TerminalCloseAck),
        "Close a terminal.",
    ),
    client_operation(
        "pong",
        WsMessageSchema::Pong,
        None,
        "Reply to a server ping with the same nonce.",
    ),
];

const fn server_operation(
    operation_type: &'static str,
    message_schema: WsMessageSchema,
    description: &'static str,
) -> WsOperationDefinition {
    WsOperationDefinition {
        operation_type,
        direction: WsOperationDirection::ServerToClient,
        kind: WsOperationKind::System,
        message_schema,
        ack_schema: None,
        description,
    }
}

pub const SERVER_SYSTEM_OPERATIONS: [WsOperationDefinition; 4] = [
    server_operation(
        "server_hello",
        WsMessageSchema::ServerHello,
        "Initial server greeting sent immediately after the socket opens.",
    ),
    server_operation(
        "ping",
        WsMessageSchema::Ping,
        "Heartbeat ping sent by the server; clients must answer with pong.",
    ),
    server_operation(
        "resync_required",
        WsMessageSchema::ResyncRequired,
        "Signals that a client must rebuild local session state from REST history.",
    ),
    server_operation(
        "error",
        WsMessageSchema::Error,
        "Server-side WebSocket protocol or runtime error.",
    ),
];

pub const SESSION_EVENT_OPERATION: WsOperationDefinition = WsOperationDefinition {
    operation_type: "session_event",
    direction: WsOperationDirection::ServerToClient,
    kind: WsOperationKind::Event,
    message_schema: WsMessageSchema::SessionEvent,
    ack_schema: None,
    description: "Session-scoped agent event envelope; frame type is the payload event type.",
};

pub const WS_OPERATIONS: [WsOperationDefinition; 17] = [
    CLIENT_CONTROL_OPERATIONS[0],
    CLIENT_CONTROL_OPERATIONS[1],
    CLIENT_CONTROL_OPERATIONS[2],
    CLIENT_CONTROL_OPERATIONS[3],
    CLIENT_CONTROL_OPERATIONS[4],
    CLIENT_CONTROL_OPERATIONS[5],
    CLIENT_CONTROL_OPERATIONS[6],
    CLIENT_CONTROL_OPERATIONS[7],
    CLIENT_CONTROL_OPERATIONS[8],
    CLIENT_CONTROL_OPERATIONS[9],
    CLIENT_CONTROL_OPERATIONS[10],
    CLIENT_CONTROL_OPERATIONS[11],
    SERVER_SYSTEM_OPERATIONS[0],
    SERVER_SYSTEM_OPERATIONS[1],
    SERVER_SYSTEM_OPERATIONS[2],
    SERVER_SYSTEM_OPERATIONS[3],
    SESSION_EVENT_OPERATION,
];

// Original: ws-control.ts, getClientControlOperation().
pub fn get_client_control_operation(
    operation_type: &str,
) -> Option<&'static WsOperationDefinition> {
    CLIENT_CONTROL_OPERATIONS
        .iter()
        .find(|operation| operation.operation_type == operation_type)
}

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
        let resize: ClientControlMessage = serde_json::from_value(serde_json::json!({
            "type": "terminal_resize", "id": "c2", "payload": {
                "session_id": "sess", "terminal_id": "term", "cols": 120, "rows": 32
            }
        }))
        .unwrap();
        assert!(matches!(resize, ClientControlMessage::TerminalResize(_)));
        assert!(
            serde_json::from_value::<TerminalAttachAckMessage>(serde_json::json!({
                "type": "ack", "id": "c", "code": 0, "msg": "ok",
                "payload": {"attached": false, "replayed": 0}
            }))
            .is_err()
        );
        let exit: TerminalExitMessage = serde_json::from_value(serde_json::json!({
            "type": "terminal_exit", "session_id": "sess", "terminal_id": "term",
            "timestamp": "2026-06-04T10:30:00Z", "payload": {"exit_code": null}
        }))
        .unwrap();
        assert_eq!(exit.payload.exit_code, OptionalNullable::Null);
        let subscribe = get_client_control_operation("subscribe").unwrap();
        assert_eq!(subscribe.ack_schema, Some(WsMessageSchema::SubscribeAck));
        assert_eq!(WS_OPERATIONS.len(), 17);
        assert!(get_client_control_operation("ping").is_none());
    }
}
