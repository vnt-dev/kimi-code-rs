use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/debug/channels",
        "/api/v1/debug/channels",
        CoreOperation::DebugChannels,
    ),
    route(
        "GET",
        "/api/v1/debug/{service}/{method}",
        "/api/v1/debug/{service}/{method}",
        CoreOperation::DebugGlobalGet,
    ),
    route(
        "POST",
        "/api/v1/debug/{service}/{method}",
        "/api/v1/debug/{service}/{method}",
        CoreOperation::DebugGlobalPost,
    ),
    route(
        "GET",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        CoreOperation::DebugSessionGet,
    ),
    route(
        "POST",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        CoreOperation::DebugSessionPost,
    ),
    route(
        "GET",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        CoreOperation::DebugAgentGet,
    ),
    route(
        "POST",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        CoreOperation::DebugAgentPost,
    ),
];

async fn list_debug_channels(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DebugChannels, request).await
}

async fn debug_global_get(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DebugGlobalGet, request).await
}

async fn debug_global_post(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DebugGlobalPost, request).await
}

async fn debug_session_get(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DebugSessionGet, request).await
}

async fn debug_session_post(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DebugSessionPost, request).await
}

async fn debug_agent_get(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DebugAgentGet, request).await
}

async fn debug_agent_post(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DebugAgentPost, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/debug/channels", get(list_debug_channels))
        .route("/api/v1/debug/{service}/{method}", get(debug_global_get))
        .route("/api/v1/debug/{service}/{method}", post(debug_global_post))
        .route(
            "/api/v1/debug/session/{session_id}/{service}/{method}",
            get(debug_session_get),
        )
        .route(
            "/api/v1/debug/session/{session_id}/{service}/{method}",
            post(debug_session_post),
        )
        .route(
            "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
            get(debug_agent_get),
        )
        .route(
            "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
            post(debug_agent_post),
        )
}
