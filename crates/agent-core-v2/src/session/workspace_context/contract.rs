//! Session workspace root and lexical path-access contract.
//!
//! Original: `packages/agent-core-v2/src/session/workspaceContext/workspaceContext.ts`.

use std::{fmt, ops::Deref, path::PathBuf, sync::Arc};

use crate::_base::di::instantiation::ServiceIdentifier;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathAccessOperation {
    Read,
    Write,
    Execute,
}

impl fmt::Display for PathAccessOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Execute => "execute",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Path outside workspace ({operation}): {path}")]
pub struct PathAccessError {
    pub path: PathBuf,
    pub operation: PathAccessOperation,
}

pub trait SessionWorkspaceContextContract: Send + Sync {
    fn work_dir(&self) -> PathBuf;
    fn additional_dirs(&self) -> Vec<PathBuf>;
    fn set_work_dir(&self, work_dir: &str) -> std::io::Result<()>;
    fn set_additional_dirs(&self, dirs: &[String]) -> std::io::Result<()>;
    fn resolve(&self, relative: &str) -> PathBuf;
    fn is_within(&self, absolute_path: &str) -> bool;
    fn assert_allowed(
        &self,
        absolute_path: &str,
        operation: PathAccessOperation,
    ) -> Result<PathBuf, PathAccessError>;
    fn add_additional_dir(&self, dir: &str) -> std::io::Result<()>;
    fn remove_additional_dir(&self, dir: &str) -> std::io::Result<()>;
}

#[derive(Clone)]
pub struct SessionWorkspaceContextHandle(pub Arc<dyn SessionWorkspaceContextContract>);

impl Deref for SessionWorkspaceContextHandle {
    type Target = dyn SessionWorkspaceContextContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_WORKSPACE_CONTEXT_ID: ServiceIdentifier<SessionWorkspaceContextHandle> =
    ServiceIdentifier::new("sessionWorkspaceContext");
