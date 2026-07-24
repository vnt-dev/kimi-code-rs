//! Shared MCP client conversion helpers.
//!
//! Original: `agent/mcp/client-shared.ts`.

use serde_json::{Map, Value};

use crate::{
    _base::{utils::abort::AbortSignal, version::get_core_version},
    agent::mcp::{McpContentBlock, McpToolDefinition, McpToolResult},
};

pub const KIMI_MCP_CLIENT_NAME: &str = "kimi-code";
pub const KIMI_MCP_CLIENT_VERSION: &str = get_core_version();

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnexpectedCloseReason {
    pub error: Option<String>,
    pub stderr: Option<String>,
}

#[derive(Clone)]
pub struct McpRequestOptions {
    pub timeout_ms: Option<u64>,
    pub signal: Option<AbortSignal>,
}

impl std::fmt::Debug for McpRequestOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpRequestOptions")
            .field("timeout_ms", &self.timeout_ms)
            .field("has_signal", &self.signal.is_some())
            .finish()
    }
}

// Original: buildRequestOptions().
pub fn build_request_options(
    tool_call_timeout_ms: Option<u64>,
    signal: Option<AbortSignal>,
) -> Option<McpRequestOptions> {
    (tool_call_timeout_ms.is_some() || signal.is_some()).then_some(McpRequestOptions {
        timeout_ms: tool_call_timeout_ms,
        signal,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct SdkListedTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Map<String, Value>,
}

// Original: toMcpToolDefinition().
pub fn to_mcp_tool_definition(tool: SdkListedTool) -> McpToolDefinition {
    McpToolDefinition {
        name: tool.name,
        description: tool.description.unwrap_or_default(),
        input_schema: Value::Object(tool.input_schema),
    }
}

// Original: toMcpToolResult(). Unsupported result shapes are intentionally a
// successful empty result, matching the source SDK adaptation.
pub fn to_mcp_tool_result(result: &Value) -> McpToolResult {
    if let Some(object) = result.as_object()
        && let Some(content) = object.get("content").and_then(Value::as_array)
    {
        return McpToolResult {
            content: content
                .iter()
                .filter_map(|block| serde_json::from_value::<McpContentBlock>(block.clone()).ok())
                .collect(),
            is_error: object.get("isError") == Some(&Value::Bool(true)),
        };
    }
    if let Some(legacy) = result
        .as_object()
        .and_then(|object| object.get("toolResult"))
    {
        let text = legacy
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| serde_json::to_string(legacy).unwrap_or_default());
        return McpToolResult {
            content: vec![McpContentBlock {
                kind: "text".into(),
                text: Some(text),
                ..McpContentBlock::default()
            }],
            is_error: false,
        };
    }
    McpToolResult::default()
}

#[cfg(test)]
mod tests {
    use crate::_base::utils::abort::AbortController;

    use super::*;

    #[test]
    fn builds_optional_request_options_and_normalizes_sdk_results() {
        assert!(build_request_options(None, None).is_none());
        assert!(build_request_options(Some(100), None).is_some());
        assert!(build_request_options(None, Some(AbortController::new().signal())).is_some());
        assert_eq!(
            to_mcp_tool_definition(SdkListedTool {
                name: "read".into(),
                description: None,
                input_schema: Map::from_iter([("type".into(), Value::String("object".into()))]),
            }),
            McpToolDefinition {
                name: "read".into(),
                description: String::new(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        );
        let modern = to_mcp_tool_result(&serde_json::json!({
            "content": [{"type": "text", "text": "bad"}],
            "isError": true
        }));
        assert!(modern.is_error);
        assert_eq!(modern.content[0].text.as_deref(), Some("bad"));
        let legacy = to_mcp_tool_result(&serde_json::json!({"toolResult": {"ok": true}}));
        assert_eq!(legacy.content[0].text.as_deref(), Some("{\"ok\":true}"));
    }
}
