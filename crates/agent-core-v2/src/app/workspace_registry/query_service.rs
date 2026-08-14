//! Workspace-centric read queries backed by the persisted session index.
//!
//! Original:
//! `packages/agent-core-v2/src/app/workspaceRegistry/workspaceQueryService.ts`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::session_index::{
        SESSION_INDEX_SERVICE_ID, SessionIndexHandle, SessionListQuery, SessionSummary,
    },
};

use super::query_contract::{
    RECENT_SESSIONS_LIMIT, WORKSPACE_QUERY_SERVICE_ID, WorkspaceQueryContract,
    WorkspaceQueryHandle, WorkspaceQueryResult,
};

pub struct WorkspaceQueryService {
    index: SessionIndexHandle,
}

impl WorkspaceQueryService {
    pub fn new(index: SessionIndexHandle) -> Self {
        Self { index }
    }
}

#[async_trait]
impl WorkspaceQueryContract for WorkspaceQueryService {
    // Original: WorkspaceQueryService.listRecentSessions().
    async fn list_recent_sessions(
        &self,
        workspace_id: &str,
    ) -> WorkspaceQueryResult<Vec<SessionSummary>> {
        let page = self
            .index
            .list(SessionListQuery {
                workspace_ids: Some(vec![workspace_id.to_owned()]),
                limit: Some(RECENT_SESSIONS_LIMIT),
                ..SessionListQuery::default()
            })
            .await?;
        Ok(page.items)
    }
}

pub fn register_workspace_query_service() {
    register_scoped_service(
        LifecycleScope::App,
        WORKSPACE_QUERY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let index = accessor.get(SESSION_INDEX_SERVICE_ID)?;
            let service: Arc<dyn WorkspaceQueryContract> =
                Arc::new(WorkspaceQueryService::new((*index).clone()));
            Ok(WorkspaceQueryHandle(service))
        }),
        InstantiationType::Eager,
        "workspaceRegistry",
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use crate::{
        app::session_index::{SessionIndexContract, SessionIndexResult},
        persistence::interface::query_store::Page,
    };

    use super::*;

    #[derive(Default)]
    struct StubSessionIndex {
        last_query: Mutex<Option<SessionListQuery>>,
        items: Mutex<Vec<SessionSummary>>,
    }

    #[async_trait]
    impl SessionIndexContract for StubSessionIndex {
        async fn list(&self, query: SessionListQuery) -> SessionIndexResult<Page<SessionSummary>> {
            *self.last_query.lock() = Some(query);
            Ok(Page {
                items: self.items.lock().clone(),
                next_cursor: Some("ignored".into()),
            })
        }

        async fn get(&self, _id: &str) -> SessionIndexResult<Option<SessionSummary>> {
            Ok(None)
        }

        async fn remove(&self, _id: &str) -> SessionIndexResult<()> {
            Ok(())
        }

        async fn count_active(&self, _workspace_ids: &[String]) -> SessionIndexResult<usize> {
            Ok(0)
        }
    }

    fn summary(id: &str, workspace_id: &str, updated_at: i64) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            workspace_id: workspace_id.into(),
            cwd: None,
            title: None,
            last_prompt: None,
            created_at: updated_at - 1,
            updated_at,
            archived: false,
            custom: None,
        }
    }

    #[tokio::test]
    async fn delegates_with_the_workspace_filter_and_recent_limit() {
        let index = Arc::new(StubSessionIndex::default());
        let service = WorkspaceQueryService::new(SessionIndexHandle(index.clone()));

        assert!(
            service
                .list_recent_sessions("wd_abc")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            *index.last_query.lock(),
            Some(SessionListQuery {
                workspace_ids: Some(vec!["wd_abc".into()]),
                limit: Some(RECENT_SESSIONS_LIMIT),
                ..SessionListQuery::default()
            })
        );
    }

    #[tokio::test]
    async fn returns_only_the_session_index_items() {
        let index = Arc::new(StubSessionIndex::default());
        let expected = vec![summary("s2", "wd_abc", 200), summary("s1", "wd_abc", 100)];
        *index.items.lock() = expected.clone();
        let service = WorkspaceQueryService::new(SessionIndexHandle(index));

        assert_eq!(
            service.list_recent_sessions("wd_abc").await.unwrap(),
            expected
        );
    }
}
