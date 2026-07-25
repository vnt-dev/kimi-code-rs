use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/approvals",
        "/api/v1/sessions/{session_id}/approvals",
        CoreOperation::ListApprovals,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/approvals/{approval_id}",
        "/api/v1/sessions/{session_id}/approvals/{approval_id}",
        CoreOperation::ResolveApproval,
    ),
];

async fn list_approvals(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListApprovals, request).await
}

async fn resolve_approval(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ResolveApproval, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route(
            "/api/v1/sessions/{session_id}/approvals",
            get(list_approvals),
        )
        .route(
            "/api/v1/sessions/{session_id}/approvals/{approval_id}",
            post(resolve_approval),
        )
}
