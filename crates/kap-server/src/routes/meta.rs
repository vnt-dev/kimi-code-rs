use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::routing::get;
use axum::{Json, Router};
use kimi_code_protocol::{
    BackendGeneration, Envelope, MetaCapabilities, MetaResponse, ok_envelope,
};

use crate::web::{AppState, middleware::RequestId};

// Original: packages/kap-server/src/routes/meta.ts, registerMetaRoute().
async fn get_meta(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Json<Envelope<MetaResponse>> {
    Json(ok_envelope(
        MetaResponse {
            server_version: state.server_version.clone(),
            capabilities: MetaCapabilities {
                websocket: true,
                file_upload: true,
                fs_query: true,
                mcp: true,
                tasks: true,
                terminal: true,
            },
            server_id: state.server_id.clone(),
            started_at: state.started_at.clone(),
            open_in_apps: Vec::new(),
            dangerous_bypass_auth: state.disable_auth,
            backend: Some(BackendGeneration::V2),
        },
        request_id.0,
    ))
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route("/api/v1/meta", get(get_meta))
}
