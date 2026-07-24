use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/snapshot.ts, registerSnapshotRoutes().
pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/sessions/{session_id}/snapshot",
    "/api/v1/sessions/{session_id}/snapshot",
    CoreOperation::GetSnapshot,
)];
