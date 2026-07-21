use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackCodebaseFile {
    pub path: String,
    pub absolute_path: PathBuf,
    pub size: u64,
    pub mtime_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackCodebaseLimitReason {
    FileCount,
    TotalSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedbackCodebaseLimitExceeded {
    pub reason: FeedbackCodebaseLimitReason,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackCodebaseScanResult {
    pub root: PathBuf,
    pub files: Vec<FeedbackCodebaseFile>,
    pub fingerprint: String,
    pub used_git_ignore: bool,
    pub exceeds_limit: Option<FeedbackCodebaseLimitExceeded>,
}
