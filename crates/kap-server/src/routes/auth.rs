use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/auth.ts, registerAuthRoute().
pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/auth",
    "/api/v1/auth",
    CoreOperation::GetAuth,
)];
