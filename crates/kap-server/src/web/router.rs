use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, RawQuery, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kimi_code_protocol::{AsyncApiDocumentOptions, create_async_api_document, ok_envelope};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::transport::ws::connection_registry::ConnectionLike;

use super::api::{create_open_api_document, register_core_routes};
use super::middleware::{RequestId, boundary};
use super::state::AppState;
use super::web_assets::serve_web_asset;
use super::websocket::{WS_PATH, ws_upgrade};

// Original: routes/registerApiV1Routes.ts, registerHealthRoute().
async fn health(Extension(request_id): Extension<RequestId>) -> Json<Value> {
    Json(json!(ok_envelope(json!({"ok": true}), request_id.0)))
}

// Original: routes/meta.ts, registerMetaRoute().
async fn meta(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Json<Value> {
    Json(json!(ok_envelope(
        json!({
            "server_version": state.server_version,
            "capabilities": {
                "websocket": true,
                "file_upload": true,
                "fs_query": true,
                "mcp": true,
                "tasks": true,
                "terminal": true
            },
            "server_id": state.server_id,
            "started_at": state.started_at,
            "open_in_apps": [],
            "dangerous_bypass_auth": state.disable_auth,
            "backend": "v2"
        }),
        request_id.0
    )))
}

// Original: routes/connections.ts, registerConnectionsRoutes().
async fn connections(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Json<Value> {
    let mut connections = state
        .connection_registry
        .values()
        .iter()
        .map(|connection| connection_json(connection.as_ref()))
        .collect::<Vec<_>>();
    connections.sort_by(|left, right| {
        left["connected_at"]
            .as_str()
            .cmp(&right["connected_at"].as_str())
    });
    Json(json!(ok_envelope(
        json!({"connections": connections}),
        request_id.0
    )))
}

fn connection_json(connection: &dyn ConnectionLike) -> Value {
    json!({
        "id": connection.id(),
        "connected_at": connection.connected_at(),
        "remote_address": connection.remote_address(),
        "user_agent": connection.user_agent(),
        "has_client_hello": connection.has_client_hello(),
        "subscriptions": connection.subscription_session_ids()
    })
}

async fn shutdown(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> (StatusCode, Json<Value>) {
    if !state.enable_shutdown {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": 40400,
                "msg": "Not Found",
                "data": null,
                "request_id": request_id.0
            })),
        );
    }
    let _ = state.shutdown.send(true);
    (
        StatusCode::OK,
        Json(json!(ok_envelope(json!({}), request_id.0))),
    )
}

async fn async_api(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(create_async_api_document(AsyncApiDocumentOptions {
        version: Some(state.server_version.clone()),
        server_host: Some(state.host.clone()),
        ..AsyncApiDocumentOptions::default()
    }))
}

async fn open_api(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(create_open_api_document(&state))
}

#[derive(Deserialize)]
struct GuiSetItem {
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct GuiKey {
    key: String,
}

// Original: routes/guiStore.ts, getItem handler.
async fn gui_get_item(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    RawQuery(query): RawQuery,
) -> Response {
    let key = query.as_deref().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(name, _)| name == "key")
            .map(|(_, value)| value.into_owned())
    });
    let Some(key) = key else {
        return validation_error(request_id.0, "query parameter `key` is required");
    };
    match state.gui_store.get_item(&key).await {
        Ok(value) => {
            Json(json!(ok_envelope(json!({"value": value}), request_id.0))).into_response()
        }
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

// Original: routes/guiStore.ts, setItem handler.
async fn gui_set_item(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    let body = match serde_json::from_slice::<GuiSetItem>(&body) {
        Ok(body) => body,
        Err(error) => return validation_error(request_id.0, error.to_string()),
    };
    match state.gui_store.set_item(body.key, body.value).await {
        Ok(()) => Json(json!(ok_envelope(Value::Null, request_id.0))).into_response(),
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

// Original: routes/guiStore.ts, removeItem handler.
async fn gui_remove_item(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    let body = match serde_json::from_slice::<GuiKey>(&body) {
        Ok(body) => body,
        Err(error) => return validation_error(request_id.0, error.to_string()),
    };
    match state.gui_store.remove_item(&body.key).await {
        Ok(()) => Json(json!(ok_envelope(Value::Null, request_id.0))).into_response(),
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

// Original: routes/guiStore.ts, clear handler.
async fn gui_clear(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match state.gui_store.clear().await {
        Ok(()) => Json(json!(ok_envelope(Value::Null, request_id.0))).into_response(),
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

// Original: routes/guiStore.ts, length handler.
async fn gui_length(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match state.gui_store.len().await {
        Ok(length) => {
            Json(json!(ok_envelope(json!({"length": length}), request_id.0))).into_response()
        }
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

fn validation_error(request_id: String, message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "code": 40001,
            "msg": message.into(),
            "data": null,
            "request_id": request_id
        })),
    )
        .into_response()
}

fn internal_error(request_id: String, message: impl Into<String>) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "code": 50000,
            "msg": message.into(),
            "data": null,
            "request_id": request_id
        })),
    )
        .into_response()
}

pub fn create_router(state: Arc<AppState>) -> Router {
    let mut router = Router::<Arc<AppState>>::new()
        .route("/api/v1/healthz", get(health))
        .route("/api/v1/meta", get(meta))
        .route("/api/v1/connections", get(connections))
        .route("/api/v1/gui/store/getItem", get(gui_get_item))
        .route("/api/v1/gui/store/setItem", post(gui_set_item))
        .route("/api/v1/gui/store/removeItem", post(gui_remove_item))
        .route("/api/v1/gui/store/clear", post(gui_clear))
        .route("/api/v1/gui/store/length", get(gui_length))
        .route(WS_PATH, get(ws_upgrade))
        .route("/asyncapi.json", get(async_api))
        .route("/openapi.json", get(open_api));
    if state.enable_shutdown {
        router = router.route("/api/v1/shutdown", post(shutdown));
    }
    let router = register_core_routes(router, &state);
    router
        .fallback(serve_web_asset)
        .with_state(Arc::clone(&state))
        .layer(from_fn_with_state(state, boundary))
}
