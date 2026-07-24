use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/transport/registerDebugRoutes.ts.
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/debug/channels",
        "/api/v1/debug/channels",
        CoreOperation::DebugChannels,
    ),
    route(
        "GET",
        "/api/v1/debug/{service}/{method}",
        "/api/v1/debug/{service}/{method}",
        CoreOperation::DebugGlobalGet,
    ),
    route(
        "POST",
        "/api/v1/debug/{service}/{method}",
        "/api/v1/debug/{service}/{method}",
        CoreOperation::DebugGlobalPost,
    ),
    route(
        "GET",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        CoreOperation::DebugSessionGet,
    ),
    route(
        "POST",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/{service}/{method}",
        CoreOperation::DebugSessionPost,
    ),
    route(
        "GET",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        CoreOperation::DebugAgentGet,
    ),
    route(
        "POST",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        "/api/v1/debug/session/{session_id}/agent/{agent_id}/{service}/{method}",
        CoreOperation::DebugAgentPost,
    ),
];
