//! Workspace catalog persistence boundary and v1-compatible document shapes.
//!
//! Original: `packages/agent-core-v2/src/app/workspaceRegistry/workspacePersistence.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::_base::di::instantiation::ServiceIdentifier;

use super::contract::Workspace;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PersistedWorkspaceEntry {
    pub root: String,
    pub name: String,
    pub created_at: String,
    pub last_opened_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PersistedWorkspaceFile {
    pub version: i64,
    pub workspaces: IndexMap<String, PersistedWorkspaceEntry>,
    pub deleted_workspace_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceCatalog {
    pub workspaces: Vec<Workspace>,
    pub deleted_ids: Vec<String>,
}

pub type WorkspacePersistenceError = Box<dyn Error + Send + Sync>;
pub type WorkspacePersistenceResult<T> = Result<T, WorkspacePersistenceError>;

#[async_trait]
pub trait WorkspacePersistenceContract: Send + Sync {
    async fn load(&self) -> WorkspacePersistenceResult<Option<WorkspaceCatalog>>;
    async fn save(&self, catalog: &WorkspaceCatalog) -> WorkspacePersistenceResult<()>;
}

#[derive(Clone)]
pub struct WorkspacePersistenceHandle(pub Arc<dyn WorkspacePersistenceContract>);

impl Deref for WorkspacePersistenceHandle {
    type Target = dyn WorkspacePersistenceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const WORKSPACE_PERSISTENCE_SERVICE_ID: ServiceIdentifier<WorkspacePersistenceHandle> =
    ServiceIdentifier::new("workspacePersistence");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn persisted_document_shape_and_service_id_match_v1() {
        let file = PersistedWorkspaceFile {
            version: 1,
            workspaces: IndexMap::from([(
                "wd_repo_hash".into(),
                PersistedWorkspaceEntry {
                    root: "/repo".into(),
                    name: "repo".into(),
                    created_at: "1970-01-01T00:00:00.000Z".into(),
                    last_opened_at: "1970-01-01T00:00:01.000Z".into(),
                },
            )]),
            deleted_workspace_ids: vec!["wd_old_hash".into()],
        };
        assert_eq!(
            serde_json::to_value(file).unwrap(),
            json!({
                "version": 1,
                "workspaces": {
                    "wd_repo_hash": {
                        "root": "/repo",
                        "name": "repo",
                        "created_at": "1970-01-01T00:00:00.000Z",
                        "last_opened_at": "1970-01-01T00:00:01.000Z"
                    }
                },
                "deleted_workspace_ids": ["wd_old_hash"]
            })
        );
        assert_eq!(
            WORKSPACE_PERSISTENCE_SERVICE_ID.to_string(),
            "workspacePersistence"
        );
    }
}
