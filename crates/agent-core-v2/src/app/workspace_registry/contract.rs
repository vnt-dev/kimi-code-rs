//! Known-workspace catalog model and service contract.
//!
//! Original: `packages/agent-core-v2/src/app/workspaceRegistry/workspaceRegistry.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::di::instantiation::ServiceIdentifier;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    pub id: String,
    pub root: String,
    pub name: String,
    pub created_at_millis: i64,
    pub last_opened_at_millis: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceUpdate {
    pub name: Option<String>,
}

pub type WorkspaceRegistryError = Box<dyn Error + Send + Sync>;
pub type WorkspaceRegistryResult<T> = Result<T, WorkspaceRegistryError>;

#[async_trait]
pub trait WorkspaceRegistryContract: Send + Sync {
    async fn list(&self) -> WorkspaceRegistryResult<Vec<Workspace>>;
    async fn get(&self, id: &str) -> WorkspaceRegistryResult<Option<Workspace>>;
    async fn resolve_alias_ids(&self, id: &str) -> WorkspaceRegistryResult<Vec<String>>;
    async fn create_or_touch(
        &self,
        root: &str,
        name: Option<&str>,
    ) -> WorkspaceRegistryResult<Workspace>;
    async fn update(
        &self,
        id: &str,
        patch: WorkspaceUpdate,
    ) -> WorkspaceRegistryResult<Option<Workspace>>;
    async fn delete(&self, id: &str) -> WorkspaceRegistryResult<()>;
}

#[derive(Clone)]
pub struct WorkspaceRegistryHandle(pub Arc<dyn WorkspaceRegistryContract>);

impl Deref for WorkspaceRegistryHandle {
    type Target = dyn WorkspaceRegistryContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const WORKSPACE_REGISTRY_SERVICE_ID: ServiceIdentifier<WorkspaceRegistryHandle> =
    ServiceIdentifier::new("workspaceRegistry");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_model_keeps_epoch_milliseconds_separate_from_rest_model() {
        let workspace = Workspace {
            id: "wd_repo_hash".into(),
            root: "/repo".into(),
            name: "repo".into(),
            created_at_millis: -1,
            last_opened_at_millis: 2,
        };
        assert_eq!(workspace.created_at_millis, -1);
        assert_eq!(workspace.last_opened_at_millis, 2);
        assert_eq!(
            WORKSPACE_REGISTRY_SERVICE_ID.to_string(),
            "workspaceRegistry"
        );
    }
}
