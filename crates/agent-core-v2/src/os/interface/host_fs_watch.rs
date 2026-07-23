//! Local host-filesystem change notifications.
//!
//! Original: `packages/agent-core-v2/src/os/interface/hostFsWatch.ts`.

use std::{path::Path, sync::Arc};

use crate::_base::{
    di::{instantiation::ServiceIdentifier, lifecycle::Disposable},
    event::Event,
};

use super::host_fs_errors::HostFsError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostFsChangeKind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostFsChangeAction {
    Created,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostFsChange {
    pub path: String,
    pub action: HostFsChangeAction,
    pub kind: HostFsChangeKind,
}

pub type IgnoredPath = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

#[derive(Clone, Default)]
pub struct HostFsWatchOptions {
    pub recursive: Option<bool>,
    pub ignored: Option<IgnoredPath>,
}

pub trait HostFsWatchHandle: Disposable {
    fn on_did_change(&self) -> Event<HostFsChange>;
}

pub trait HostFsWatchService: Send + Sync {
    fn watch(
        &self,
        path: &Path,
        options: HostFsWatchOptions,
    ) -> Result<Arc<dyn HostFsWatchHandle>, HostFsError>;
}

pub const HOST_FS_WATCH_SERVICE_ID: ServiceIdentifier<dyn HostFsWatchService> =
    ServiceIdentifier::new("hostFsWatchService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_and_defaults_match_source() {
        assert_eq!(HOST_FS_WATCH_SERVICE_ID.to_string(), "hostFsWatchService");
        let options = HostFsWatchOptions::default();
        assert_eq!(options.recursive, None);
        assert!(options.ignored.is_none());
    }
}
