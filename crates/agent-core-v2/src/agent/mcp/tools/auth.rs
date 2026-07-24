//! Synthetic MCP OAuth authentication tool.
//!
//! Original: `agent/mcp/tools/auth.ts`, `createMcpAuthTool`.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use crate::{
    agent::mcp::{
        AlreadyAuthorizedError, McpOAuthService, McpOAuthServiceError, qualify_mcp_tool_name,
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, ToolUpdate, ToolUpdateKind,
    },
};

pub const MCP_OAUTH_AUTHORIZATION_URL_TOOL_UPDATE: &str = "mcp.oauth.authorization_url";
pub const DEFAULT_AUTH_TIMEOUT_MS: u64 = 15 * 60 * 1000;

pub type McpReconnect = Arc<
    dyn Fn(crate::_base::utils::abort::AbortSignal) -> BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct McpAuthToolOptions {
    pub server_name: String,
    pub server_url: String,
    pub oauth_service: Arc<McpOAuthService>,
    pub reconnect: McpReconnect,
    pub timeout_ms: Option<u64>,
}

pub struct McpAuthTool {
    definition: Tool,
    options: McpAuthToolOptions,
}

pub fn create_mcp_auth_tool(options: McpAuthToolOptions) -> McpAuthTool {
    let name = qualify_mcp_tool_name(&options.server_name, "authenticate");
    let description = format!(
        "Authenticate with MCP server \"{}\" via OAuth.\n\nThis server requires an OAuth login that has not yet been completed. Calling this tool starts the authorization flow and waits up to 15 minutes for the browser callback. Take no arguments. Treat the authorization URL as sensitive and show it to the user verbatim.",
        options.server_name
    );
    McpAuthTool {
        definition: Tool {
            name,
            description,
            parameters: Map::new(),
            deferred: None,
        },
        options,
    }
}

#[async_trait]
impl ExecutableTool for McpAuthTool {
    type Input = Value;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, _args: Value) -> ToolExecution {
        let options = self.options.clone();
        let approval_rule = self.definition.name.clone();
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let options = options.clone();
            Box::pin(async move { execute_authentication(options, context).await })
                as BoxFuture<'static, ExecutableToolResult>
        });
        ToolExecution::Runnable(RunnableToolExecution::new(approval_rule, execute))
    }
}

async fn execute_authentication(
    options: McpAuthToolOptions,
    context: ExecutableToolContext,
) -> ExecutableToolResult {
    if let Err(error) = context.signal.throw_if_aborted() {
        return ExecutableToolResult::error(error.to_string());
    }
    status(
        &context,
        format!("Discovering OAuth metadata for {}…", options.server_name),
    );
    let flow = match options
        .oauth_service
        .begin_authorization(&options.server_name, &options.server_url, None)
        .await
    {
        Ok(flow) => flow,
        Err(McpOAuthServiceError::AlreadyAuthorized(AlreadyAuthorizedError { .. })) => {
            status(
                &context,
                format!("Already authorized; reconnecting {}…", options.server_name),
            );
            return match (options.reconnect)(context.signal.clone()).await {
                Ok(()) => ExecutableToolResult::success(format!(
                    "MCP server \"{}\" already had valid OAuth credentials. Reconnected; real tools are available now.",
                    options.server_name
                )),
                Err(error) => error_result(&options.server_name, error, None),
            };
        }
        Err(error) => return error_result(&options.server_name, error.to_string(), None),
    };
    let url = flow.authorization_url.to_string();
    update(
        &context,
        ToolUpdate {
            kind: ToolUpdateKind::Custom,
            text: None,
            percent: None,
            custom_kind: Some(MCP_OAUTH_AUTHORIZATION_URL_TOOL_UPDATE.into()),
            custom_data: Some(serde_json::json!({
                "serverName": options.server_name,
                "authorizationUrl": url,
            })),
        },
    );
    status(
        &context,
        format!(
            "Open this URL in your browser to authorize \"{}\":\n\n{}\n\nWaiting for the OAuth callback (timeout 15 min). If you cancel, call this tool again to restart the flow.",
            options.server_name, url
        ),
    );
    if let Err(error) = flow
        .complete(
            Some(context.signal.clone()),
            options.timeout_ms.or(Some(DEFAULT_AUTH_TIMEOUT_MS)),
        )
        .await
    {
        return error_result(&options.server_name, error.to_string(), Some(&url));
    }
    status(
        &context,
        format!("Authorized — reconnecting {}…", options.server_name),
    );
    match (options.reconnect)(context.signal).await {
        Ok(()) => ExecutableToolResult::success(format!(
            "MCP server \"{}\" authenticated successfully. The real MCP tools have replaced this synthetic authenticate tool.",
            options.server_name
        )),
        Err(error) => error_result(&options.server_name, error, None),
    }
}

fn status(context: &ExecutableToolContext, text: String) {
    update(
        context,
        ToolUpdate {
            kind: ToolUpdateKind::Status,
            text: Some(text),
            percent: None,
            custom_kind: None,
            custom_data: None,
        },
    );
}

fn update(context: &ExecutableToolContext, update: ToolUpdate) {
    if let Some(on_update) = &context.on_update {
        on_update(update);
    }
}

fn error_result(
    server_name: &str,
    error: String,
    authorization_url: Option<&str>,
) -> ExecutableToolResult {
    let suffix = authorization_url.map_or_else(String::new, |url| {
        format!("\n\nAuthorization URL (still valid if the listener has not timed out): {url}")
    });
    ExecutableToolResult::error(format!(
        "OAuth flow for MCP server \"{server_name}\" did not complete: {error}{suffix}"
    ))
}
