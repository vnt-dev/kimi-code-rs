use std::sync::Arc;

use axum::Router;

use super::{
    approvals, auth, config, connections, debug, files, fs, gui_store, health, messages, meta,
    model_catalog, oauth, prompts, questions, session_export, sessions, shutdown, skills, snapshot,
    tasks, terminals, tools, transcript, workspace_fs, workspaces,
};
use crate::web::AppState;

// Original: routes/registerApiV1Routes.ts, registerApiV1Routes().
pub fn register(router: Router<Arc<AppState>>, state: &AppState) -> Router<Arc<AppState>> {
    let router = health::register(router);
    let router = meta::register(router);
    let router = auth::register(router);
    let router = oauth::register(router);
    let router = config::register(router);
    let router = model_catalog::register(router);
    let router = sessions::register(router);
    let router = session_export::register(router);
    let router = skills::register(router);
    let router = messages::register(router);
    let router = tasks::register(router);
    let router = approvals::register(router);
    let router = questions::register(router);
    let router = prompts::register(router);
    let router = workspaces::register(router);
    let router = files::register(router);
    let router = fs::register(router);
    let router = workspace_fs::register(router);
    let router = gui_store::register(router);
    let router = tools::register(router);
    let router = if state.enable_terminals {
        terminals::register(router)
    } else {
        router
    };
    let router = connections::register(router);
    let router = snapshot::register(router);
    let router = transcript::register(router);
    let router = shutdown::register(router, state.enable_shutdown);
    if state.debug_endpoints {
        debug::register(router)
    } else {
        router
    }
}
