use std::sync::Arc;

use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::get;

use crate::routes::{documentation, register_api_v1_routes, web_assets::serve_web_asset};

use super::middleware::boundary;
use super::state::AppState;
use super::websocket::{WS_PATH, ws_upgrade};

/// Axum transport composition. HTTP handler ownership remains in
/// `crate::routes`; WebSocket transport remains in `crate::web::websocket`.
pub fn create_router(state: Arc<AppState>) -> Router {
    let router = Router::<Arc<AppState>>::new().route(WS_PATH, get(ws_upgrade));
    let router = documentation::register(router);
    let router = register_api_v1_routes::register(router, &state);
    router
        .fallback(serve_web_asset)
        .with_state(Arc::clone(&state))
        .layer(from_fn_with_state(state, boundary))
}
