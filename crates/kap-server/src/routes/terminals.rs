use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/terminals.ts, registerTerminalsRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/terminals",
        "/api/v1/sessions/{session_id}/terminals",
        CoreOperation::ListTerminals,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/terminals",
        "/api/v1/sessions/{session_id}/terminals",
        CoreOperation::CreateTerminal,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/terminals/{item}",
        "/api/v1/sessions/{session_id}/terminals/{terminal_id}",
        CoreOperation::GetTerminal,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/terminals/{item}",
        "/api/v1/sessions/{session_id}/terminals/{tail}",
        CoreOperation::TerminalAction,
    ),
];
