use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/fs.ts.
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
