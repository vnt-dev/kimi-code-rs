use std::sync::Arc;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Extension, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kimi_code_protocol::{
    ErrorCode, OAuthLoginQuery, OAuthLoginStartRequest, OAuthLogoutRequest, err_envelope,
    ok_envelope,
};

use super::{RouteSpec, route};
use crate::web::{AppState, CoreOperation, middleware::RequestId};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::GetOauthLogin,
    ),
    route(
        "POST",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::StartOauthLogin,
    ),
    route(
        "DELETE",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::DeleteOauthLogin,
    ),
    route(
        "POST",
        "/api/v1/oauth/logout",
        "/api/v1/oauth/logout",
        CoreOperation::OauthLogout,
    ),
];

// Original: packages/kap-server/src/routes/oauth.ts, GET /oauth/login.
async fn get_oauth_login(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    query: Result<Query<OAuthLoginQuery>, QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(error) => return validation_error(error.body_text(), request_id.0),
    };
    let Some(service) = state.oauth_service.as_ref() else {
        return missing_service(request_id.0);
    };
    Json(ok_envelope(
        service.get_flow(query.provider.as_deref()),
        request_id.0,
    ))
    .into_response()
}

// Original: packages/kap-server/src/routes/oauth.ts, POST /oauth/login.
async fn start_oauth_login(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<OAuthLoginStartRequest>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return validation_error(error.body_text(), request_id.0),
    };
    let Some(service) = state.oauth_service.as_ref() else {
        return missing_service(request_id.0);
    };
    match service.start_login(body.provider.as_deref()).await {
        Ok(result) => Json(ok_envelope(result, request_id.0)).into_response(),
        Err(error) => internal_error(error.to_string(), request_id.0),
    }
}

// Original: packages/kap-server/src/routes/oauth.ts, DELETE /oauth/login.
async fn delete_oauth_login(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    query: Result<Query<OAuthLoginQuery>, QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(error) => return validation_error(error.body_text(), request_id.0),
    };
    let Some(service) = state.oauth_service.as_ref() else {
        return missing_service(request_id.0);
    };
    match service.cancel_login(query.provider.as_deref()).await {
        Ok(result) => Json(ok_envelope(result, request_id.0)).into_response(),
        Err(error) => internal_error(error.to_string(), request_id.0),
    }
}

// Original: packages/kap-server/src/routes/oauth.ts, POST /oauth/logout.
async fn oauth_logout(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<OAuthLogoutRequest>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(error) => return validation_error(error.body_text(), request_id.0),
    };
    let Some(service) = state.oauth_service.as_ref() else {
        return missing_service(request_id.0);
    };
    match service.logout(body.provider.as_deref()).await {
        Ok(result) => Json(ok_envelope(result, request_id.0)).into_response(),
        Err(error) => internal_error(error.to_string(), request_id.0),
    }
}

fn validation_error(message: String, request_id: String) -> Response {
    Json(err_envelope(
        ErrorCode::ValidationFailed,
        message,
        request_id,
        None,
    ))
    .into_response()
}

fn internal_error(message: String, request_id: String) -> Response {
    Json(err_envelope(
        ErrorCode::InternalError,
        message,
        request_id,
        None,
    ))
    .into_response()
}

fn missing_service(request_id: String) -> Response {
    // MIGRATION-TODO:
    // Remove the optional boundary after start.ts's full agent-core-v2 Scope
    // composition has a Rust counterpart.
    internal_error("OAuthService is not configured".into(), request_id)
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/oauth/login", get(get_oauth_login))
        .route("/api/v1/oauth/login", post(start_oauth_login))
        .route("/api/v1/oauth/login", delete(delete_oauth_login))
        .route("/api/v1/oauth/logout", post(oauth_logout))
}
