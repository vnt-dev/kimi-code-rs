use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};

use crate::protocol::fs::{FsEntry, FsGitStatus, FsGrepFileHit, FsSearchHit};
use crate::protocol::validation::{
    absolute_url, literal_true, non_empty, optional_non_null, positive_u64, required_nullable,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsListSort {
    #[default]
    TypeFirst,
    NameAsc,
    NameDesc,
    MtimeDesc,
    SizeDesc,
}

fn default_path() -> String {
    ".".to_owned()
}

fn default_depth() -> u64 {
    1
}

fn default_limit() -> u64 {
    200
}

fn default_true() -> bool {
    true
}

fn depth<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_u64(deserializer, 1, 10, "depth")
}

fn list_limit<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_u64(deserializer, 1, 1_000, "limit")
}

fn bounded_u64<'de, D>(
    deserializer: D,
    minimum: u64,
    maximum: u64,
    field: &str,
) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "{field} must be between {minimum} and {maximum}"
        )))
    }
}

// Original: rest/fs.ts, fsListRequestSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsListRequest {
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "default_depth", deserialize_with = "depth")]
    pub depth: u64,
    #[serde(default = "default_limit", deserialize_with = "list_limit")]
    pub limit: u64,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default = "default_true")]
    pub follow_gitignore: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub exclude_globs: Option<Vec<String>>,
    #[serde(default)]
    pub sort: FsListSort,
    #[serde(default)]
    pub include_git_status: bool,
}

impl Default for FsListRequest {
    fn default() -> Self {
        Self {
            path: default_path(),
            depth: default_depth(),
            limit: default_limit(),
            show_hidden: false,
            follow_gitignore: true,
            exclude_globs: None,
            sort: FsListSort::default(),
            include_git_status: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsListResponse {
    pub items: Vec<FsEntry>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub children_by_path: Option<IndexMap<String, Vec<FsEntry>>>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsReadEncodingRequest {
    #[default]
    Auto,
    #[serde(rename = "utf-8")]
    Utf8,
    Base64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsReadEncoding {
    #[serde(rename = "utf-8")]
    Utf8,
    Base64,
}

fn default_read_length() -> u64 {
    1_048_576
}

fn read_length<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_u64(deserializer, 1, 10_485_760, "length")
}

// Original: rest/fs.ts, fsReadRequestSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsReadRequest {
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "default_read_length", deserialize_with = "read_length")]
    pub length: u64,
    #[serde(default)]
    pub encoding: FsReadEncodingRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsReadResponse {
    pub path: String,
    pub content: String,
    pub encoding: FsReadEncoding,
    pub size: u64,
    pub truncated: bool,
    pub etag: String,
    pub mime: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub language_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub line_count: Option<u64>,
    pub is_binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsOpenRequest {
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_positive"
    )]
    pub line: Option<u64>,
}

fn optional_positive<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        Err(serde::de::Error::custom("must be at least 1"))
    } else {
        Ok(Some(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsOpenResponse {
    #[serde(deserialize_with = "literal_true")]
    pub opened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRevealRequest {
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRevealResponse {
    #[serde(deserialize_with = "literal_true")]
    pub revealed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsMkdirRequest {
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

pub type FsMkdirResponse = FsEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsOpenInAppId {
    Finder,
    Cursor,
    Vscode,
    Iterm,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsOpenInRequest {
    pub app_id: FsOpenInAppId,
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_positive"
    )]
    pub line: Option<u64>,
}

pub type FsOpenInResponse = FsOpenResponse;

// Original: rest/fs.ts, fsListManyRequestSchema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsListManyRequest {
    #[serde(deserialize_with = "paths_up_to_100")]
    pub paths: Vec<String>,
    #[serde(default = "default_depth", deserialize_with = "depth")]
    pub depth: u64,
    #[serde(default = "default_limit", deserialize_with = "list_limit")]
    pub limit: u64,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default = "default_true")]
    pub follow_gitignore: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub exclude_globs: Option<Vec<String>>,
    #[serde(default)]
    pub sort: FsListSort,
    #[serde(default)]
    pub include_git_status: bool,
}

fn paths_up_to_100<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_non_empty_paths(deserializer, 100)
}

fn paths_up_to_1000<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_non_empty_paths(deserializer, 1_000)
}

fn bounded_non_empty_paths<'de, D>(deserializer: D, maximum: usize) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let paths = Vec::<String>::deserialize(deserializer)?;
    if paths.is_empty() || paths.len() > maximum || paths.iter().any(String::is_empty) {
        Err(serde::de::Error::custom(format!(
            "requires 1 to {maximum} non-empty paths"
        )))
    } else {
        Ok(paths)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsListManyPartialError {
    pub code: i64,
    pub msg: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsListManyResponse {
    pub results: IndexMap<String, Vec<FsEntry>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub truncated_paths: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub partial_errors: Option<IndexMap<String, FsListManyPartialError>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsStatRequest {
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
}

pub type FsStatResponse = FsEntry;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsStatManyRequest {
    #[serde(deserialize_with = "paths_up_to_1000")]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsStatManyResponse {
    pub entries: IndexMap<String, Option<FsEntry>>,
}

fn default_search_limit() -> u64 {
    50
}

fn search_limit<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_u64(deserializer, 1, 200, "limit")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsSearchRequest {
    #[serde(deserialize_with = "non_empty")]
    pub query: String,
    #[serde(default = "default_search_limit", deserialize_with = "search_limit")]
    pub limit: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub include_globs: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub exclude_globs: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub follow_gitignore: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsSearchResponse {
    pub items: Vec<FsSearchHit>,
    pub truncated: bool,
}

fn default_max_files() -> u64 {
    200
}

fn default_max_matches_per_file() -> u64 {
    50
}

fn default_max_total_matches() -> u64 {
    5_000
}

fn default_context_lines() -> u64 {
    2
}

fn max_files<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_u64(deserializer, 1, 10_000, "max_files")
}

fn max_matches_per_file<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_u64(deserializer, 1, 10_000, "max_matches_per_file")
}

fn max_total_matches<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_u64(deserializer, 1, 100_000, "max_total_matches")
}

fn context_lines<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    bounded_u64(deserializer, 0, 10, "context_lines")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsGrepRequest {
    #[serde(deserialize_with = "non_empty")]
    pub pattern: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub include_globs: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub exclude_globs: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub follow_gitignore: bool,
    #[serde(default = "default_max_files", deserialize_with = "max_files")]
    pub max_files: u64,
    #[serde(
        default = "default_max_matches_per_file",
        deserialize_with = "max_matches_per_file"
    )]
    pub max_matches_per_file: u64,
    #[serde(
        default = "default_max_total_matches",
        deserialize_with = "max_total_matches"
    )]
    pub max_total_matches: u64,
    #[serde(default = "default_context_lines", deserialize_with = "context_lines")]
    pub context_lines: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsGrepResponse {
    pub files: Vec<FsGrepFileHit>,
    pub files_scanned: u64,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsGitStatusRequest {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty_paths"
    )]
    pub paths: Option<Vec<String>>,
}

fn optional_non_empty_paths<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let paths = Vec::<String>::deserialize(deserializer)?;
    if paths.iter().any(String::is_empty) {
        Err(serde::de::Error::custom("paths must not be empty"))
    } else {
        Ok(Some(paths))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsPullRequestState {
    Open,
    Merged,
    Closed,
    Draft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsPullRequest {
    #[serde(deserialize_with = "positive_u64")]
    pub number: u64,
    pub state: FsPullRequestState,
    #[serde(deserialize_with = "absolute_url")]
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsGitStatusResponse {
    pub branch: String,
    pub ahead: u64,
    pub behind: u64,
    pub entries: IndexMap<String, FsGitStatus>,
    pub additions: u64,
    pub deletions: u64,
    #[serde(rename = "pullRequest", deserialize_with = "required_nullable")]
    pub pull_request: Option<FsPullRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsDiffRequest {
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsDiffResponse {
    pub path: String,
    pub diff: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsDownloadParams {
    #[serde(deserialize_with = "non_empty")]
    pub path: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub range: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub if_none_match: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_list_and_read_defaults_and_bounds() {
        let list: FsListRequest = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(list, FsListRequest::default());
        assert!(serde_json::from_value::<FsListRequest>(serde_json::json!({"depth": 11})).is_err());

        let read: FsReadRequest =
            serde_json::from_value(serde_json::json!({"path": "a.ts"})).unwrap();
        assert_eq!(read.offset, 0);
        assert_eq!(read.length, 1_048_576);
        assert_eq!(read.encoding, FsReadEncodingRequest::Auto);
        assert!(
            serde_json::from_value::<FsReadRequest>(serde_json::json!({
                "path": "a.ts", "length": 10_485_761
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<FsOpenResponse>(serde_json::json!({"opened": false})).is_err()
        );

        let grep: FsGrepRequest =
            serde_json::from_value(serde_json::json!({"pattern": "hello"})).unwrap();
        assert_eq!(grep.max_files, 200);
        assert_eq!(grep.max_total_matches, 5_000);
        assert_eq!(grep.context_lines, 2);
        assert!(
            serde_json::from_value::<FsListManyRequest>(serde_json::json!({"paths": []})).is_err()
        );
        assert!(
            serde_json::from_value::<FsGitStatusResponse>(serde_json::json!({
                "branch": "main", "ahead": 0, "behind": 0, "entries": {},
                "additions": 0, "deletions": 0
            }))
            .is_err()
        );
    }
}
