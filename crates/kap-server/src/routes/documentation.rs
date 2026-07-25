use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use kimi_code_protocol::{AsyncApiDocumentOptions, create_async_api_document};
use serde_json::Value;

use super::create_open_api_document;
use crate::web::AppState;

// Original: start.ts, GET /asyncapi.json.
async fn get_async_api(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(create_async_api_document(AsyncApiDocumentOptions {
        version: Some(state.server_version.clone()),
        server_host: Some(state.host.clone()),
        ..AsyncApiDocumentOptions::default()
    }))
}

// Original: start.ts, GET /openapi.json.
async fn get_open_api(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(create_open_api_document(&state))
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/asyncapi.json", get(get_async_api))
        .route("/openapi.json", get(get_open_api))
}
