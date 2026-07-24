use std::sync::Arc;

use axum::Router;

use super::{connections, gui_store, health, meta, register_core_routes, shutdown};
use crate::web::AppState;

// Original: routes/registerApiV1Routes.ts, registerApiV1Routes().
pub fn register(router: Router<Arc<AppState>>, state: &AppState) -> Router<Arc<AppState>> {
    let router = health::register(router);
    let router = meta::register(router);
    let router = gui_store::register(router);
    let router = connections::register(router);
    let router = shutdown::register(router, state.enable_shutdown);
    register_core_routes(router, state)
}
