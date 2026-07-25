use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::get;

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/sessions/{session_id}/fs/{*path}",
    "/api/v1/sessions/{session_id}/fs/{*}",
    CoreOperation::ReadSessionFile,
)];

async fn read_session_file(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ReadSessionFile, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route(
        "/api/v1/sessions/{session_id}/fs/{*path}",
        get(read_session_file),
    )
}
