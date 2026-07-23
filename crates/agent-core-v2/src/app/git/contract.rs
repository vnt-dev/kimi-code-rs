//! Local Git repository protocol and service contract.
//!
//! Original: `packages/agent-core-v2/src/app/git/git.ts`.

use std::{collections::HashSet, error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::_base::di::instantiation::ServiceIdentifier;

pub type GitServiceError = Box<dyn Error + Send + Sync>;
pub type GitServiceResult<T> = Result<T, GitServiceError>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FsGitStatus {
    Clean,
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Conflicted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FsPullRequestState {
    Open,
    Merged,
    Closed,
    Draft,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsPullRequest {
    pub number: u64,
    pub state: FsPullRequestState,
    pub url: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsGitStatusRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsGitStatusResponse {
    pub branch: String,
    pub ahead: u64,
    pub behind: u64,
    pub entries: IndexMap<String, FsGitStatus>,
    pub additions: u64,
    pub deletions: u64,
    pub pull_request: Option<FsPullRequest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsDiffRequest {
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsDiffResponse {
    pub path: String,
    pub diff: String,
    pub truncated: bool,
}

#[async_trait]
pub trait GitServiceContract: Send + Sync {
    async fn status(
        &self,
        cwd: &str,
        path_filter: Option<&HashSet<String>>,
    ) -> GitServiceResult<FsGitStatusResponse>;

    async fn diff(
        &self,
        cwd: &str,
        relative_path: &str,
        absolute_path: &str,
    ) -> GitServiceResult<FsDiffResponse>;
}

#[derive(Clone)]
pub struct GitServiceHandle(pub Arc<dyn GitServiceContract>);

impl Deref for GitServiceHandle {
    type Target = dyn GitServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const FS_GIT_SERVICE_ID: ServiceIdentifier<GitServiceHandle> =
    ServiceIdentifier::new("gitService");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn protocol_uses_the_original_wire_names() {
        let response = FsGitStatusResponse {
            branch: "main".into(),
            ahead: 1,
            behind: 2,
            entries: IndexMap::from([("src/lib.rs".into(), FsGitStatus::Modified)]),
            additions: 3,
            deletions: 4,
            pull_request: Some(FsPullRequest {
                number: 5,
                state: FsPullRequestState::Draft,
                url: "https://github.com/acme/repo/pull/5".into(),
            }),
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "branch": "main",
                "ahead": 1,
                "behind": 2,
                "entries": {"src/lib.rs": "modified"},
                "additions": 3,
                "deletions": 4,
                "pullRequest": {
                    "number": 5,
                    "state": "draft",
                    "url": "https://github.com/acme/repo/pull/5"
                }
            })
        );
        assert_eq!(FS_GIT_SERVICE_ID.to_string(), "gitService");
    }

    #[test]
    fn absent_status_paths_remain_absent_on_the_wire() {
        assert_eq!(
            serde_json::to_value(FsGitStatusRequest::default()).unwrap(),
            json!({})
        );
    }
}
