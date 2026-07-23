//! Process-wide known-workspace catalog.

pub mod contract;
pub mod errors;
pub mod file_persistence;
pub mod persistence;

pub use contract::{
    WORKSPACE_REGISTRY_SERVICE_ID, Workspace, WorkspaceRegistryContract, WorkspaceRegistryError,
    WorkspaceRegistryHandle, WorkspaceRegistryResult, WorkspaceUpdate,
};
pub use errors::{WORKSPACE_ERRORS, WORKSPACE_NOT_FOUND, ensure_workspace_errors_registered};
pub use file_persistence::{FileWorkspacePersistence, register_workspace_persistence};
pub use persistence::{
    PersistedWorkspaceEntry, PersistedWorkspaceFile, WORKSPACE_PERSISTENCE_SERVICE_ID,
    WorkspaceCatalog, WorkspacePersistenceContract, WorkspacePersistenceHandle,
    WorkspacePersistenceResult,
};
