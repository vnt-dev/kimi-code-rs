//! Process-wide known-workspace catalog.

pub mod contract;
pub mod errors;
pub mod file_persistence;
pub mod persistence;
pub mod query_contract;
pub mod query_service;
pub mod service;

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
pub use query_contract::{
    RECENT_SESSIONS_LIMIT, WORKSPACE_QUERY_SERVICE_ID, WorkspaceQueryContract, WorkspaceQueryError,
    WorkspaceQueryHandle, WorkspaceQueryResult,
};
pub use query_service::{WorkspaceQueryService, register_workspace_query_service};
pub use service::{WorkspaceRegistryService, register_workspace_registry_service};
