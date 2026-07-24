use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/tools.ts, registerToolsRoutes().
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
