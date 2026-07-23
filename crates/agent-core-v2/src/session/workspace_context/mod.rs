//! Session workspace root and path access.

pub mod contract;
pub mod service;

pub use contract::{
    PathAccessError, PathAccessOperation, SESSION_WORKSPACE_CONTEXT_ID,
    SessionWorkspaceContextContract, SessionWorkspaceContextHandle,
};
pub use service::{SessionWorkspaceContextService, register_session_workspace_context};
