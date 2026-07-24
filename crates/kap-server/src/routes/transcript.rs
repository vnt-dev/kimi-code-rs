use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/transcript.ts, registerTranscriptRoutes().
pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/sessions/{session_id}/transcript",
    "/api/v1/sessions/{session_id}/transcript",
    CoreOperation::GetTranscript,
)];
