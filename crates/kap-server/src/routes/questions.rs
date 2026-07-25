use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/questions",
        "/api/v1/sessions/{session_id}/questions",
        CoreOperation::ListQuestions,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/questions/{tail}",
        "/api/v1/sessions/{session_id}/questions/{tail}",
        CoreOperation::QuestionAction,
    ),
];

async fn list_questions(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListQuestions, request).await
}

async fn question_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::QuestionAction, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route(
            "/api/v1/sessions/{session_id}/questions",
            get(list_questions),
        )
        .route(
            "/api/v1/sessions/{session_id}/questions/{tail}",
            post(question_action),
        )
}
