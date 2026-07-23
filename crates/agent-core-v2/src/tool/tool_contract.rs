use serde::{Deserialize, Serialize};

use crate::kosong::contract::message::{ContentPart, ToolCall};

pub use kimi_code_protocol::ToolInputDisplay;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolSource {
    Builtin,
    User,
    Mcp,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutableToolOutput {
    Text(String),
    Content(Vec<ContentPart>),
}

impl From<String> for ExecutableToolOutput {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for ExecutableToolOutput {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDeliveryMessage {
    pub content: Vec<ContentPart>,
    pub tool_calls: Vec<ToolCall>,
    pub origin: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDelivery {
    pub message: ToolDeliveryMessage,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExecutableToolResult {
    pub output: ExecutableToolOutput,
    pub is_error: bool,
    pub stop_turn: Option<bool>,
    pub truncated: Option<bool>,
    pub note: Option<String>,
    pub delivery: Option<ToolDelivery>,
}

impl ExecutableToolResult {
    pub fn success(output: impl Into<ExecutableToolOutput>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            stop_turn: None,
            truncated: None,
            note: None,
            delivery: None,
        }
    }

    pub fn error(output: impl Into<ExecutableToolOutput>) -> Self {
        Self {
            is_error: true,
            ..Self::success(output)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolUpdateKind {
    Stdout,
    Stderr,
    Progress,
    Status,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUpdate {
    pub kind: ToolUpdateKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_data: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Option<serde_json::Map<String, serde_json::Value>>,
    pub source: Option<ToolSource>,
    pub info: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolFileAccessOperation {
    Read,
    Write,
    ReadWrite,
    Search,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolFileAccess {
    pub operation: ToolFileAccessOperation,
    pub path: String,
    pub recursive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolResourceAccess {
    File(ToolFileAccess),
    All,
}

pub type ToolAccesses = Vec<ToolResourceAccess>;

pub struct ToolAccess;

impl ToolAccess {
    pub fn none() -> ToolAccesses {
        Vec::new()
    }

    pub fn all() -> ToolAccesses {
        vec![ToolResourceAccess::All]
    }

    pub fn file(
        operation: ToolFileAccessOperation,
        path: impl Into<String>,
        recursive: bool,
    ) -> ToolAccesses {
        vec![ToolResourceAccess::File(ToolFileAccess {
            operation,
            path: path.into(),
            recursive,
        })]
    }

    pub fn read_file(path: impl Into<String>) -> ToolAccesses {
        Self::file(ToolFileAccessOperation::Read, path, false)
    }

    pub fn read_tree(path: impl Into<String>) -> ToolAccesses {
        Self::file(ToolFileAccessOperation::Read, path, true)
    }

    pub fn write_file(path: impl Into<String>) -> ToolAccesses {
        Self::file(ToolFileAccessOperation::Write, path, false)
    }

    pub fn write_tree(path: impl Into<String>) -> ToolAccesses {
        Self::file(ToolFileAccessOperation::Write, path, true)
    }

    pub fn read_write_file(path: impl Into<String>) -> ToolAccesses {
        Self::file(ToolFileAccessOperation::ReadWrite, path, false)
    }

    pub fn read_write_tree(path: impl Into<String>) -> ToolAccesses {
        Self::file(ToolFileAccessOperation::ReadWrite, path, true)
    }

    pub fn search_tree(path: impl Into<String>) -> ToolAccesses {
        Self::file(ToolFileAccessOperation::Search, path, true)
    }

    // Original: ToolAccesses.conflict(). Reads/searches can overlap; any overlapping write conflicts.
    pub fn conflict(left: &[ToolResourceAccess], right: &[ToolResourceAccess]) -> bool {
        left.iter().any(|left| {
            right
                .iter()
                .any(|right| resource_accesses_conflict(left, right))
        })
    }
}

fn resource_accesses_conflict(left: &ToolResourceAccess, right: &ToolResourceAccess) -> bool {
    match (left, right) {
        (ToolResourceAccess::All, _) | (_, ToolResourceAccess::All) => true,
        (ToolResourceAccess::File(left), ToolResourceAccess::File(right)) => {
            if !operation_writes(left.operation) && !operation_writes(right.operation) {
                return false;
            }
            file_accesses_overlap(left, right)
        }
    }
}

fn operation_writes(operation: ToolFileAccessOperation) -> bool {
    matches!(
        operation,
        ToolFileAccessOperation::Write | ToolFileAccessOperation::ReadWrite
    )
}

fn file_accesses_overlap(left: &ToolFileAccess, right: &ToolFileAccess) -> bool {
    let left_path = normalize_path(&left.path);
    let right_path = normalize_path(&right.path);
    if left_path == right_path {
        return true;
    }
    let left_prefix = format!("{left_path}/");
    let right_prefix = format!("{right_path}/");
    (left.recursive && right_path.starts_with(&left_prefix))
        || (right.recursive && left_path.starts_with(&right_prefix))
}

fn normalize_path(path: &str) -> String {
    let mut normalized = String::with_capacity(path.len());
    let mut previous_slash = false;
    for character in path.chars() {
        let character = if character == '\\' { '/' } else { character };
        if character == '/' {
            if previous_slash {
                continue;
            }
            previous_slash = true;
        } else {
            previous_slash = false;
        }
        normalized.extend(character.to_lowercase());
    }
    if normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    normalized
}

pub fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with("mcp__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_conflicts_preserve_write_recursive_and_normalization_rules() {
        assert!(!ToolAccess::conflict(
            &ToolAccess::read_tree("C:\\Work"),
            &ToolAccess::search_tree("c:/work/sub")
        ));
        assert!(ToolAccess::conflict(
            &ToolAccess::write_tree("C:\\Work\\"),
            &ToolAccess::read_file("c:/work/sub/file")
        ));
        assert!(!ToolAccess::conflict(
            &ToolAccess::write_file("/workspace/a"),
            &ToolAccess::read_file("/workspace/a/child")
        ));
        assert!(!ToolAccess::conflict(
            &ToolAccess::all(),
            &ToolAccess::none()
        ));
        assert!(!ToolAccess::conflict(
            &ToolAccess::none(),
            &ToolAccess::all()
        ));
        assert!(ToolAccess::conflict(
            &ToolAccess::all(),
            &ToolAccess::read_file("/workspace/file")
        ));
    }

    #[test]
    fn mcp_name_requires_exact_prefix() {
        assert!(is_mcp_tool_name("mcp__server__tool"));
        assert!(!is_mcp_tool_name("mcp_server__tool"));
    }
}
