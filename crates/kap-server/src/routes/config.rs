use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/config.ts, registerConfigRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/config",
        "/api/v1/config",
        CoreOperation::GetConfig,
    ),
    route(
        "POST",
        "/api/v1/config",
        "/api/v1/config",
        CoreOperation::UpdateConfig,
    ),
];
