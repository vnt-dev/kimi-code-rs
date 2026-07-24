use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/tools",
        "/api/v1/tools",
        CoreOperation::ListTools,
    ),
    route(
        "GET",
        "/api/v1/mcp/servers",
        "/api/v1/mcp/servers",
        CoreOperation::ListMcpServers,
    ),
    route(
        "POST",
        "/api/v1/mcp/servers/{tail}",
        "/api/v1/mcp/servers/{tail}",
        CoreOperation::McpServerAction,
    ),
];

async fn list_tools(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListTools, request).await
}

async fn list_mcp_servers(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListMcpServers, request).await
}

async fn mcp_server_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::McpServerAction, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/tools", get(list_tools))
        .route("/api/v1/mcp/servers", get(list_mcp_servers))
        .route("/api/v1/mcp/servers/{tail}", post(mcp_server_action))
}
