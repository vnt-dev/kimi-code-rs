use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/workspaces.ts, registerWorkspacesRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/workspaces",
        "/api/v1/workspaces",
        CoreOperation::ListWorkspaces,
    ),
    route(
        "POST",
        "/api/v1/workspaces",
        "/api/v1/workspaces",
        CoreOperation::CreateWorkspace,
    ),
    route(
        "PATCH",
        "/api/v1/workspaces/{workspace_id}",
        "/api/v1/workspaces/{workspace_id}",
        CoreOperation::UpdateWorkspace,
    ),
    route(
        "DELETE",
        "/api/v1/workspaces/{workspace_id}",
        "/api/v1/workspaces/{workspace_id}",
        CoreOperation::DeleteWorkspace,
    ),
];
