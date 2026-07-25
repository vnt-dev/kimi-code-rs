use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/tasks",
        "/api/v1/sessions/{session_id}/tasks",
        CoreOperation::ListTasks,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/tasks/{item}",
        "/api/v1/sessions/{session_id}/tasks/{task_id}",
        CoreOperation::GetTask,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/tasks/{item}",
        "/api/v1/sessions/{session_id}/tasks/{tail}",
        CoreOperation::TaskAction,
    ),
];

async fn list_tasks(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListTasks, request).await
}

async fn get_task(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetTask, request).await
}

async fn task_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::TaskAction, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/sessions/{session_id}/tasks", get(list_tasks))
        .route("/api/v1/sessions/{session_id}/tasks/{item}", get(get_task))
        .route(
            "/api/v1/sessions/{session_id}/tasks/{item}",
            post(task_action),
        )
}
