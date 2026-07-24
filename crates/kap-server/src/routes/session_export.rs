use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/sessionExport.ts, registerSessionExportRoute().
pub const ROUTES: &[RouteSpec] = &[route(
    "POST",
    "/api/v1/sessions/{session_id}/export",
    "/api/v1/sessions/{session_id}/export",
    CoreOperation::ExportSession,
)];
