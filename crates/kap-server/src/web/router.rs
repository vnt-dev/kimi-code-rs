use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::{Json, Router};
use kimi_code_protocol::{AsyncApiDocumentOptions, create_async_api_document, ok_envelope};
use serde_json::{Value, json};

use crate::transport::ws::connection_registry::ConnectionLike;

use super::middleware::{RequestId, boundary};
use super::state::AppState;

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
    Json(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Kimi Code Server API",
            "description": "REST API for the Kimi Code local server. All JSON responses are wrapped in a uniform envelope `{ code, msg, data, request_id }`.",
            "version": state.server_version
        },
        "paths": {
            "/api/v1/healthz": {"get": {"description": "Health check"}},
            "/api/v1/meta": {"get": {"description": "Get server metadata"}},
            "/api/v1/connections": {"get": {"description": "List active WebSocket clients connected to the server"}},
            "/api/v1/shutdown": {"post": {"description": "Shut down the server"}}
        }
    }))
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/healthz", get(health))
        .route("/api/v1/meta", get(meta))
        .route("/api/v1/connections", get(connections))
        .route("/api/v1/shutdown", post(shutdown))
        .route("/asyncapi.json", get(async_api))
        .route("/openapi.json", get(open_api))
        .with_state(Arc::clone(&state))
        .layer(from_fn_with_state(state, boundary))
}
