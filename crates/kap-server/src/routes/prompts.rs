use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/prompts.ts, registerPromptsRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/prompts",
        "/api/v1/sessions/{session_id}/prompts",
        CoreOperation::ListPrompts,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/prompts",
        "/api/v1/sessions/{session_id}/prompts",
        CoreOperation::SubmitPrompt,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/prompts:steer",
        "/api/v1/sessions/{session_id}/prompts:steer",
        CoreOperation::SteerPrompt,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/prompts/{tail}",
        "/api/v1/sessions/{session_id}/prompts/{tail}",
        CoreOperation::PromptAction,
    ),
];
