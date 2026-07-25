use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/skills",
        "/api/v1/sessions/{session_id}/skills",
        CoreOperation::ListSkills,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/skills/{tail}",
        "/api/v1/sessions/{session_id}/skills/{tail}",
        CoreOperation::SkillAction,
    ),
    route(
        "GET",
        "/api/v1/workspaces/{workspace_id}/skills",
        "/api/v1/workspaces/{workspace_id}/skills",
        CoreOperation::ListWorkspaceSkills,
    ),
];

async fn list_session_skills(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListSkills, request).await
}

async fn session_skill_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::SkillAction, request).await
}

async fn list_workspace_skills(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListWorkspaceSkills, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route(
            "/api/v1/sessions/{session_id}/skills",
            get(list_session_skills),
        )
        .route(
            "/api/v1/sessions/{session_id}/skills/{tail}",
            post(session_skill_action),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/skills",
            get(list_workspace_skills),
        )
}
