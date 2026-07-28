//! Workspace-confined filesystem change-feed contract.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/fsWatch.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::_base::{
    di::{instantiation::ServiceIdentifier, lifecycle::Disposable},
    event::Event,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FsChangeKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FsChangeAction {
    Created,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsChangeEntry {
    pub path: String,
    pub change: FsChangeAction,
    pub kind: FsChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_delta: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsChangeEvent {
    pub changes: Vec<FsChangeEntry>,
    pub coalesced_window_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

pub type SessionFsWatchError = Box<dyn Error + Send + Sync>;

pub trait SessionFsWatchServiceContract: Disposable + Send + Sync {
    fn set_watched_paths(&self, paths: &[String]) -> Result<(), SessionFsWatchError>;
    fn watched_paths(&self) -> Vec<String>;
    fn on_did_change_files(&self) -> Event<FsChangeEvent>;
}

#[derive(Clone)]
pub struct SessionFsWatchServiceHandle(pub Arc<dyn SessionFsWatchServiceContract>);

impl Deref for SessionFsWatchServiceHandle {
    type Target = dyn SessionFsWatchServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for SessionFsWatchServiceHandle {
    fn dispose(&self) -> crate::_base::di::lifecycle::DisposeResult {
        self.0.dispose()
    }
}

pub const SESSION_FS_WATCH_SERVICE_ID: ServiceIdentifier<SessionFsWatchServiceHandle> =
    ServiceIdentifier::new("sessionFsWatchService");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn change_wire_shape_and_service_identity_match_source() {
        assert_eq!(
            serde_json::to_value(FsChangeEvent {
                changes: vec![FsChangeEntry {
                    path: "src/lib.rs".into(),
                    change: FsChangeAction::Modified,
                    kind: FsChangeKind::File,
                    size_delta: None,
                    etag: None,
                }],
                coalesced_window_ms: 200,
                truncated: None,
                count: None,
            })
            .unwrap(),
            json!({
                "changes":[{
                    "path":"src/lib.rs",
                    "change":"modified",
                    "kind":"file"
                }],
                "coalesced_window_ms":200
            })
        );
        assert_eq!(
            SESSION_FS_WATCH_SERVICE_ID.to_string(),
            "sessionFsWatchService"
        );
    }
}
