//! Local host-filesystem primitives.
//!
//! Original: `packages/agent-core-v2/src/os/interface/hostFileSystem.ts`.

use std::{ops::Deref, path::Path, pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_util::Stream;

use crate::_base::{
    di::instantiation::ServiceIdentifier,
    exec_env::decode_text::{TextDecodeErrors, TextEncoding},
};

use super::host_fs_errors::HostFsError;

#[derive(Clone, Debug, PartialEq)]
pub struct HostFileStat {
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symbolic_link: bool,
    pub size: u64,
    pub modified_millis: Option<i64>,
    pub inode: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDirEntry {
    pub name: String,
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symbolic_link: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadTextOptions {
    pub encoding: TextEncoding,
    pub errors: TextDecodeErrors,
}

impl Default for ReadTextOptions {
    fn default() -> Self {
        Self {
            encoding: TextEncoding::Utf8,
            errors: TextDecodeErrors::Strict,
        }
    }
}

pub type HostLineStream = Pin<Box<dyn Stream<Item = Result<String, HostFsError>> + Send + 'static>>;

#[async_trait]
pub trait HostFileSystemService: Send + Sync {
    async fn read_text(
        &self,
        path: &Path,
        options: Option<ReadTextOptions>,
    ) -> Result<String, HostFsError>;
    async fn write_text(&self, path: &Path, data: &str) -> Result<(), HostFsError>;
    async fn append_text(&self, path: &Path, data: &str) -> Result<(), HostFsError>;
    async fn read_bytes(&self, path: &Path, count: Option<usize>) -> Result<Vec<u8>, HostFsError>;
    async fn write_bytes(&self, path: &Path, data: &[u8]) -> Result<(), HostFsError>;
    fn read_lines(&self, path: &Path, options: Option<ReadTextOptions>) -> HostLineStream;
    async fn create_exclusive(&self, path: &Path, data: &[u8]) -> Result<bool, HostFsError>;
    async fn stat(&self, path: &Path) -> Result<HostFileStat, HostFsError>;
    async fn lstat(&self, path: &Path) -> Result<HostFileStat, HostFsError>;
    async fn read_dir(&self, path: &Path) -> Result<Vec<HostDirEntry>, HostFsError>;
    async fn create_dir(&self, path: &Path, recursive: bool) -> Result<(), HostFsError>;
    async fn remove(&self, path: &Path) -> Result<(), HostFsError>;
    async fn real_path(&self, path: &Path) -> Result<String, HostFsError>;
}

#[derive(Clone)]
pub struct HostFileSystemServiceHandle(pub Arc<dyn HostFileSystemService>);

impl Deref for HostFileSystemServiceHandle {
    type Target = dyn HostFileSystemService;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const HOST_FILE_SYSTEM_SERVICE_ID: ServiceIdentifier<HostFileSystemServiceHandle> =
    ServiceIdentifier::new("hostFileSystem");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_service_identity_match_source() {
        assert_eq!(ReadTextOptions::default().encoding, TextEncoding::Utf8);
        assert_eq!(ReadTextOptions::default().errors, TextDecodeErrors::Strict);
        assert_eq!(HOST_FILE_SYSTEM_SERVICE_ID.to_string(), "hostFileSystem");
    }
}
