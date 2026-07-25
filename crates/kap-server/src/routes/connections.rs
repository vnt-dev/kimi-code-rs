use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use kimi_code_protocol::{
    Connection, ConnectionsListResponse, err_envelope, ok_envelope, parse_iso_date_time,
};

use crate::web::{AppState, middleware::RequestId};

// Original: packages/kap-server/src/routes/connections.ts.
async fn list_connections(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let mut connections = Vec::new();
    for connection in state.connection_registry.values() {
        let connected_at = match parse_iso_date_time(connection.connected_at()) {
            Ok(connected_at) => connected_at,
            Err(error) => {
                return Json(err_envelope(50_000, error.to_string(), request_id.0, None))
                    .into_response();
            }
        };
        connections.push(Connection {
            id: connection.id().to_owned(),
            connected_at,
            remote_address: connection.remote_address().map(str::to_owned),
            user_agent: connection.user_agent().map(str::to_owned),
            has_client_hello: connection.has_client_hello(),
            subscriptions: connection.subscription_session_ids(),
        });
    }
    connections.sort_by(|left, right| left.connected_at.cmp(&right.connected_at));
    Json(ok_envelope(
        ConnectionsListResponse { connections },
        request_id.0,
    ))
    .into_response()
}

pub fn register(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route("/api/v1/connections", get(list_connections))
}
