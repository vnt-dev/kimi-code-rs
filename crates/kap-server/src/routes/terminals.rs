use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/terminals",
        "/api/v1/sessions/{session_id}/terminals",
        CoreOperation::ListTerminals,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/terminals",
        "/api/v1/sessions/{session_id}/terminals",
        CoreOperation::CreateTerminal,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/terminals/{item}",
        "/api/v1/sessions/{session_id}/terminals/{terminal_id}",
        CoreOperation::GetTerminal,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/terminals/{item}",
        "/api/v1/sessions/{session_id}/terminals/{tail}",
        CoreOperation::TerminalAction,
    ),
];

async fn list_terminals(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListTerminals, request).await
}

async fn create_terminal(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::CreateTerminal, request).await
}

async fn get_terminal(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetTerminal, request).await
}

async fn terminal_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::TerminalAction, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route(
            "/api/v1/sessions/{session_id}/terminals",
            get(list_terminals),
        )
        .route(
            "/api/v1/sessions/{session_id}/terminals",
            post(create_terminal),
        )
        .route(
            "/api/v1/sessions/{session_id}/terminals/{item}",
            get(get_terminal),
        )
        .route(
            "/api/v1/sessions/{session_id}/terminals/{item}",
            post(terminal_action),
        )
}
