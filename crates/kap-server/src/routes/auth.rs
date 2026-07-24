use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use kimi_code_agent_core_v2::app::auth_legacy::{
    AuthSummary as CoreAuthSummary, ManagedProviderStatus as CoreManagedProviderStatus,
    ManagedProviderSummary as CoreManagedProviderSummary,
};
use kimi_code_protocol::{
    AuthSummary, ErrorCode, ManagedProviderStatus, ManagedProviderSummary, err_envelope,
    ok_envelope,
};

use super::{RouteSpec, route};
use crate::web::{AppState, CoreOperation, middleware::RequestId};

pub const ROUTES: &[RouteSpec] = &[route(
    "GET",
    "/api/v1/auth",
    "/api/v1/auth",
    CoreOperation::GetAuth,
)];

// Original:
//   packages/kap-server/src/routes/auth.ts, GET /auth handler.
//
// Rust adaptation:
//   The assembled agent-core-v2 scope is represented by an injected
//   AuthLegacyServiceHandle. The service call and HTTP response remain async,
//   and the legacy v1 summary is explicitly converted to the protocol DTO.
async fn get_auth(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(service) = state.auth_legacy_service.as_ref() else {
        // MIGRATION-TODO:
        // Original: start.ts assembles a complete agent-core-v2 Scope, from
        // which auth.ts resolves IAuthLegacyService.
        // Missing dependency: the Rust bootstrap does not yet assemble that
        // application Scope.
        // Temporary behavior: return the same internal-error envelope used by
        // the TypeScript global error handler.
        // Completion condition: inject the handle from the assembled Scope.
        return Json(err_envelope(
            ErrorCode::InternalError,
            "AuthLegacyService is not configured",
            request_id.0,
            None,
        ))
        .into_response();
    };

    match service.get().await {
        Ok(summary) => {
            Json(ok_envelope(to_protocol_auth_summary(summary), request_id.0)).into_response()
        }
        Err(error) => Json(err_envelope(
            ErrorCode::InternalError,
            error.to_string(),
            request_id.0,
            None,
        ))
        .into_response(),
    }
}

fn to_protocol_auth_summary(summary: CoreAuthSummary) -> AuthSummary {
    AuthSummary {
        ready: summary.ready,
        providers_count: summary.providers_count,
        default_model: summary.default_model,
        managed_provider: summary.managed_provider.map(
            |CoreManagedProviderSummary { name, status }| ManagedProviderSummary {
                name,
                status: match status {
                    CoreManagedProviderStatus::Authenticated => {
                        ManagedProviderStatus::Authenticated
                    }
                    CoreManagedProviderStatus::Expired => ManagedProviderStatus::Expired,
                    CoreManagedProviderStatus::Revoked => ManagedProviderStatus::Revoked,
                    CoreManagedProviderStatus::Unauthenticated => {
                        ManagedProviderStatus::Unauthenticated
                    }
                },
            },
        ),
    }
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route("/api/v1/auth", get(get_auth))
}
