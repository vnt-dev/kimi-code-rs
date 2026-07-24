use axum::extract::Extension;
use axum::routing::get;
use axum::{Json, Router};
use kimi_code_protocol::{Envelope, ok_envelope};
use serde::Serialize;

use crate::web::{AppState, middleware::RequestId};

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub ok: bool,
}

// Original: routes/registerApiV1Routes.ts, registerHealthRoute().
async fn get_health(Extension(request_id): Extension<RequestId>) -> Json<Envelope<HealthResponse>> {
    Json(ok_envelope(HealthResponse { ok: true }, request_id.0))
}

pub fn register(router: Router<std::sync::Arc<AppState>>) -> Router<std::sync::Arc<AppState>> {
    router.route("/api/v1/healthz", get(get_health))
}
