//! Built-in FetchURL tool.
//! Original: `packages/agent-core-v2/src/app/web/tools/fetch-url.ts`,
//! `FetchURLTool`.
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    _base::{di::instantiation::ServicesAccessorExt, utils::abort::AbortSignal},
    agent::tool_registry::{ToolContributionOptions, register_tool},
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolAccess, ToolExecution, ToolInputDisplay,
        input_schema::to_input_json_schema,
        result_builder::{ToolResultBuilder, ToolResultBuilderOptions},
        rule_match::{literal_rule_pattern, matches_glob_rule_subject},
    },
};

use super::{
    HttpFetchError, UrlFetchKind, UrlFetchOptions, UrlFetcherHandle, WEB_FETCH_SERVICE_ID,
};

const FETCH_URL_DESCRIPTION: &str = include_str!("fetch_url.md");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct FetchUrlInput {
    pub url: String,
}

pub fn fetch_url_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from."
                }
            },
            "required": ["url"]
        })
        .as_object()
        .cloned()
        .expect("FetchURL schema is an object"),
    )
}

pub struct FetchUrlTool {
    fetcher: UrlFetcherHandle,
    definition: Tool,
}

impl FetchUrlTool {
    pub fn new(fetcher: UrlFetcherHandle) -> Self {
        Self {
            fetcher,
            definition: Tool {
                name: "FetchURL".into(),
                description: FETCH_URL_DESCRIPTION.into(),
                parameters: fetch_url_parameters(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for FetchUrlTool {
    type Input = FetchUrlInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    // Original: FetchURLTool.resolveExecution().
    async fn resolve_execution(&self, args: FetchUrlInput) -> ToolExecution {
        let url = args.url;
        let fetcher = Arc::clone(&self.fetcher);
        let execution_url = url.clone();
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let fetcher = Arc::clone(&fetcher);
            let url = execution_url.clone();
            Box::pin(async move { execute_fetch_url(fetcher, url, context).await })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution =
            RunnableToolExecution::new(literal_rule_pattern("FetchURL", &url), execute);
        execution.accesses = Some(ToolAccess::none());
        execution.description = Some(format!("Fetching: {}", preview_url(&url)));
        execution.display = Some(ToolInputDisplay::UrlFetch {
            url: url.clone(),
            method: None,
        });
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, &url)
        }));
        ToolExecution::Runnable(execution)
    }
}

async fn execute_fetch_url(
    fetcher: UrlFetcherHandle,
    url: String,
    context: ExecutableToolContext,
) -> ExecutableToolResult {
    let bridge = FetchCancellationBridge::new(context.signal.clone());
    let result = fetcher
        .fetch(
            &url,
            Some(UrlFetchOptions {
                tool_call_id: Some(context.tool_call_id),
                cancellation: Some(bridge.token.clone()),
            }),
        )
        .await;
    drop(bridge);
    match result {
        Ok(result) if result.content.is_empty() => {
            ExecutableToolResult::success("The response body is empty.")
        }
        Ok(result) => {
            let note = match result.kind {
                UrlFetchKind::Passthrough => {
                    "The returned content is the full response body, returned verbatim."
                }
                UrlFetchKind::Extracted => {
                    "The returned content is the main text extracted from the page."
                }
            };
            let mut builder = ToolResultBuilder::new(ToolResultBuilderOptions {
                max_chars: None,
                max_line_length: Some(None),
            })
            .expect("an unlimited line length is valid");
            builder.write(&format!(
                "{note} If you use it in your answer, cite this page as a markdown link, e.g. [title](url).\n\n{}",
                result.content
            ));
            let output = builder.ok("", None);
            let mut result = ExecutableToolResult::success(output.output);
            result.truncated = output.truncated.then_some(true);
            result
        }
        Err(error) if context.signal.aborted() => {
            // The Rust executable-tool boundary returns a result rather than
            // throwing. The linked cancellation has already stopped the
            // fetch; retain the original error text as the observable result.
            ExecutableToolResult::error(error.to_string())
        }
        Err(error) => {
            if let Some(http) = error.downcast_ref::<HttpFetchError>() {
                ExecutableToolResult::error(format!(
                    "Failed to fetch URL. Status: {}. {}",
                    http.status, http.message
                ))
            } else {
                ExecutableToolResult::error(format!(
                    "Failed to fetch URL due to network error: {url}. {error}"
                ))
            }
        }
    }
}

fn preview_url(url: &str) -> String {
    let units = url.encode_utf16().collect::<Vec<_>>();
    if units.len() <= 50 {
        url.to_owned()
    } else {
        format!("{}…", String::from_utf16_lossy(&units[..50]))
    }
}

struct FetchCancellationBridge {
    token: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl FetchCancellationBridge {
    fn new(signal: AbortSignal) -> Self {
        let token = CancellationToken::new();
        if signal.aborted() {
            token.cancel();
            return Self { token, task: None };
        }
        let cancellation = token.clone();
        let task = tokio::spawn(async move {
            signal.cancelled().await;
            cancellation.cancel();
        });
        Self {
            token,
            task: Some(task),
        }
    }
}

impl Drop for FetchCancellationBridge {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

// Original: registerTool(FetchURLTool, { staticArgs: ... }).
pub fn register_fetch_url_tool() {
    register_tool(
        Arc::new(|accessor| {
            let service = accessor.get(WEB_FETCH_SERVICE_ID)?;
            Ok(Arc::new(FetchUrlTool::new(service.get_url_fetcher())))
        }),
        ToolContributionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        tool::{ExecutableToolOutput, ToolExecution},
    };

    enum StubResponse {
        Success(super::super::UrlFetchResult),
        Http { status: u16, message: String },
        Network(String),
    }

    struct StubFetcher {
        response: StubResponse,
    }

    #[async_trait]
    impl super::super::UrlFetcher for StubFetcher {
        async fn fetch(
            &self,
            _url: &str,
            _options: Option<UrlFetchOptions>,
        ) -> Result<super::super::UrlFetchResult, super::super::UrlFetchError> {
            match &self.response {
                StubResponse::Success(result) => Ok(result.clone()),
                StubResponse::Http { status, message } => Err(Box::new(HttpFetchError {
                    status: *status,
                    message: message.clone(),
                })),
                StubResponse::Network(message) => {
                    Err(Box::new(std::io::Error::other(message.clone())))
                }
            }
        }
    }

    fn context() -> ExecutableToolContext {
        ExecutableToolContext {
            turn_id: crate::agent::TurnId::new(1),
            tool_call_id: "call-1".into(),
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
            on_foreground_task_start: None,
        }
    }

    #[tokio::test]
    async fn execution_sets_source_metadata_and_formats_extracted_content() {
        let tool = FetchUrlTool::new(Arc::new(StubFetcher {
            response: StubResponse::Success(super::super::UrlFetchResult {
                content: "article".into(),
                kind: UrlFetchKind::Extracted,
            }),
        }));
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(FetchUrlInput {
                url: "https://example.com/".into(),
            })
            .await
        else {
            panic!("FetchURL should resolve to a runnable execution");
        };
        assert_eq!(execution.approval_rule, "FetchURL(https://example.com/)");
        assert_eq!(
            execution.description.as_deref(),
            Some("Fetching: https://example.com/")
        );
        assert!(execution.matches_rule("https://example.com/*"));
        let output = execution.execute(context()).await;
        assert!(!output.is_error);
        let ExecutableToolOutput::Text(output) = output.output else {
            panic!("FetchURL returns text");
        };
        assert!(
            output.starts_with("The returned content is the main text extracted from the page.")
        );
        assert!(output.ends_with("\n\narticle"));
    }

    #[tokio::test]
    async fn reports_http_and_network_failures_with_source_messages() {
        let http = FetchUrlTool::new(Arc::new(StubFetcher {
            response: StubResponse::Http {
                status: 418,
                message: "teapot".into(),
            },
        }));
        let ToolExecution::Runnable(execution) = http
            .resolve_execution(FetchUrlInput {
                url: "https://example.com/".into(),
            })
            .await
        else {
            panic!("FetchURL should resolve to a runnable execution");
        };
        let output = execution.execute(context()).await;
        assert!(output.is_error);
        let ExecutableToolOutput::Text(output) = output.output else {
            panic!("FetchURL returns text");
        };
        assert_eq!(output, "Failed to fetch URL. Status: 418. teapot");

        let network = FetchUrlTool::new(Arc::new(StubFetcher {
            response: StubResponse::Network("offline".into()),
        }));
        let ToolExecution::Runnable(execution) = network
            .resolve_execution(FetchUrlInput {
                url: "https://example.com/".into(),
            })
            .await
        else {
            panic!("FetchURL should resolve to a runnable execution");
        };
        let output = execution.execute(context()).await;
        assert!(output.is_error);
        let ExecutableToolOutput::Text(output) = output.output else {
            panic!("FetchURL returns text");
        };
        assert_eq!(
            output,
            "Failed to fetch URL due to network error: https://example.com/. offline"
        );
    }

    #[test]
    fn schema_and_preview_match_source_shape() {
        assert_eq!(fetch_url_parameters()["additionalProperties"], false);
        assert_eq!(preview_url(&"a".repeat(51)), format!("{}…", "a".repeat(50)));
    }
}
