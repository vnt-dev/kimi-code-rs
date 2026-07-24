use std::sync::Arc;

use axum::Router;
use axum::response::Response;
use axum::routing::{delete, get, post};

use super::{CoreRouteRequest, RouteSpec, dispatch_core, route};
use crate::web::{AppState, CoreOperation};

pub const ROUTES: &[RouteSpec] = &[
    route(
        "POST",
        "/api/v1/files",
        "/api/v1/files",
        CoreOperation::UploadFile,
    ),
    route(
        "GET",
        "/api/v1/files/{file_id}",
        "/api/v1/files/{file_id}",
        CoreOperation::DownloadFile,
    ),
    route(
        "DELETE",
        "/api/v1/files/{file_id}",
        "/api/v1/files/{file_id}",
        CoreOperation::DeleteFile,
    ),
];

async fn upload_file(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::UploadFile, request).await
}

async fn download_file(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DownloadFile, request).await
}

async fn delete_file(request: CoreRouteRequest) -> Response {
    dispatch_core(CoreOperation::DeleteFile, request).await
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/api/v1/files", post(upload_file))
        .route("/api/v1/files/{file_id}", get(download_file))
        .route("/api/v1/files/{file_id}", delete(delete_file))
}
