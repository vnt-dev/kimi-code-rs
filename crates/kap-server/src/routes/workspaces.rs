use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{delete, get, patch, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

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

async fn list_workspaces(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListWorkspaces, request).await
}

async fn create_workspace(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::CreateWorkspace, request).await
}

async fn update_workspace(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::UpdateWorkspace, request).await
}

async fn delete_workspace(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DeleteWorkspace, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/workspaces", get(list_workspaces))
        .route("/api/v1/workspaces", post(create_workspace))
        .route("/api/v1/workspaces/{workspace_id}", patch(update_workspace))
        .route(
            "/api/v1/workspaces/{workspace_id}",
            delete(delete_workspace),
        )
}
