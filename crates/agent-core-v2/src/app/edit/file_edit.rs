//! Async file-edit service contract.
//!
//! Original: `packages/agent-core-v2/src/app/edit/fileEdit.ts`.
use crate::_base::di::instantiation::ServiceIdentifier;
use async_trait::async_trait;
use std::{ops::Deref, sync::Arc};
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEditInput {
    pub path: String,
    pub display_path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileEditResult {
    Ok { count: usize },
    Err { error: String },
}
#[async_trait]
pub trait FileEditServiceContract: Send + Sync {
    async fn edit(&self, input: FileEditInput) -> FileEditResult;
}
#[derive(Clone)]
pub struct FileEditServiceHandle(pub Arc<dyn FileEditServiceContract>);
impl Deref for FileEditServiceHandle {
    type Target = dyn FileEditServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const FILE_EDIT_SERVICE_ID: ServiceIdentifier<FileEditServiceHandle> =
    ServiceIdentifier::new("fileEditService");
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifier_matches_source() {
        assert_eq!(FILE_EDIT_SERVICE_ID.to_string(), "fileEditService");
    }
}
