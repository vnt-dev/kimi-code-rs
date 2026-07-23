//! Foundational tool metadata, execution, result, and resource-access contracts.
//!
//! Original: `packages/agent-core-v2/src/tool/toolContract.ts` and
//! `packages/agent-core-v2/src/tool/toolInputDisplay.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::{
    _base::utils::abort::AbortSignal,
    kosong::contract::{
        message::{ContentPart, ToolCall},
        request_trace::LlmRequestTrace,
        tool::Tool,
    },
};

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
    pub tool_calls: Option<Vec<ToolCall>>,
    pub origin: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolDeliveryKind {
    Steer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDelivery {
    pub kind: ToolDeliveryKind,
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

pub type ToolUpdateCallback = Arc<dyn Fn(ToolUpdate) + Send + Sync>;
pub type ForegroundTaskStartCallback = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone)]
pub struct ExecutableToolContext {
    pub turn_id: u64,
    pub tool_call_id: String,
    pub trace: Option<LlmRequestTrace>,
    pub metadata: Option<serde_json::Value>,
    pub signal: AbortSignal,
    pub on_update: Option<ToolUpdateCallback>,
    pub on_foreground_task_start: Option<ForegroundTaskStartCallback>,
}

pub type ToolExecute =
    Arc<dyn Fn(ExecutableToolContext) -> BoxFuture<'static, ExecutableToolResult> + Send + Sync>;
pub type ToolRuleMatcher = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct RunnableToolExecution {
    pub accesses: Option<ToolAccesses>,
    pub display: Option<ToolInputDisplay>,
    pub description: Option<String>,
    pub stop_batch_after_this: Option<bool>,
    pub approval_rule: String,
    pub matches_rule: Option<ToolRuleMatcher>,
    execute: ToolExecute,
}

impl RunnableToolExecution {
    pub fn new(approval_rule: impl Into<String>, execute: ToolExecute) -> Self {
        Self {
            accesses: None,
            display: None,
            description: None,
            stop_batch_after_this: None,
            approval_rule: approval_rule.into(),
            matches_rule: None,
            execute,
        }
    }

    // Original: RunnableToolExecution.execute(ctx).
    pub async fn execute(&self, context: ExecutableToolContext) -> ExecutableToolResult {
        (self.execute)(context).await
    }

    pub fn matches_rule(&self, rule_args: &str) -> bool {
        self.matches_rule
            .as_ref()
            .is_some_and(|matches| matches(rule_args))
    }
}

#[derive(Clone)]
pub enum ToolExecution {
    Runnable(RunnableToolExecution),
    Error(ExecutableToolResult),
}

// Original: ExecutableTool.resolveExecution(). Promise-based implementations
// become async methods while synchronous implementations can return immediately.
#[async_trait]
pub trait ExecutableTool: Send + Sync {
    type Input: Send;

    fn tool(&self) -> &Tool;

    async fn resolve_execution(&self, input: Self::Input) -> ToolExecution;
}

pub trait BuiltinTool: ExecutableTool {}

impl<T: ExecutableTool> BuiltinTool for T {}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Option<serde_json::Map<String, serde_json::Value>>,
    pub source: Option<ToolSource>,
    pub info: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: Option<serde_json::Map<String, serde_json::Value>>,
    pub source: ToolSource,
    pub info: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ToolDefinition {
    pub fn with_source(self, default_source: ToolSource) -> ToolInfo {
        ToolInfo {
            name: self.name,
            description: self.description,
            parameters: self.parameters,
            source: self.source.unwrap_or(default_source),
            info: self.info,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub output: ExecutableToolOutput,
    pub is_error: bool,
    pub stop_turn: Option<bool>,
    pub truncated: Option<bool>,
    pub note: Option<String>,
    pub delivery: Option<ToolDelivery>,
    pub description: Option<String>,
    pub display: Option<ToolInputDisplay>,
    pub approval_rule: Option<String>,
    pub stop_batch_after_this: Option<bool>,
}

impl From<ExecutableToolResult> for ToolResult {
    fn from(result: ExecutableToolResult) -> Self {
        Self {
            output: result.output,
            is_error: result.is_error,
            stop_turn: result.stop_turn,
            truncated: result.truncated,
            note: result.note,
            delivery: result.delivery,
            description: None,
            display: None,
            approval_rule: None,
            stop_batch_after_this: None,
        }
    }
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
    use crate::_base::utils::abort::AbortController;

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

    struct EchoTool {
        tool: Tool,
    }

    #[async_trait]
    impl ExecutableTool for EchoTool {
        type Input = String;

        fn tool(&self) -> &Tool {
            &self.tool
        }

        async fn resolve_execution(&self, input: Self::Input) -> ToolExecution {
            let execute = Arc::new(move |context: ExecutableToolContext| {
                let input = input.clone();
                Box::pin(async move {
                    if let Some(update) = context.on_update {
                        update(ToolUpdate {
                            kind: ToolUpdateKind::Stdout,
                            text: Some(input.clone()),
                            percent: None,
                            custom_kind: None,
                            custom_data: None,
                        });
                    }
                    ExecutableToolResult::success(input)
                }) as BoxFuture<'static, ExecutableToolResult>
            });
            ToolExecution::Runnable(RunnableToolExecution::new("Echo(*)", execute))
        }
    }

    #[tokio::test]
    async fn executable_tool_resolves_then_runs_with_context_callbacks() {
        let tool = EchoTool {
            tool: Tool {
                name: "Echo".into(),
                description: "Echo input".into(),
                parameters: serde_json::Map::new(),
                deferred: None,
            },
        };
        assert_eq!(tool.tool().name, "Echo");
        let ToolExecution::Runnable(mut execution) = tool.resolve_execution("hello".into()).await
        else {
            panic!("expected runnable execution")
        };
        execution.matches_rule = Some(Arc::new(|rule| rule == "*"));
        assert!(execution.matches_rule("*"));
        assert!(!execution.matches_rule("other"));

        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let updates_for_callback = Arc::clone(&updates);
        let result = execution
            .execute(ExecutableToolContext {
                turn_id: 7,
                tool_call_id: "call-1".into(),
                trace: None,
                metadata: None,
                signal: AbortController::new().signal(),
                on_update: Some(Arc::new(move |update| {
                    updates_for_callback.lock().unwrap().push(update);
                })),
                on_foreground_task_start: None,
            })
            .await;
        assert_eq!(result.output, ExecutableToolOutput::Text("hello".into()));
        assert!(!result.is_error);
        assert_eq!(updates.lock().unwrap()[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn definitions_resolve_default_source_and_delivery_preserves_optional_calls() {
        let info = ToolDefinition {
            name: "Read".into(),
            description: "Read a file".into(),
            parameters: None,
            source: None,
            info: None,
        }
        .with_source(ToolSource::Builtin);
        assert_eq!(info.source, ToolSource::Builtin);

        let result = ExecutableToolResult {
            delivery: Some(ToolDelivery {
                kind: ToolDeliveryKind::Steer,
                message: ToolDeliveryMessage {
                    content: Vec::new(),
                    tool_calls: None,
                    origin: None,
                },
            }),
            ..ExecutableToolResult::success("done")
        };
        assert_eq!(result.delivery.unwrap().kind, ToolDeliveryKind::Steer);
        let finalized = ToolResult::from(ExecutableToolResult::success("done"));
        assert_eq!(finalized.output, ExecutableToolOutput::Text("done".into()));
        assert!(!finalized.is_error);
    }
}
