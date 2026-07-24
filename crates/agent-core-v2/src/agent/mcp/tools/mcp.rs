//! Adapter from a connected MCP tool to an agent executable tool.
//!
//! Original: `agent/mcp/tools/mcp.ts`.

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use crate::{
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution,
    },
};

use super::super::{McpClient, McpOutputOptions, mcp_result_to_executable_output};

#[derive(Clone, Debug, Default)]
pub struct McpToolOptions {
    pub originals_dir: Option<PathBuf>,
}

pub struct McpTool {
    qualified_name: String,
    source_name: String,
    client: Arc<dyn McpClient>,
    options: McpToolOptions,
    definition: Tool,
}

// Original: createMcpTool().
pub fn create_mcp_tool(
    qualified_name: impl Into<String>,
    tool: Tool,
    client: Arc<dyn McpClient>,
    options: McpToolOptions,
) -> McpTool {
    let qualified_name = qualified_name.into();
    McpTool {
        source_name: tool.name.clone(),
        definition: Tool {
            name: qualified_name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
            deferred: tool.deferred,
        },
        qualified_name,
        client,
        options,
    }
}

#[async_trait]
impl ExecutableTool for McpTool {
    type Input = Value;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: Value) -> ToolExecution {
        let source_name = self.source_name.clone();
        let qualified_name = self.qualified_name.clone();
        let client = Arc::clone(&self.client);
        let output_options = McpOutputOptions {
            originals_dir: self.options.originals_dir.clone(),
        };
        let arguments = args.as_object().cloned().unwrap_or_else(Map::new);
        let approval_rule = qualified_name.clone();
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let source_name = source_name.clone();
            let qualified_name = qualified_name.clone();
            let client = Arc::clone(&client);
            let output_options = output_options.clone();
            let arguments = arguments.clone();
            Box::pin(async move {
                let result = match client
                    .call_tool(&source_name, arguments, Some(context.signal))
                    .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        return ExecutableToolResult::error(format!(
                            "MCP tool \"{qualified_name}\" failed: {error}"
                        ));
                    }
                };
                let normalized =
                    mcp_result_to_executable_output(&result, &qualified_name, &output_options)
                        .await;
                ExecutableToolResult {
                    output: normalized.output,
                    is_error: normalized.is_error,
                    stop_turn: None,
                    truncated: normalized.truncated,
                    note: normalized.note,
                    delivery: None,
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        ToolExecution::Runnable(RunnableToolExecution::new(approval_rule, execute))
    }
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Mutex};

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        agent::mcp::{McpContentBlock, McpToolDefinition, McpToolResult},
        tool::{ExecutableToolOutput, ToolExecution},
    };

    struct FakeClient {
        calls: Mutex<Vec<(String, Map<String, Value>)>>,
    }

    #[async_trait]
    impl McpClient for FakeClient {
        async fn list_tools(
            &self,
        ) -> Result<Vec<McpToolDefinition>, super::super::super::McpClientError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            name: &str,
            args: Map<String, Value>,
            _signal: Option<crate::_base::utils::abort::AbortSignal>,
        ) -> Result<McpToolResult, super::super::super::McpClientError> {
            self.calls.lock().unwrap().push((name.into(), args));
            Ok(McpToolResult {
                content: vec![McpContentBlock {
                    kind: "text".into(),
                    text: Some("remote result".into()),
                    ..Default::default()
                }],
                is_error: false,
            })
        }
    }

    #[tokio::test]
    async fn calls_the_unqualified_remote_tool_and_uses_qualified_approval_rule() {
        let client = Arc::new(FakeClient {
            calls: Mutex::new(Vec::new()),
        });
        let tool = create_mcp_tool(
            "mcp__server__read",
            Tool {
                name: "read".into(),
                description: "Read remote data".into(),
                parameters: Map::new(),
                deferred: None,
            },
            client.clone(),
            McpToolOptions::default(),
        );
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(serde_json::json!({"path": "/tmp/a"}))
            .await
        else {
            panic!("MCP tool must be runnable");
        };
        assert_eq!(execution.approval_rule, "mcp__server__read");
        let result = execution
            .execute(ExecutableToolContext {
                turn_id: 1,
                tool_call_id: "call".into(),
                trace: None,
                metadata: None,
                signal: AbortController::new().signal(),
                on_update: None,
                on_foreground_task_start: None,
            })
            .await;
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text("remote result".into())
        );
        assert!(!result.is_error);
        assert_eq!(
            client.calls.lock().unwrap().as_slice(),
            &[(
                "read".into(),
                serde_json::json!({"path": "/tmp/a"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )]
        );
    }

    #[tokio::test]
    async fn converts_client_failures_to_tool_errors() {
        struct FailingClient;
        #[async_trait]
        impl McpClient for FailingClient {
            async fn list_tools(
                &self,
            ) -> Result<Vec<McpToolDefinition>, super::super::super::McpClientError> {
                Ok(Vec::new())
            }
            async fn call_tool(
                &self,
                _name: &str,
                _args: Map<String, Value>,
                _signal: Option<crate::_base::utils::abort::AbortSignal>,
            ) -> Result<McpToolResult, super::super::super::McpClientError> {
                Err(Box::new(io::Error::other("offline")))
            }
        }
        let tool = create_mcp_tool(
            "mcp__server__read",
            Tool {
                name: "read".into(),
                description: String::new(),
                parameters: Map::new(),
                deferred: None,
            },
            Arc::new(FailingClient),
            McpToolOptions::default(),
        );
        let ToolExecution::Runnable(execution) = tool.resolve_execution(Value::Null).await else {
            panic!("MCP tool must be runnable");
        };
        let result = execution
            .execute(ExecutableToolContext {
                turn_id: 1,
                tool_call_id: "call".into(),
                trace: None,
                metadata: None,
                signal: AbortController::new().signal(),
                on_update: None,
                on_foreground_task_start: None,
            })
            .await;
        assert!(result.is_error);
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text("MCP tool \"mcp__server__read\" failed: offline".into())
        );
    }
}
