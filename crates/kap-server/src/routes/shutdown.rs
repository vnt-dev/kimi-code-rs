use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::routing::post;
use axum::{Json, Router};
use kimi_code_protocol::{Envelope, ok_envelope};
use serde_json::Value;

use crate::web::{AppState, middleware::RequestId};

// Original: packages/kap-server/src/routes/shutdown.ts.
async fn shutdown(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Json<Envelope<Value>> {
    let _ = state.shutdown.send(true);
    Json(ok_envelope(Value::Object(Default::default()), request_id.0))
}

pub fn register(router: Router<Arc<AppState>>, enabled: bool) -> Router<Arc<AppState>> {
    if enabled {
        router.route("/api/v1/shutdown", post(shutdown))
    } else {
        router
    }
}
