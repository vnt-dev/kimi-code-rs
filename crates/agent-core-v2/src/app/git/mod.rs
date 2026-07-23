//! Local Git repository contracts and output parsing.

pub mod contract;
pub mod parsers;
pub mod service;

pub use contract::{
    FS_GIT_SERVICE_ID, FsDiffRequest, FsDiffResponse, FsGitStatus, FsGitStatusRequest,
    FsGitStatusResponse, FsPullRequest, FsPullRequestState, GitServiceContract, GitServiceError,
    GitServiceHandle, GitServiceResult,
};
pub use parsers::{NumstatSummary, parse_numstat, parse_porcelain, parse_pull_request};
pub use service::{GitService, register_git_service};
