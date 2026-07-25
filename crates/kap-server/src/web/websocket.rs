use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::header::{AUTHORIZATION, HOST, ORIGIN, SEC_WEBSOCKET_PROTOCOL, USER_AGENT};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use kimi_code_protocol::{ServerHelloCapabilities, ServerHelloPayload, WS_PROTOCOL_VERSION};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use ulid::Ulid;

use crate::middleware::auth::extract_bearer;
use crate::middleware::origin::is_origin_allowed;
use crate::transport::ws::bearer_protocol::{extract_ws_bearer_token, select_ws_bearer_protocol};
use crate::transport::ws::connection_registry::ConnectionLike;
use crate::transport::ws::v1::protocol::{build_ack, build_server_hello};

use super::core_bridge::{CoreHttpRequest, CoreOperation};
use super::state::AppState;

pub const WS_PATH: &str = "/api/v1/ws";
const DEFAULT_MAX_BUFFER_SIZE: u64 = 1_000;

pub async fn ws_upgrade(
    State(state): State<Arc<AppState>>,
    ConnectInfo(remote_address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let host = header(&headers, HOST);
    let origin = header(&headers, ORIGIN);
    if !is_origin_allowed(origin, host, &state.allowed_origins) {
        return StatusCode::FORBIDDEN.into_response();
    }

    if !state.disable_auth {
        let authorization = header(&headers, AUTHORIZATION);
        let protocol_header = header(&headers, SEC_WEBSOCKET_PROTOCOL);
        let candidate =
            extract_bearer(authorization).or_else(|| extract_ws_bearer_token(protocol_header));
        let valid = match candidate {
            Some(candidate) => state
                .credential_validator
                .is_valid(candidate)
                .await
                .unwrap_or(false),
            None => false,
        };
        if !valid {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    let selected_protocol = header(&headers, SEC_WEBSOCKET_PROTOCOL)
        .and_then(|header| select_ws_bearer_protocol(header.split(',').map(str::trim)))
        .map(str::to_owned);
    let remote_address = Some(remote_address.ip().to_string());
    let user_agent = header(&headers, USER_AGENT).map(str::to_owned);
    let connection = Arc::new(AxumWsConnection::new(remote_address, user_agent));
    let upgrade = match selected_protocol {
        Some(protocol) => ws.protocols([protocol]),
        None => ws,
    };
    upgrade
        .on_upgrade(move |socket| serve_socket(socket, state, connection))
        .into_response()
}

struct AxumWsConnection {
    id: String,
    connected_at: String,
    remote_address: Option<String>,
    user_agent: Option<String>,
    has_client_hello: AtomicBool,
    subscriptions: RwLock<BTreeSet<String>>,
    close_tx: mpsc::UnboundedSender<(u16, String)>,
    close_rx: tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<(u16, String)>>>,
}

impl AxumWsConnection {
    fn new(remote_address: Option<String>, user_agent: Option<String>) -> Self {
        let (close_tx, close_rx) = mpsc::unbounded_channel();
        Self {
            id: format!("conn_{}", Ulid::new()),
            connected_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            remote_address,
            user_agent,
            has_client_hello: AtomicBool::new(false),
            subscriptions: RwLock::new(BTreeSet::new()),
            close_tx,
            close_rx: tokio::sync::Mutex::new(Some(close_rx)),
        }
    }

    fn add_subscriptions(&self, session_ids: &[String]) {
        self.subscriptions
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .extend(session_ids.iter().cloned());
    }

    fn remove_subscriptions(&self, session_ids: &[String]) {
        let mut subscriptions = self
            .subscriptions
            .write()
            .unwrap_or_else(|error| error.into_inner());
        for session_id in session_ids {
            subscriptions.remove(session_id);
        }
    }
}

impl ConnectionLike for AxumWsConnection {
    fn id(&self) -> &str {
        &self.id
    }

    fn connected_at(&self) -> &str {
        &self.connected_at
    }

    fn remote_address(&self) -> Option<&str> {
        self.remote_address.as_deref()
    }

    fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }

    fn has_client_hello(&self) -> bool {
        self.has_client_hello.load(Ordering::SeqCst)
    }

    fn subscription_session_ids(&self) -> Vec<String> {
        self.subscriptions
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    fn close(&self, code: u16, reason: Option<&str>) {
        let _ = self
            .close_tx
            .send((code, reason.unwrap_or_default().to_owned()));
    }
}

async fn serve_socket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    connection: Arc<AxumWsConnection>,
) {
    let Some(mut close_rx) = connection.close_rx.lock().await.take() else {
        return;
    };
    state
        .connection_registry
        .add(Arc::clone(&connection) as Arc<dyn ConnectionLike>);
    let hello = build_server_hello(ServerHelloPayload {
        ws_connection_id: connection.id.clone(),
        protocol_version: WS_PROTOCOL_VERSION,
        heartbeat_ms: None,
        max_event_buffer_size: DEFAULT_MAX_BUFFER_SIZE,
        capabilities: ServerHelloCapabilities {
            event_batching: false,
            compression: false,
        },
    });
    if send_json(&mut socket, &hello).await.is_err() {
        state.connection_registry.remove(&connection.id);
        return;
    }

    loop {
        tokio::select! {
            close = close_rx.recv() => {
                if let Some((code, reason)) = close {
                    let _ = socket.send(Message::Close(Some(CloseFrame {
                        code,
                        reason: reason.into(),
                    }))).await;
                }
                break;
            }
            inbound = socket.recv() => {
                let Some(Ok(message)) = inbound else {
                    break;
                };
                match message {
                    Message::Text(text) => {
                        if !handle_text_frame(&mut socket, &state, &connection, text.as_str()).await {
                            break;
                        }
                    }
                    Message::Binary(bytes) => {
                        if let Ok(text) = std::str::from_utf8(&bytes)
                            && !handle_text_frame(&mut socket, &state, &connection, text).await
                        {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
        }
    }
    state.connection_registry.remove(&connection.id);
}

async fn handle_text_frame(
    socket: &mut WebSocket,
    state: &AppState,
    connection: &AxumWsConnection,
    text: &str,
) -> bool {
    let Ok(frame) = serde_json::from_str::<Value>(text) else {
        return true;
    };
    let Some(frame_type) = frame.get("type").and_then(Value::as_str) else {
        return true;
    };
    let id = frame.get("id").and_then(Value::as_str).unwrap_or_default();
    let payload = frame
        .get("payload")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    match frame_type {
        "client_hello" => {
            if let Some(token) = payload.get("token").and_then(Value::as_str) {
                let valid = state
                    .credential_validator
                    .is_valid(token)
                    .await
                    .unwrap_or(false);
                if !valid {
                    let ack = build_ack(id, 40_112, "unauthorized", json!({}));
                    let _ = send_json(socket, &ack).await;
                    let _ = socket.send(Message::Close(None)).await;
                    return false;
                }
            }
            let subscriptions = string_array(payload.get("subscriptions"));
            if !subscriptions.is_empty() {
                invoke_ws_core(state, CoreOperation::WebSocketEventReplay, &frame).await;
            }
            connection.add_subscriptions(&subscriptions);
            connection.has_client_hello.store(true, Ordering::SeqCst);
            let ack = build_ack(
                id,
                0,
                "success",
                json!({
                    "accepted_subscriptions": subscriptions,
                    "resync_required": [],
                    "cursors": {}
                }),
            );
            let _ = send_json(socket, &ack).await;
        }
        "subscribe" => {
            let session_ids = string_array(payload.get("session_ids"));
            if !session_ids.is_empty() {
                invoke_ws_core(state, CoreOperation::WebSocketEventReplay, &frame).await;
            }
            connection.add_subscriptions(&session_ids);
            let ack = build_ack(
                id,
                0,
                "success",
                json!({
                    "accepted": session_ids,
                    "not_found": [],
                    "resync_required": [],
                    "cursors": {}
                }),
            );
            let _ = send_json(socket, &ack).await;
        }
        "unsubscribe" => {
            let session_ids = string_array(payload.get("session_ids"));
            connection.remove_subscriptions(&session_ids);
            let ack = build_ack(
                id,
                0,
                "success",
                json!({
                    "accepted": [],
                    "not_found": [],
                    "resync_required": []
                }),
            );
            let _ = send_json(socket, &ack).await;
        }
        "watch_fs_add" | "watch_fs_remove" => {
            invoke_ws_core(state, CoreOperation::WebSocketFileWatch, &frame).await;
            let ack = build_ack(id, 0, "success", json!({}));
            let _ = send_json(socket, &ack).await;
        }
        _ => {}
    }
    true
}

async fn invoke_ws_core(state: &AppState, operation: CoreOperation, frame: &Value) {
    let _response = state
        .core_bridge
        .invoke(
            operation,
            CoreHttpRequest {
                method: "WS".to_owned(),
                path: WS_PATH.to_owned(),
                query: None,
                headers: BTreeMap::new(),
                body: serde_json::to_vec(frame).unwrap_or_default(),
            },
        )
        .await;
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

async fn send_json(
    socket: &mut WebSocket,
    value: &impl serde::Serialize,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(value).map_err(axum::Error::new)?;
    socket.send(Message::Text(text.into())).await
}

fn header(headers: &HeaderMap, name: axum::http::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}
