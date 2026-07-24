use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::get;

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/messages",
        "/api/v1/sessions/{session_id}/messages",
        CoreOperation::ListMessages,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/messages/{message_id}",
        "/api/v1/sessions/{session_id}/messages/{message_id}",
        CoreOperation::GetMessage,
    ),
];

async fn list_messages(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListMessages, request).await
}

async fn get_message(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetMessage, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/sessions/{session_id}/messages", get(list_messages))
        .route(
            "/api/v1/sessions/{session_id}/messages/{message_id}",
            get(get_message),
        )
}
