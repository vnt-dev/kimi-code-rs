use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/files.ts, registerFilesRoutes().
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
