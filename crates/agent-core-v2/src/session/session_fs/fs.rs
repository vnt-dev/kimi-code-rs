//! Session-scoped filesystem protocol and service contract.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/fs.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    _base::di::instantiation::ServiceIdentifier,
    app::git::{
        FsDiffRequest, FsDiffResponse, FsGitStatus, FsGitStatusRequest, FsGitStatusResponse,
    },
};

pub type SessionFsError = Box<dyn Error + Send + Sync>;
pub type SessionFsResult<T> = Result<T, SessionFsError>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FsKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FsEntry {
    pub path: String,
    pub name: String,
    pub kind: FsKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    pub modified_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_binary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_symlink_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_status: Option<FsGitStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_count: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FsSearchHit {
    pub path: String,
    pub name: String,
    pub kind: FsKind,
    pub score: f64,
    pub match_positions: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsGrepMatch {
    pub line: u64,
    pub col: u64,
    pub text: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsGrepFileHit {
    pub path: String,
    pub matches: Vec<FsGrepMatch>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
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
    ".".into()
}
fn default_depth() -> usize {
    1
}
fn default_list_limit() -> usize {
    200
}
fn default_true() -> bool {
    true
}
fn default_read_length() -> usize {
    1_048_576
}
fn default_search_limit() -> usize {
    50
}
fn default_max_files() -> usize {
    200
}
fn default_max_matches_per_file() -> usize {
    50
}
fn default_max_total_matches() -> usize {
    5_000
}
fn default_context_lines() -> usize {
    2
}

fn deserialize_nonempty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        Err(serde::de::Error::custom("must not be empty"))
    } else {
        Ok(value)
    }
}

fn deserialize_depth<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_range(deserializer, 1, 10, "depth")
}

fn deserialize_list_limit<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_range(deserializer, 1, 1_000, "limit")
}

fn deserialize_read_length<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_range(deserializer, 1, 10_485_760, "length")
}

fn deserialize_search_limit<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_range(deserializer, 1, 200, "limit")
}

fn deserialize_max_files<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_range(deserializer, 1, 10_000, "max_files")
}

fn deserialize_max_matches<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_range(deserializer, 1, 10_000, "max_matches_per_file")
}

fn deserialize_max_total<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_range(deserializer, 1, 100_000, "max_total_matches")
}

fn deserialize_context_lines<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_range(deserializer, 0, 10, "context_lines")
}

fn deserialize_range<'de, D>(
    deserializer: D,
    minimum: usize,
    maximum: usize,
    field: &str,
) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "{field} must be from {minimum} through {maximum}"
        )))
    }
}

fn deserialize_paths_100<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_paths(deserializer, 100)
}

fn deserialize_paths_1000<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_paths(deserializer, 1_000)
}

fn deserialize_paths<'de, D>(deserializer: D, maximum: usize) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let paths = Vec::<String>::deserialize(deserializer)?;
    if paths.is_empty() || paths.len() > maximum || paths.iter().any(String::is_empty) {
        return Err(serde::de::Error::custom(format!(
            "paths must contain 1 through {maximum} non-empty strings"
        )));
    }
    Ok(paths)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsListRequest {
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "default_depth", deserialize_with = "deserialize_depth")]
    pub depth: usize,
    #[serde(
        default = "default_list_limit",
        deserialize_with = "deserialize_list_limit"
    )]
    pub limit: usize,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default = "default_true")]
    pub follow_gitignore: bool,
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
            limit: default_list_limit(),
            show_hidden: false,
            follow_gitignore: true,
            exclude_globs: None,
            sort: FsListSort::TypeFirst,
            include_git_status: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FsListResponse {
    pub items: Vec<FsEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children_by_path: Option<IndexMap<String, Vec<FsEntry>>>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum FsReadRequestEncoding {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "utf-8")]
    Utf8,
    #[serde(rename = "base64")]
    Base64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FsReadEncoding {
    #[serde(rename = "utf-8")]
    Utf8,
    #[serde(rename = "base64")]
    Base64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsReadRequest {
    #[serde(deserialize_with = "deserialize_nonempty")]
    pub path: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(
        default = "default_read_length",
        deserialize_with = "deserialize_read_length"
    )]
    pub length: usize,
    #[serde(default)]
    pub encoding: FsReadRequestEncoding,
}

impl Default for FsReadRequest {
    fn default() -> Self {
        Self {
            path: String::new(),
            offset: 0,
            length: default_read_length(),
            encoding: FsReadRequestEncoding::Auto,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsReadResponse {
    pub path: String,
    pub content: String,
    pub encoding: FsReadEncoding,
    pub size: u64,
    pub truncated: bool,
    pub etag: String,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_count: Option<u64>,
    pub is_binary: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsMkdirRequest {
    #[serde(deserialize_with = "deserialize_nonempty")]
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

pub type FsMkdirResponse = FsEntry;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsListManyRequest {
    #[serde(deserialize_with = "deserialize_paths_100")]
    pub paths: Vec<String>,
    #[serde(default = "default_depth", deserialize_with = "deserialize_depth")]
    pub depth: usize,
    #[serde(
        default = "default_list_limit",
        deserialize_with = "deserialize_list_limit"
    )]
    pub limit: usize,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default = "default_true")]
    pub follow_gitignore: bool,
    pub exclude_globs: Option<Vec<String>>,
    #[serde(default)]
    pub sort: FsListSort,
    #[serde(default)]
    pub include_git_status: bool,
}

impl Default for FsListManyRequest {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            depth: default_depth(),
            limit: default_list_limit(),
            show_hidden: false,
            follow_gitignore: true,
            exclude_globs: None,
            sort: FsListSort::TypeFirst,
            include_git_status: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsListManyPartialError {
    pub code: i64,
    pub msg: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FsListManyResponse {
    pub results: IndexMap<String, Vec<FsEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_errors: Option<IndexMap<String, FsListManyPartialError>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsStatRequest {
    #[serde(deserialize_with = "deserialize_nonempty")]
    pub path: String,
}

pub type FsStatResponse = FsEntry;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsStatManyRequest {
    #[serde(deserialize_with = "deserialize_paths_1000")]
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FsStatManyResponse {
    pub entries: IndexMap<String, Option<FsEntry>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsSearchRequest {
    #[serde(deserialize_with = "deserialize_nonempty")]
    pub query: String,
    #[serde(
        default = "default_search_limit",
        deserialize_with = "deserialize_search_limit"
    )]
    pub limit: usize,
    pub include_globs: Option<Vec<String>>,
    pub exclude_globs: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub follow_gitignore: bool,
}

impl Default for FsSearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: default_search_limit(),
            include_globs: None,
            exclude_globs: None,
            follow_gitignore: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FsSearchResponse {
    pub items: Vec<FsSearchHit>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsGrepRequest {
    #[serde(deserialize_with = "deserialize_nonempty")]
    pub pattern: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    pub include_globs: Option<Vec<String>>,
    pub exclude_globs: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub follow_gitignore: bool,
    #[serde(
        default = "default_max_files",
        deserialize_with = "deserialize_max_files"
    )]
    pub max_files: usize,
    #[serde(
        default = "default_max_matches_per_file",
        deserialize_with = "deserialize_max_matches"
    )]
    pub max_matches_per_file: usize,
    #[serde(
        default = "default_max_total_matches",
        deserialize_with = "deserialize_max_total"
    )]
    pub max_total_matches: usize,
    #[serde(
        default = "default_context_lines",
        deserialize_with = "deserialize_context_lines"
    )]
    pub context_lines: usize,
}

impl Default for FsGrepRequest {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            regex: false,
            case_sensitive: true,
            include_globs: None,
            exclude_globs: None,
            follow_gitignore: true,
            max_files: default_max_files(),
            max_matches_per_file: default_max_matches_per_file(),
            max_total_matches: default_max_total_matches(),
            context_lines: default_context_lines(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FsGrepResponse {
    pub files: Vec<FsGrepFileHit>,
    pub files_scanned: usize,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FsPathResolved {
    pub absolute: String,
    pub relative: String,
    pub is_directory: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FsDownloadResolved {
    pub absolute: String,
    pub relative: String,
    pub size: u64,
    pub etag: String,
    pub mime: String,
    pub modified_at: DateTime<Utc>,
}

#[async_trait]
pub trait SessionFsServiceContract: Send + Sync {
    async fn list(&self, request: FsListRequest) -> SessionFsResult<FsListResponse>;
    async fn read(&self, request: FsReadRequest) -> SessionFsResult<FsReadResponse>;
    async fn list_many(&self, request: FsListManyRequest) -> SessionFsResult<FsListManyResponse>;
    async fn stat(&self, request: FsStatRequest) -> SessionFsResult<FsStatResponse>;
    async fn stat_many(&self, request: FsStatManyRequest) -> SessionFsResult<FsStatManyResponse>;
    async fn mkdir(&self, request: FsMkdirRequest) -> SessionFsResult<FsMkdirResponse>;
    async fn search(&self, request: FsSearchRequest) -> SessionFsResult<FsSearchResponse>;
    async fn grep(&self, request: FsGrepRequest) -> SessionFsResult<FsGrepResponse>;
    async fn git_status(&self, request: FsGitStatusRequest)
    -> SessionFsResult<FsGitStatusResponse>;
    async fn diff(&self, request: FsDiffRequest) -> SessionFsResult<FsDiffResponse>;
    async fn resolve_path(&self, relative_path: &str) -> SessionFsResult<FsPathResolved>;
    async fn resolve_download(&self, relative_path: &str) -> SessionFsResult<FsDownloadResolved>;
}

#[derive(Clone)]
pub struct SessionFsServiceHandle(pub Arc<dyn SessionFsServiceContract>);

impl Deref for SessionFsServiceHandle {
    type Target = dyn SessionFsServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_FS_SERVICE_ID: ServiceIdentifier<SessionFsServiceHandle> =
    ServiceIdentifier::new("sessionFsService");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn request_defaults_constraints_and_unknown_stripping_match_source_schemas() {
        assert_eq!(
            serde_json::from_value::<FsListRequest>(json!({})).unwrap(),
            FsListRequest::default()
        );
        assert_eq!(
            serde_json::from_value::<FsGrepRequest>(json!({"pattern":"x"}))
                .unwrap()
                .context_lines,
            2
        );
        for invalid in [
            json!({"path":"x","depth":0}),
            json!({"path":"x","limit":1001}),
        ] {
            assert!(serde_json::from_value::<FsListRequest>(invalid).is_err());
        }
        assert!(serde_json::from_value::<FsReadRequest>(json!({"path":""})).is_err());
        assert!(serde_json::from_value::<FsStatManyRequest>(json!({"paths":[]})).is_err());
        assert!(serde_json::from_value::<FsListRequest>(json!({"unknown":true})).is_ok());
        assert_eq!(SESSION_FS_SERVICE_ID.to_string(), "sessionFsService");
    }

    #[test]
    fn wire_shapes_preserve_snake_case_and_optional_omission() {
        let response = FsReadResponse {
            path: "src/lib.rs".into(),
            content: "x".into(),
            encoding: FsReadEncoding::Utf8,
            size: 1,
            truncated: false,
            etag: "tag".into(),
            mime: "text/rust".into(),
            language_id: Some("rust".into()),
            line_count: Some(1),
            is_binary: false,
        };
        assert_eq!(serde_json::to_value(response).unwrap()["encoding"], "utf-8");
        assert_eq!(
            serde_json::to_value(FsListResponse {
                items: Vec::new(),
                children_by_path: None,
                truncated: false,
            })
            .unwrap(),
            json!({"items":[],"truncated":false})
        );
    }
}
