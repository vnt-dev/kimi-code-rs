//! Workspace-centric read-model query contract.
//!
//! Original: `packages/agent-core-v2/src/app/workspaceRegistry/workspaceQuery.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{_base::di::instantiation::ServiceIdentifier, app::session_index::SessionSummary};

pub const RECENT_SESSIONS_LIMIT: usize = 20;

pub type WorkspaceQueryError = Box<dyn Error + Send + Sync>;
pub type WorkspaceQueryResult<T> = Result<T, WorkspaceQueryError>;

#[async_trait]
pub trait WorkspaceQueryContract: Send + Sync {
    async fn list_recent_sessions(
        &self,
        workspace_id: &str,
    ) -> WorkspaceQueryResult<Vec<SessionSummary>>;
}

#[derive(Clone)]
pub struct WorkspaceQueryHandle(pub Arc<dyn WorkspaceQueryContract>);

impl Deref for WorkspaceQueryHandle {
    type Target = dyn WorkspaceQueryContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const WORKSPACE_QUERY_SERVICE_ID: ServiceIdentifier<WorkspaceQueryHandle> =
    ServiceIdentifier::new("workspaceQuery");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_limit_and_service_id_match_the_source_contract() {
        assert_eq!(RECENT_SESSIONS_LIMIT, 20);
        assert_eq!(WORKSPACE_QUERY_SERVICE_ID.to_string(), "workspaceQuery");
    }
}
