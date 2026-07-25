use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kimi_code_protocol::{
    GuiStoreGetItemQuery, GuiStoreGetItemResponse, GuiStoreLengthResponse, GuiStoreRemoveItemBody,
    GuiStoreSetItemBody, err_envelope, ok_envelope,
};
use serde_json::Value;

use crate::web::{AppState, middleware::RequestId};

// Original: routes/guiStore.ts, getItem handler.
async fn get_item(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    RawQuery(query): RawQuery,
) -> Response {
    let query = query.unwrap_or_default();
    let query = match serde_urlencoded::from_str::<GuiStoreGetItemQuery>(&query) {
        Ok(query) => query,
        Err(error) => return validation_error(request_id.0, error.to_string()),
    };
    match state.gui_store.get_item(&query.key).await {
        Ok(value) => {
            Json(ok_envelope(GuiStoreGetItemResponse { value }, request_id.0)).into_response()
        }
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

// Original: routes/guiStore.ts, setItem handler.
async fn set_item(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    let body = match serde_json::from_slice::<GuiStoreSetItemBody>(&body) {
        Ok(body) => body,
        Err(error) => return validation_error(request_id.0, error.to_string()),
    };
    match state.gui_store.set_item(body.key, body.value).await {
        Ok(()) => Json(ok_envelope(Value::Null, request_id.0)).into_response(),
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

// Original: routes/guiStore.ts, removeItem handler.
async fn remove_item(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    body: Bytes,
) -> Response {
    let body = match serde_json::from_slice::<GuiStoreRemoveItemBody>(&body) {
        Ok(body) => body,
        Err(error) => return validation_error(request_id.0, error.to_string()),
    };
    match state.gui_store.remove_item(&body.key).await {
        Ok(()) => Json(ok_envelope(Value::Null, request_id.0)).into_response(),
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

async fn clear(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match state.gui_store.clear().await {
        Ok(()) => Json(ok_envelope(Value::Null, request_id.0)).into_response(),
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

async fn length(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    match state.gui_store.len().await {
        Ok(length) => Json(ok_envelope(
            GuiStoreLengthResponse {
                length: length as f64,
            },
            request_id.0,
        ))
        .into_response(),
        Err(error) => internal_error(request_id.0, error.to_string()),
    }
}

fn validation_error(request_id: String, message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(err_envelope(40_001, message, request_id, None)),
    )
        .into_response()
}

fn internal_error(request_id: String, message: impl Into<String>) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(err_envelope(50_000, message, request_id, None)),
    )
        .into_response()
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/gui/store/getItem", get(get_item))
        .route("/api/v1/gui/store/setItem", post(set_item))
        .route("/api/v1/gui/store/removeItem", post(remove_item))
        .route("/api/v1/gui/store/clear", post(clear))
        .route("/api/v1/gui/store/length", get(length))
}
