use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/prompts",
        "/api/v1/sessions/{session_id}/prompts",
        CoreOperation::ListPrompts,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/prompts",
        "/api/v1/sessions/{session_id}/prompts",
        CoreOperation::SubmitPrompt,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/prompts:steer",
        "/api/v1/sessions/{session_id}/prompts:steer",
        CoreOperation::SteerPrompt,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/prompts/{tail}",
        "/api/v1/sessions/{session_id}/prompts/{tail}",
        CoreOperation::PromptAction,
    ),
];

async fn list_prompts(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::ListPrompts, request).await
}

async fn submit_prompt(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::SubmitPrompt, request).await
}

async fn steer_prompt(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::SteerPrompt, request).await
}

async fn prompt_action(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::PromptAction, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/sessions/{session_id}/prompts", get(list_prompts))
        .route("/api/v1/sessions/{session_id}/prompts", post(submit_prompt))
        .route(
            "/api/v1/sessions/{session_id}/prompts:steer",
            post(steer_prompt),
        )
        .route(
            "/api/v1/sessions/{session_id}/prompts/{tail}",
            post(prompt_action),
        )
}
