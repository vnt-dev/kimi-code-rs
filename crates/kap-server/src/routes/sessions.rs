use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions",
        "/api/v1/sessions",
        CoreOperation::ListSessions,
    ),
    route(
        "POST",
        "/api/v1/sessions",
        "/api/v1/sessions",
        CoreOperation::CreateSession,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_ref}",
        "/api/v1/sessions/{session_id}",
        CoreOperation::GetSession,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_ref}",
        "/api/v1/sessions/{session_id}:archive",
        CoreOperation::SessionAction,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/{tail}",
        "/api/v1/sessions/{session_id}/{tail}",
        CoreOperation::SessionNestedAction,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/children",
        "/api/v1/sessions/{session_id}/children",
        CoreOperation::ListSessionChildren,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/children",
        "/api/v1/sessions/{session_id}/children",
        CoreOperation::CreateSessionChild,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/goal",
        "/api/v1/sessions/{session_id}/goal",
        CoreOperation::GetSessionGoal,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/profile",
        "/api/v1/sessions/{session_id}/profile",
        CoreOperation::GetSessionProfile,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/profile",
        "/api/v1/sessions/{session_id}/profile",
        CoreOperation::UpdateSessionProfile,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/status",
        "/api/v1/sessions/{session_id}/status",
        CoreOperation::GetSessionStatus,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/warnings",
        "/api/v1/sessions/{session_id}/warnings",
        CoreOperation::GetSessionWarnings,
    ),
];

async fn list_sessions(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListSessions, request).await
}

async fn create_session(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::CreateSession, request).await
}

async fn get_session(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetSession, request).await
}

async fn session_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::SessionAction, request).await
}

async fn session_nested_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::SessionNestedAction, request).await
}

async fn list_session_children(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListSessionChildren, request).await
}

async fn create_session_child(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::CreateSessionChild, request).await
}

async fn get_session_goal(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetSessionGoal, request).await
}

async fn get_session_profile(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetSessionProfile, request).await
}

async fn update_session_profile(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::UpdateSessionProfile, request).await
}

async fn get_session_status(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetSessionStatus, request).await
}

async fn get_session_warnings(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetSessionWarnings, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/sessions", post(create_session))
        .route("/api/v1/sessions/{session_ref}", get(get_session))
        .route("/api/v1/sessions/{session_ref}", post(session_action))
        .route(
            "/api/v1/sessions/{session_id}/{tail}",
            post(session_nested_action),
        )
        .route(
            "/api/v1/sessions/{session_id}/children",
            get(list_session_children),
        )
        .route(
            "/api/v1/sessions/{session_id}/children",
            post(create_session_child),
        )
        .route("/api/v1/sessions/{session_id}/goal", get(get_session_goal))
        .route(
            "/api/v1/sessions/{session_id}/profile",
            get(get_session_profile),
        )
        .route(
            "/api/v1/sessions/{session_id}/profile",
            post(update_session_profile),
        )
        .route(
            "/api/v1/sessions/{session_id}/status",
            get(get_session_status),
        )
        .route(
            "/api/v1/sessions/{session_id}/warnings",
            get(get_session_warnings),
        )
}
