use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::get;

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/sessions/{session_id}/transcript",
    "/api/v1/sessions/{session_id}/transcript",
    CoreOperation::GetTranscript,
)];

async fn get_transcript(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetTranscript, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route(
        "/api/v1/sessions/{session_id}/transcript",
        get(get_transcript),
    )
}
