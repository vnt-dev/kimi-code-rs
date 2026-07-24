use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/skills.ts, registerSkillsRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/skills",
        "/api/v1/sessions/{session_id}/skills",
        CoreOperation::ListSkills,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/skills/{tail}",
        "/api/v1/sessions/{session_id}/skills/{tail}",
        CoreOperation::SkillAction,
    ),
    route(
        "GET",
        "/api/v1/workspaces/{workspace_id}/skills",
        "/api/v1/workspaces/{workspace_id}/skills",
        CoreOperation::ListWorkspaceSkills,
    ),
];
