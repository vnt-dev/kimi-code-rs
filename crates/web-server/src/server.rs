use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Multipart, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header, uri::Authority},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use kimi_code_agent_core_v2::{
    _base::di::lifecycle::DisposableHandle, app::desktop_client::KimiCodeDesktopClient,
};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    RpcError, RpcRequest, RpcResponse, app_events::ApplicationEventBus, rpc::dispatch_rpc,
    settings::WebServerListenScope, wire::ServerFrame,
};

const WS_BEARER_PROTOCOL_PREFIX: &str = "kimi-code.bearer.";
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct WebAsset {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub csp_header: Option<String>,
}

pub trait AssetProvider: Send + Sync + 'static {
    fn get(&self, path: &str) -> Option<WebAsset>;
}

impl<F> AssetProvider for F
where
    F: Fn(&str) -> Option<WebAsset> + Send + Sync + 'static,
{
    fn get(&self, path: &str) -> Option<WebAsset> {
        self(path)
    }
}

pub(crate) struct RpcConnection {
    sender: mpsc::UnboundedSender<String>,
    subscriptions: Mutex<HashMap<String, DisposableHandle>>,
}

impl RpcConnection {
    fn new(sender: mpsc::UnboundedSender<String>) -> Self {
        Self {
            sender,
            subscriptions: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn emit(&self, event: &str, payload: Value) {
        let frame = ServerFrame::Event {
            event: event.to_owned(),
            payload,
        };
        if let Ok(serialized) = serde_json::to_string(&frame) {
            let _ = self.sender.send(serialized);
        }
    }

    pub(crate) fn add_subscription(
        &self,
        subscription: DisposableHandle,
    ) -> Result<String, RpcError> {
        let id = Uuid::new_v4().to_string();
        self.subscriptions
            .lock()
            .map_err(|_| RpcError::transport("agent subscription registry is unavailable"))?
            .insert(id.clone(), subscription);
        Ok(id)
    }

    pub(crate) fn remove_subscription(&self, id: &str) -> Result<(), RpcError> {
        let subscription = self
            .subscriptions
            .lock()
            .map_err(|_| RpcError::transport("agent subscription registry is unavailable"))?
            .remove(id);
        if let Some(subscription) = subscription {
            subscription
                .dispose()
                .map_err(|error| RpcError::transport(error.to_string()))?;
        }
        Ok(())
    }

    fn dispose_all(&self) {
        let subscriptions = self
            .subscriptions
            .lock()
            .map(|mut subscriptions| subscriptions.drain().map(|(_, value)| value).collect())
            .unwrap_or_else(|_| Vec::new());
        for subscription in subscriptions {
            let _ = subscription.dispose();
        }
    }
}

#[derive(Clone)]
struct ServerState {
    client: Arc<KimiCodeDesktopClient>,
    assets: Arc<dyn AssetProvider>,
    connections: Arc<Mutex<HashMap<String, Arc<RpcConnection>>>>,
    events: Arc<ApplicationEventBus>,
    token: Arc<str>,
    version: Arc<str>,
    port: u16,
    listen_scope: WebServerListenScope,
}

pub(crate) struct RunningServer {
    pub port: u16,
    pub listen_scope: WebServerListenScope,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
    connections: Arc<Mutex<HashMap<String, Arc<RpcConnection>>>>,
    event_subscription: DisposableHandle,
}

impl RunningServer {
    pub async fn close(self) {
        self.cancellation.cancel();
        let connections = self
            .connections
            .lock()
            .map(|mut connections| connections.drain().map(|(_, value)| value).collect())
            .unwrap_or_else(|_| Vec::new());
        for connection in connections {
            connection.dispose_all();
        }
        let _ = self.event_subscription.dispose();
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(2), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    }
}

pub(crate) async fn start_server(
    client: Arc<KimiCodeDesktopClient>,
    assets: Arc<dyn AssetProvider>,
    events: Arc<ApplicationEventBus>,
    token: String,
    version: String,
    port: u16,
    listen_scope: WebServerListenScope,
) -> Result<RunningServer, String> {
    let bind_address = listen_scope.bind_address();
    let listener = TcpListener::bind((bind_address, port))
        .await
        .map_err(|error| format!("failed to listen on {bind_address}:{port}: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let connections: Arc<Mutex<HashMap<String, Arc<RpcConnection>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let connections_for_events = Arc::clone(&connections);
    let event_subscription = events.subscribe(Arc::new(move |event, payload| {
        let connections = connections_for_events
            .lock()
            .map(|connections| connections.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for connection in connections {
            connection.emit(event, payload.clone());
        }
    }));
    let state = ServerState {
        client,
        assets,
        connections: Arc::clone(&connections),
        events,
        token: token.into(),
        version: version.into(),
        port,
        listen_scope,
    };
    let router = Router::new()
        .route("/_kimi/v1/meta", get(meta_handler))
        .route("/_kimi/v1/rpc", post(rpc_handler))
        .route("/_kimi/v1/files", post(upload_handler))
        .route("/_kimi/v1/events", get(websocket_handler))
        .fallback(static_handler)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state);
    let cancellation = CancellationToken::new();
    let shutdown = cancellation.clone();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await;
    });
    Ok(RunningServer {
        port,
        listen_scope,
        cancellation,
        task,
        connections,
        event_subscription,
    })
}

async fn meta_handler(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    if let Err(response) = authenticate_bearer(&state, &headers) {
        return *response;
    }
    Json(serde_json::json!({
        "serverVersion": state.version,
        "websocket": true,
        "fileUpload": true,
        "fsBrowse": true,
    }))
    .into_response()
}

async fn rpc_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<RpcRequest>,
) -> Response {
    let connection = match authenticate_connection(&state, &headers) {
        Ok(connection) => connection,
        Err(response) => return *response,
    };
    let id = request.id;
    match dispatch_rpc(
        &state.client,
        &state.events,
        &connection,
        &request.command,
        request.args,
    )
    .await
    {
        Ok(result) => Json(RpcResponse::success(id, result)).into_response(),
        Err(error) => Json(RpcResponse::error(
            id,
            error.code,
            error.message,
            error.details.map(Value::Object),
        ))
        .into_response(),
    }
}

async fn upload_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(response) = authenticate_connection(&state, &headers) {
        return *response;
    }
    let request_id = Uuid::new_v4().to_string();
    let mut filename = None;
    let mut media_type = None;
    let mut bytes = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return rpc_http_error(
                    StatusCode::BAD_REQUEST,
                    request_id,
                    "request.invalid",
                    error.to_string(),
                );
            }
        };
        if field.name() != Some("file") {
            continue;
        }
        filename = field.file_name().map(str::to_owned);
        media_type = field.content_type().map(str::to_owned);
        match field.bytes().await {
            Ok(value) => bytes = Some(value.to_vec()),
            Err(error) => {
                return rpc_http_error(
                    StatusCode::BAD_REQUEST,
                    request_id,
                    "request.invalid",
                    error.to_string(),
                );
            }
        }
    }
    let Some(bytes) = bytes else {
        return rpc_http_error(
            StatusCode::BAD_REQUEST,
            request_id,
            "request.invalid",
            "multipart request must include a file field",
        );
    };
    let filename = filename.unwrap_or_else(|| "attachment".into());
    let media_type = media_type.unwrap_or_else(|| "application/octet-stream".into());
    match state
        .client
        .upload_file(&filename, &media_type, bytes)
        .await
    {
        Ok(file) => Json(RpcResponse::success(
            request_id,
            serde_json::to_value(file).unwrap_or(Value::Null),
        ))
        .into_response(),
        Err(error) => rpc_http_error(StatusCode::OK, request_id, "transport.error", error),
    }
}

async fn websocket_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    if let Err(response) = validate_host_and_origin(&state, &headers) {
        return *response;
    }
    let Some(protocol) = websocket_bearer_protocol(&headers) else {
        return rpc_http_error(
            StatusCode::UNAUTHORIZED,
            "",
            "auth.required",
            "a WebSocket bearer credential is required",
        );
    };
    let candidate = protocol.trim_start_matches(WS_BEARER_PROTOCOL_PREFIX);
    if !token_matches(&state.token, candidate) {
        return rpc_http_error(
            StatusCode::UNAUTHORIZED,
            "",
            "auth.invalid",
            "the WebSocket bearer credential is invalid",
        );
    }
    websocket
        .protocols([protocol.clone()])
        .on_upgrade(move |socket| websocket_session(state, socket))
        .into_response()
}

async fn websocket_session(state: ServerState, mut socket: WebSocket) {
    let connection_id = Uuid::new_v4().to_string();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let connection = Arc::new(RpcConnection::new(sender));
    if let Ok(mut connections) = state.connections.lock() {
        connections.insert(connection_id.clone(), Arc::clone(&connection));
    } else {
        let _ = socket.close().await;
        return;
    }

    let ready = serde_json::to_string(&ServerFrame::Ready {
        connection_id: connection_id.clone(),
    })
    .expect("ready frame is serializable");
    if socket.send(Message::Text(ready.into())).await.is_ok() {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
        loop {
            tokio::select! {
                outgoing = receiver.recv() => {
                    let Some(outgoing) = outgoing else { break };
                    if socket.send(Message::Text(outgoing.into())).await.is_err() { break; }
                }
                incoming = socket.next() => {
                    match incoming {
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {}
                    }
                }
                _ = heartbeat.tick() => {
                    if socket.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
                }
            }
        }
    }

    if let Ok(mut connections) = state.connections.lock() {
        connections.remove(&connection_id);
    }
    connection.dispose_all();
}

async fn static_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(response) = validate_host(&state, &headers) {
        return *response;
    }
    if uri.path().starts_with("/_kimi/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let requested = normalized_asset_path(uri.path());
    let asset = requested
        .as_deref()
        .and_then(|path| state.assets.get(path))
        .or_else(|| state.assets.get("index.html"));
    let Some(asset) = asset else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = Response::new(Body::from(asset.bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&asset.mime_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    if let Some(csp) = asset
        .csp_header
        .and_then(|value| HeaderValue::from_str(&value).ok())
    {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    let cache = if requested
        .as_deref()
        .is_some_and(|path| path.starts_with("assets/"))
    {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

fn normalized_asset_path(path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return Some("index.html".into());
    }
    if path.split('/').any(|segment| matches!(segment, "." | "..")) {
        return None;
    }
    Some(path.to_owned())
}

fn authenticate_connection(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Arc<RpcConnection>, Box<Response>> {
    authenticate_bearer(state, headers)?;
    let Some(connection_id) = headers
        .get("x-kimi-connection-id")
        .and_then(|value| value.to_str().ok())
    else {
        return Err(Box::new(rpc_http_error(
            StatusCode::CONFLICT,
            "",
            "connection.required",
            "an active WebSocket connection is required",
        )));
    };
    state
        .connections
        .lock()
        .ok()
        .and_then(|connections| connections.get(connection_id).cloned())
        .ok_or_else(|| {
            Box::new(rpc_http_error(
                StatusCode::CONFLICT,
                "",
                "connection.stale",
                "the WebSocket connection is no longer active",
            ))
        })
}

fn authenticate_bearer(state: &ServerState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    validate_host_and_origin(state, headers)?;
    let credential = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !credential.is_some_and(|value| token_matches(&state.token, value)) {
        return Err(Box::new(rpc_http_error(
            StatusCode::UNAUTHORIZED,
            "",
            "auth.invalid",
            "a valid bearer credential is required",
        )));
    }
    Ok(())
}

fn validate_host_and_origin(state: &ServerState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    validate_host(state, headers)?;
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        && !origin_allowed(state.listen_scope, state.port, host, origin)
    {
        return Err(Box::new(rpc_http_error(
            StatusCode::FORBIDDEN,
            "",
            "origin.rejected",
            "request origin is not allowed",
        )));
    }
    Ok(())
}

fn validate_host(state: &ServerState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !host_allowed(state.listen_scope, state.port, host) {
        return Err(Box::new(rpc_http_error(
            StatusCode::BAD_REQUEST,
            "",
            "host.rejected",
            "request host is not allowed",
        )));
    }
    Ok(())
}

fn host_allowed(scope: WebServerListenScope, port: u16, host: &str) -> bool {
    match scope {
        WebServerListenScope::Local => {
            host.eq_ignore_ascii_case(&format!("127.0.0.1:{port}"))
                || host.eq_ignore_ascii_case(&format!("localhost:{port}"))
        }
        WebServerListenScope::Global => authority_for_port(host, port).is_some(),
    }
}

fn origin_allowed(scope: WebServerListenScope, port: u16, host: &str, origin: &str) -> bool {
    if scope == WebServerListenScope::Local {
        return origin.eq_ignore_ascii_case(&format!("http://127.0.0.1:{port}"))
            || origin.eq_ignore_ascii_case(&format!("http://localhost:{port}"));
    }
    let Ok(uri) = origin.parse::<Uri>() else {
        return false;
    };
    if uri.scheme_str() != Some("http") || uri.query().is_some() {
        return false;
    }
    let Some(origin_authority) = uri.authority() else {
        return false;
    };
    let Some(host_authority) = authority_for_port(host, port) else {
        return false;
    };
    authority_port(origin_authority) == port
        && origin_authority
            .host()
            .eq_ignore_ascii_case(host_authority.host())
}

fn authority_for_port(value: &str, port: u16) -> Option<Authority> {
    if value.contains('@') {
        return None;
    }
    let authority = value.parse::<Authority>().ok()?;
    (authority_port(&authority) == port).then_some(authority)
}

fn authority_port(authority: &Authority) -> u16 {
    authority.port_u16().unwrap_or(80)
}

fn websocket_bearer_protocol(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)?
        .to_str()
        .ok()?
        .split(',')
        .map(str::trim)
        .find(|protocol| protocol.starts_with(WS_BEARER_PROTOCOL_PREFIX))
        .map(str::to_owned)
}

fn token_matches(expected: &str, candidate: &str) -> bool {
    expected.len() == candidate.len() && bool::from(expected.as_bytes().ct_eq(candidate.as_bytes()))
}

fn rpc_http_error(
    status: StatusCode,
    id: impl Into<String>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> Response {
    (status, Json(RpcResponse::error(id, code, message, None))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use kimi_code_agent_core_v2::_base::di::lifecycle::to_disposable;
    use reqwest::{Client, multipart};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kimi-web-server-{}", Uuid::new_v4()))
    }

    async fn rpc_call(
        http: &Client,
        origin: &str,
        token: &str,
        connection_id: &str,
        id: &str,
        command: &str,
        args: Value,
    ) -> Value {
        http.post(format!("{origin}/_kimi/v1/rpc"))
            .bearer_auth(token)
            .header("x-kimi-connection-id", connection_id)
            .json(&json!({"id": id, "command": command, "args": args}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn authenticates_bridge_routes_and_serves_spa_assets() {
        let root = temp_dir();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let plugin_source = root.join("web-plugin");
        std::fs::create_dir_all(&plugin_source).unwrap();
        std::fs::write(
            plugin_source.join("kimi.plugin.json"),
            r#"{"name":"web-plugin"}"#,
        )
        .unwrap();
        let client = Arc::new(KimiCodeDesktopClient::new(&home, "test").unwrap());
        let assets: Arc<dyn AssetProvider> = Arc::new(|path: &str| match path {
            "index.html" => Some(WebAsset {
                bytes: b"<main>Kimi Web</main>".to_vec(),
                mime_type: "text/html; charset=utf-8".into(),
                csp_header: Some("default-src 'self'".into()),
            }),
            "assets/app.js" => Some(WebAsset {
                bytes: b"export {};".to_vec(),
                mime_type: "text/javascript".into(),
                csp_header: None,
            }),
            _ => None,
        });
        let token = "test-token-that-must-never-appear-in-errors";
        let events = Arc::new(ApplicationEventBus::default());
        let server = start_server(
            client,
            assets,
            Arc::clone(&events),
            token.into(),
            "1.2.3".into(),
            0,
            WebServerListenScope::Local,
        )
        .await
        .unwrap();
        let origin = format!("http://127.0.0.1:{}", server.port);
        let http = Client::new();

        let page = http
            .get(format!("{origin}/conversation/example"))
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(
            page.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(page.text().await.unwrap(), "<main>Kimi Web</main>");

        let asset = http
            .get(format!("{origin}/assets/app.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(asset.status(), StatusCode::OK);
        assert_eq!(asset.headers()[header::CONTENT_TYPE], "text/javascript");
        assert_eq!(
            asset.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );

        assert_eq!(
            http.get(format!("{origin}/_kimi/v1/missing"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            http.get(format!("{origin}/"))
                .header(header::HOST, "malicious.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );

        let unauthorized = http
            .get(format!("{origin}/_kimi/v1/meta"))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert!(!unauthorized.text().await.unwrap().contains(token));
        assert_eq!(
            http.get(format!("{origin}/_kimi/v1/meta"))
                .bearer_auth("wrong-token")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            http.get(format!("{origin}/_kimi/v1/meta"))
                .bearer_auth(token)
                .header(header::ORIGIN, "http://malicious.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let meta: Value = http
            .get(format!("{origin}/_kimi/v1/meta"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(meta["serverVersion"], "1.2.3");

        let websocket_url = format!("ws://127.0.0.1:{}/_kimi/v1/events", server.port);
        let mut request = websocket_url.into_client_request().unwrap();
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_str(&format!("{WS_BEARER_PROTOCOL_PREFIX}{token}")).unwrap(),
        );
        request
            .headers_mut()
            .insert(header::ORIGIN, HeaderValue::from_str(&origin).unwrap());
        let (mut websocket, response) = connect_async(request).await.unwrap();
        assert_eq!(
            response.headers()[header::SEC_WEBSOCKET_PROTOCOL],
            format!("{WS_BEARER_PROTOCOL_PREFIX}{token}")
        );
        let ready: Value =
            serde_json::from_str(websocket.next().await.unwrap().unwrap().to_text().unwrap())
                .unwrap();
        let connection_id = ready["connectionId"].as_str().unwrap().to_owned();

        let plugins: Value = http
            .post(format!("{origin}/_kimi/v1/rpc"))
            .bearer_auth(token)
            .header("x-kimi-connection-id", &connection_id)
            .json(&json!({"id":"rpc-plugins", "command":"list_plugins", "args":{}}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(plugins["ok"], true);
        assert_eq!(plugins["result"], json!([]));

        let provider = rpc_call(
            &http,
            &origin,
            token,
            &connection_id,
            "rpc-provider-save",
            "save_provider",
            json!({"input": {
                "id": "example-provider",
                "type": "openai",
                "apiKey": "YOUR_API_KEY",
                "replaceApiKey": true,
                "baseUrl": "https://api.example.test/v1",
                "defaultModel": "example-model",
                "models": [{
                    "model": "example-model",
                    "displayName": "Example Model",
                    "maxContextSize": 131072,
                    "capabilities": ["tool_use", "thinking"],
                    "supportEfforts": [],
                    "adaptiveThinking": true
                }]
            }}),
        )
        .await;
        assert_eq!(provider["ok"], true);
        assert_eq!(provider["result"]["id"], "example-provider");
        assert!(provider["result"].get("apiKey").is_none());
        let providers = rpc_call(
            &http,
            &origin,
            token,
            &connection_id,
            "rpc-providers",
            "list_providers",
            json!({}),
        )
        .await;
        assert_eq!(providers["result"].as_array().unwrap().len(), 1);
        let provider_delete = rpc_call(
            &http,
            &origin,
            token,
            &connection_id,
            "rpc-provider-delete",
            "delete_provider",
            json!({"id": "example-provider"}),
        )
        .await;
        assert_eq!(provider_delete["ok"], true);

        let install = rpc_call(
            &http,
            &origin,
            token,
            &connection_id,
            "rpc-plugin-install",
            "install_plugin",
            json!({
                "source": plugin_source.to_string_lossy(),
                "operationId": "web-plugin-install"
            }),
        )
        .await;
        assert_eq!(install["ok"], true);

        let mut progress = Value::Null;
        for _ in 0..100 {
            progress = rpc_call(
                &http,
                &origin,
                token,
                &connection_id,
                "rpc-plugin-progress",
                "get_plugin_install_progress",
                json!({"operationId": "web-plugin-install"}),
            )
            .await;
            if progress["result"]["phase"] == "complete" || progress["result"]["error"].is_string()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(progress["ok"], true);
        assert_eq!(progress["result"]["phase"], "complete");

        let plugins = rpc_call(
            &http,
            &origin,
            token,
            &connection_id,
            "rpc-plugins-after-install",
            "list_plugins",
            json!({}),
        )
        .await;
        assert_eq!(plugins["result"].as_array().unwrap().len(), 1);
        assert_eq!(plugins["result"][0]["id"], "web-plugin");

        let remove = rpc_call(
            &http,
            &origin,
            token,
            &connection_id,
            "rpc-plugin-remove",
            "remove_plugin",
            json!({"id": "web-plugin"}),
        )
        .await;
        assert_eq!(remove["ok"], true);

        events.desktop_state_changed(crate::DesktopStateChange::WorkspaceUpserted {
            workspace_id: "workspace-from-desktop".into(),
        });
        let desktop_change: Value = loop {
            let message = websocket.next().await.unwrap().unwrap();
            if !message.is_text() {
                continue;
            }
            break serde_json::from_str(message.to_text().unwrap()).unwrap();
        };
        assert_eq!(desktop_change["event"], crate::DESKTOP_STATE_CHANGED_EVENT);
        assert_eq!(
            desktop_change["payload"]["workspaceId"],
            "workspace-from-desktop"
        );

        let workspace_root = root.join("workspace");
        std::fs::create_dir_all(&workspace_root).unwrap();
        let workspace: Value = http
            .post(format!("{origin}/_kimi/v1/rpc"))
            .bearer_auth(token)
            .header("x-kimi-connection-id", &connection_id)
            .json(&json!({
                "id":"rpc-workspace",
                "command":"create_or_touch_workspace",
                "args":{"root": workspace_root.to_string_lossy()}
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(workspace["ok"], true);
        let web_change: Value = loop {
            let message = websocket.next().await.unwrap().unwrap();
            if !message.is_text() {
                continue;
            }
            break serde_json::from_str(message.to_text().unwrap()).unwrap();
        };
        assert_eq!(web_change["event"], crate::DESKTOP_STATE_CHANGED_EVENT);
        assert_eq!(web_change["payload"]["kind"], "workspace_upserted");
        assert_eq!(
            web_change["payload"]["workspaceId"],
            workspace["result"]["id"]
        );

        let unknown: Value = http
            .post(format!("{origin}/_kimi/v1/rpc"))
            .bearer_auth(token)
            .header("x-kimi-connection-id", &connection_id)
            .json(&json!({"id":"rpc-1", "command":"not_allowed", "args":{}}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(unknown["ok"], false);
        assert_eq!(unknown["error"]["code"], "request.unknown_command");

        let no_file = http
            .post(format!("{origin}/_kimi/v1/files"))
            .bearer_auth(token)
            .header("x-kimi-connection-id", &connection_id)
            .multipart(multipart::Form::new().text("ignored", "value"))
            .send()
            .await
            .unwrap();
        assert_eq!(no_file.status(), StatusCode::BAD_REQUEST);

        websocket.close(None).await.unwrap();
        let mut stale_status = StatusCode::OK;
        for _ in 0..50 {
            stale_status = http
                .post(format!("{origin}/_kimi/v1/rpc"))
                .bearer_auth(token)
                .header("x-kimi-connection-id", &connection_id)
                .json(&json!({"id":"rpc-2", "command":"auth_status", "args":{}}))
                .send()
                .await
                .unwrap()
                .status();
            if stale_status == StatusCode::CONFLICT {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(stale_status, StatusCode::CONFLICT);

        server.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_traversal_and_compares_tokens_exactly() {
        assert_eq!(normalized_asset_path("/"), Some("index.html".into()));
        assert_eq!(
            normalized_asset_path("/assets/app.js"),
            Some("assets/app.js".into())
        );
        assert_eq!(normalized_asset_path("/../secret"), None);
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abc0"));
        assert!(host_allowed(
            WebServerListenScope::Global,
            58627,
            "example.test:58627"
        ));
        assert!(!host_allowed(
            WebServerListenScope::Global,
            58627,
            "example.test:1234"
        ));
        assert!(origin_allowed(
            WebServerListenScope::Global,
            58627,
            "example.test:58627",
            "http://example.test:58627"
        ));
        assert!(!origin_allowed(
            WebServerListenScope::Global,
            58627,
            "example.test:58627",
            "http://malicious.test:58627"
        ));
        assert!(!origin_allowed(
            WebServerListenScope::Global,
            58627,
            "example.test:58627",
            "https://example.test:58627"
        ));
    }

    #[test]
    fn connection_disposes_owned_subscriptions() {
        let (sender, _receiver) = mpsc::unbounded_channel();
        let connection = RpcConnection::new(sender);
        let disposed = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let disposed = Arc::clone(&disposed);
            connection
                .add_subscription(to_disposable(move || {
                    disposed.fetch_add(1, Ordering::SeqCst);
                }))
                .unwrap();
        }
        connection.dispose_all();
        assert_eq!(disposed.load(Ordering::SeqCst), 2);
        connection.dispose_all();
        assert_eq!(disposed.load(Ordering::SeqCst), 2);
    }
}
