use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::get;

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/sessions/{session_id}/snapshot",
    "/api/v1/sessions/{session_id}/snapshot",
    CoreOperation::GetSnapshot,
)];

async fn get_snapshot(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetSnapshot, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route("/api/v1/sessions/{session_id}/snapshot", get(get_snapshot))
}
