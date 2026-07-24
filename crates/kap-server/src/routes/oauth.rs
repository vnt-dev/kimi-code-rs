use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{delete, get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

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

async fn get_oauth_login(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetOauthLogin, request).await
}

async fn start_oauth_login(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::StartOauthLogin, request).await
}

async fn delete_oauth_login(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DeleteOauthLogin, request).await
}

async fn oauth_logout(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::OauthLogout, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/oauth/login", get(get_oauth_login))
        .route("/api/v1/oauth/login", post(start_oauth_login))
        .route("/api/v1/oauth/login", delete(delete_oauth_login))
        .route("/api/v1/oauth/logout", post(oauth_logout))
}
