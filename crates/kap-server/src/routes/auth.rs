use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::get;

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/auth",
    "/api/v1/auth",
    CoreOperation::GetAuth,
)];

// Original: routes/auth.ts, GET /auth.
async fn get_auth(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetAuth, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route("/api/v1/auth", get(get_auth))
}
