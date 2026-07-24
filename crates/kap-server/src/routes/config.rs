use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/config",
        "/api/v1/config",
        CoreOperation::GetConfig,
    ),
    route(
        "POST",
        "/api/v1/config",
        "/api/v1/config",
        CoreOperation::UpdateConfig,
    ),
];

// Original: routes/config.ts, GET /config.
async fn get_config(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetConfig, request).await
}

// Original: routes/config.ts, POST /config.
async fn update_config(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::UpdateConfig, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/config", get(get_config))
        .route("/api/v1/config", post(update_config))
}
