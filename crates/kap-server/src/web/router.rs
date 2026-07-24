use std::sync::Arc;

use axum::extract::State;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::{Json, Router};
use kimi_code_protocol::{AsyncApiDocumentOptions, create_async_api_document};
use serde_json::Value;

use crate::routes::{
    create_open_api_document, register_api_v1_routes, web_assets::serve_web_asset,
};

use super::middleware::boundary;
use super::state::AppState;
use super::websocket::{WS_PATH, ws_upgrade};

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

/// Axum transport composition. REST ownership remains in `crate::routes`.
pub fn create_router(state: Arc<AppState>) -> Router {
    let router = Router::<Arc<AppState>>::new()
        .route(WS_PATH, get(ws_upgrade))
        .route("/asyncapi.json", get(async_api))
        .route("/openapi.json", get(open_api));
    let router = register_api_v1_routes::register(router, &state);
    router
        .fallback(serve_web_asset)
        .with_state(Arc::clone(&state))
        .layer(from_fn_with_state(state, boundary))
}
