use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::post;

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[route(
    "POST",
    "/api/v1/sessions/{session_id}/export",
    "/api/v1/sessions/{session_id}/export",
    CoreOperation::ExportSession,
)];

async fn export_session(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ExportSession, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route("/api/v1/sessions/{session_id}/export", post(export_session))
}
