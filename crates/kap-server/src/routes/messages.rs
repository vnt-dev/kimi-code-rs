use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/messages.ts, registerMessagesRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/messages",
        "/api/v1/sessions/{session_id}/messages",
        CoreOperation::ListMessages,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/messages/{message_id}",
        "/api/v1/sessions/{session_id}/messages/{message_id}",
        CoreOperation::GetMessage,
    ),
];
