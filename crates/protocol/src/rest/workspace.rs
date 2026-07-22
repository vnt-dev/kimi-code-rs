use serde::{Deserialize, Serialize};

use crate::validation::literal_true;
use crate::{Workspace, WorkspaceCreate, WorkspaceId, WorkspaceUpdate};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListWorkspacesResponse {
    pub items: Vec<Workspace>,
}

pub type CreateWorkspaceRequest = WorkspaceCreate;
pub type CreateWorkspaceResponse = Workspace;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceIdParam {
    pub workspace_id: WorkspaceId,
}

pub type UpdateWorkspaceRequest = WorkspaceUpdate;
pub type UpdateWorkspaceResponse = Workspace;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteWorkspaceResponse {
    #[serde(deserialize_with = "literal_true")]
    pub deleted: bool,
}
