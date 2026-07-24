use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/approvals.ts, registerApprovalsRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/approvals",
        "/api/v1/sessions/{session_id}/approvals",
        CoreOperation::ListApprovals,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/approvals/{approval_id}",
        "/api/v1/sessions/{session_id}/approvals/{approval_id}",
        CoreOperation::ResolveApproval,
    ),
];
