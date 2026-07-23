//! Seeded per-session facts.
//!
//! Original: `packages/agent-core-v2/src/session/sessionContext/sessionContext.ts`.

use std::sync::Arc;

use crate::_base::di::{instantiation::ServiceIdentifier, service_collection::ServiceCollection};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContext {
    pub session_id: String,
    pub workspace_id: String,
    pub session_dir: String,
    pub meta_scope: String,
    pub cwd: String,
    session_scope: String,
}

impl SessionContext {
    // Original: ISessionContext.scope(). Empty subkeys deliberately resolve to
    // the base scope rather than creating a trailing slash.
    pub fn scope(&self, sub_key: Option<&str>) -> String {
        match sub_key {
            None | Some("") => self.session_scope.clone(),
            Some(sub_key) => format!("{}/{sub_key}", self.session_scope),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContextInput {
    pub session_id: String,
    pub workspace_id: String,
    pub session_dir: String,
    pub session_scope: String,
    pub cwd: String,
    pub meta_scope: Option<String>,
}

// Original: makeSessionContext().
pub fn make_session_context(input: SessionContextInput) -> SessionContext {
    SessionContext {
        session_id: input.session_id,
        workspace_id: input.workspace_id,
        session_dir: input.session_dir,
        meta_scope: input
            .meta_scope
            .unwrap_or_else(|| input.session_scope.clone()),
        cwd: input.cwd,
        session_scope: input.session_scope,
    }
}

pub const SESSION_CONTEXT_ID: ServiceIdentifier<SessionContext> =
    ServiceIdentifier::new("sessionContext");

// Original: sessionContextSeed(). `ServiceCollection` is the Rust scope-seed
// representation consumed by `ScopeOptions.extra`.
pub fn session_context_seed(context: SessionContext) -> ServiceCollection {
    let mut seed = ServiceCollection::new();
    seed.set_instance(SESSION_CONTEXT_ID, Arc::new(context));
    seed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> SessionContextInput {
        SessionContextInput {
            session_id: "session-1".into(),
            workspace_id: "wd-repo".into(),
            session_dir: "/home/sessions/wd-repo/session-1".into(),
            session_scope: "sessions/wd-repo/session-1".into(),
            cwd: "/repo".into(),
            meta_scope: None,
        }
    }

    #[test]
    fn constructor_defaults_meta_scope_and_joins_child_scopes() {
        let context = make_session_context(input());
        assert_eq!(context.meta_scope, "sessions/wd-repo/session-1");
        assert_eq!(context.scope(None), "sessions/wd-repo/session-1");
        assert_eq!(context.scope(Some("")), "sessions/wd-repo/session-1");
        assert_eq!(
            context.scope(Some("agents/main/cron")),
            "sessions/wd-repo/session-1/agents/main/cron"
        );
    }

    #[test]
    fn explicit_meta_scope_and_di_seed_preserve_identity() {
        let mut input = input();
        input.meta_scope = Some("custom/meta".into());
        let context = make_session_context(input);
        let seed = session_context_seed(context.clone());
        assert_eq!(
            seed.get(SESSION_CONTEXT_ID).unwrap().as_deref(),
            Some(&context)
        );
        assert_eq!(SESSION_CONTEXT_ID.to_string(), "sessionContext");
    }
}
