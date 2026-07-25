use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::get;

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/fs:browse",
        "/api/v1/fs:browse",
        CoreOperation::BrowseFileSystem,
    ),
    route(
        "GET",
        "/api/v1/fs:home",
        "/api/v1/fs:home",
        CoreOperation::GetFileSystemHome,
    ),
];

async fn browse_file_system(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::BrowseFileSystem, request).await
}

async fn get_file_system_home(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::GetFileSystemHome, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/fs:browse", get(browse_file_system))
        .route("/api/v1/fs:home", get(get_file_system_home))
}
