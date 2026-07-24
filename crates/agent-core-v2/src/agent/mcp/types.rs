//! MCP protocol values used by the agent-side tool runtime.
//!
//! Original: `agent/mcp/types.ts`.

use std::error::Error;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::_base::utils::abort::AbortSignal;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEmbeddedResourceContents {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<McpEmbeddedResourceContents>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolResult {
    pub content: Vec<McpContentBlock>,
    pub is_error: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
#[error("Invalid inputSchema for MCP tool \"{tool_name}\": schema must be a JSON object")]
pub struct McpInputSchemaError {
    pub tool_name: String,
}

// Original: assertMcpInputSchema().
pub fn assert_mcp_input_schema(
    tool_name: &str,
    input_schema: &Value,
) -> Result<Map<String, Value>, McpInputSchemaError> {
    input_schema
        .as_object()
        .cloned()
        .ok_or_else(|| McpInputSchemaError {
            tool_name: tool_name.into(),
        })
}

pub type McpClientError = Box<dyn Error + Send + Sync>;

// Original: MCPClient. Transport implementations remain async because MCP
// tools/list and tools/call may wait on a local process or remote server.
#[async_trait]
pub trait McpClient: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpClientError>;

    async fn call_tool(
        &self,
        name: &str,
        args: Map<String, Value>,
        signal: Option<AbortSignal>,
    ) -> Result<McpToolResult, McpClientError>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn preserves_content_extensions_and_rejects_non_object_schemas() {
        let block: McpContentBlock = serde_json::from_value(json!({
            "type": "resource",
            "resource": {"uri": "file:///x", "text": "body", "vendor": true},
            "vendorBlock": 1
        }))
        .unwrap();
        assert_eq!(block.resource.as_ref().unwrap().extra["vendor"], true);
        assert_eq!(block.extra["vendorBlock"], 1);
        assert_eq!(
            assert_mcp_input_schema("read", &json!({"type": "object"})).unwrap()["type"],
            "object"
        );
        assert_eq!(
            assert_mcp_input_schema("read", &json!([]))
                .unwrap_err()
                .to_string(),
            "Invalid inputSchema for MCP tool \"read\": schema must be a JSON object"
        );
    }
}
