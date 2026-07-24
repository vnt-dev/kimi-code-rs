use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/oauth.ts, registerOAuthRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::GetOauthLogin,
    ),
    route(
        "POST",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::StartOauthLogin,
    ),
    route(
        "DELETE",
        "/api/v1/oauth/login",
        "/api/v1/oauth/login",
        CoreOperation::DeleteOauthLogin,
    ),
    route(
        "POST",
        "/api/v1/oauth/logout",
        "/api/v1/oauth/logout",
        CoreOperation::OauthLogout,
    ),
];
