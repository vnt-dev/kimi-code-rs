use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/workspaceFs.ts.
pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/sessions/{session_id}/fs/{*path}",
    "/api/v1/sessions/{session_id}/fs/{*}",
    CoreOperation::ReadSessionFile,
)];
