//! Built-in authenticated `WebSearch` tool.
//!
//! Original: `app/auth/webSearch/tools/web-search.ts`.

use std::{error::Error, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    _base::di::{errors::DiError, instantiation::ServicesAccessorExt},
    agent::tool_registry::{ToolContributionOptions, register_tool},
    app::auth::web_search::{
        WEB_SEARCH_PROVIDER_SERVICE_ID, WebSearchOptions, WebSearchProviderHandle, WebSearchResult,
    },
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolAccess, ToolExecution, ToolInputDisplay,
        input_schema::to_input_json_schema,
        result_builder::{ToolResultBuilder, ToolResultBuilderOptions},
        rule_match::{literal_rule_pattern, matches_glob_rule_subject},
    },
};

const WEB_SEARCH_DESCRIPTION: &str = include_str!("web-search.md");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct WebSearchInput {
    pub query: String,
}

pub fn web_search_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The query text to search for."
                }
            },
            "required": ["query"]
        })
        .as_object()
        .cloned()
        .expect("WebSearch schema is an object"),
    )
}

pub struct WebSearchTool {
    provider: WebSearchProviderHandle,
    definition: Tool,
}

impl WebSearchTool {
    pub fn new(provider: WebSearchProviderHandle) -> Self {
        Self {
            provider,
            definition: Tool {
                name: "WebSearch".into(),
                description: WEB_SEARCH_DESCRIPTION.into(),
                parameters: web_search_parameters(),
                deferred: None,
            },
        }
    }
}

#[async_trait]
impl ExecutableTool for WebSearchTool {
    type Input = WebSearchInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: WebSearchInput) -> ToolExecution {
        let query = args.query;
        let provider = Arc::clone(&self.provider);
        let execution_query = query.clone();
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let provider = Arc::clone(&provider);
            let query = execution_query.clone();
            Box::pin(async move { execute_web_search(provider, query, context).await })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution =
            RunnableToolExecution::new(literal_rule_pattern("WebSearch", &query), execute);
        execution.accesses = Some(ToolAccess::none());
        execution.description = Some(format!("Searching: {}", preview_query(&query)));
        execution.display = Some(ToolInputDisplay::Search {
            query: query.clone(),
            scope: None,
        });
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, &query)
        }));
        ToolExecution::Runnable(execution)
    }
}

async fn execute_web_search(
    provider: WebSearchProviderHandle,
    query: String,
    context: ExecutableToolContext,
) -> ExecutableToolResult {
    match provider
        .search(
            &query,
            Some(WebSearchOptions {
                tool_call_id: Some(context.tool_call_id),
                signal: Some(context.signal.clone()),
            }),
        )
        .await
    {
        Ok(results) => format_search_results(&results),
        Err(error) if context.signal.aborted() => {
            // The Rust executable-tool boundary returns results rather than
            // throwing. The provider has already observed the same signal.
            ExecutableToolResult::error(error.to_string())
        }
        Err(error) => ExecutableToolResult::error(classify_search_error(error.as_ref())),
    }
}

fn format_search_results(results: &[WebSearchResult]) -> ExecutableToolResult {
    let mut builder = ToolResultBuilder::new(ToolResultBuilderOptions {
        max_chars: None,
        max_line_length: Some(None),
    })
    .expect("an unlimited line length is valid");
    if results.is_empty() {
        builder.write("No search results found.");
    } else {
        for (index, result) in results.iter().enumerate() {
            if index > 0 {
                builder.write("---\n\n");
            }
            builder.write(&format!("Title: {}\n", result.title));
            if let Some(site_name) = result
                .site_name
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                builder.write(&format!("Site: {site_name}\n"));
            }
            if let Some(date) = result.date.as_deref().filter(|value| !value.is_empty()) {
                builder.write(&format!("Date: {date}\n"));
            }
            builder.write(&format!("URL: {}\n", result.url));
            builder.write(&format!("Snippet: {}\n\n", result.snippet));
        }
        builder.write(
            "When you rely on a result in your answer, cite it inline as a markdown link, e.g. [title](url).",
        );
    }
    let output = builder.ok("", None);
    let mut result = ExecutableToolResult::success(output.output);
    result.truncated = output.truncated.then_some(true);
    result
}

fn classify_search_error(error: &(dyn Error + Send + Sync + 'static)) -> String {
    let message = error.to_string();
    let lower = message.to_lowercase();
    if lower.contains("abort") || lower.contains("cancel") {
        return format!("Search cancelled: {message}");
    }
    if error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(reqwest::Error::is_timeout)
        || error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::TimedOut)
        || lower.contains("timed out")
        || lower.contains("timeout")
    {
        return format!("Search timed out: {message}");
    }
    if lower.contains("401") || lower.contains("unauthorized") || lower.contains("auth") {
        return format!("Search failed (authentication): {message}");
    }
    if error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(|error| !error.is_decode())
        || lower.contains("http ")
        || lower.contains("network")
        || lower.contains("fetch")
    {
        return format!("Search failed (network): {message}");
    }
    format!("Search failed: {message}")
}

fn preview_query(query: &str) -> String {
    utf16_preview(query, 40)
}

fn utf16_preview(value: &str, limit: usize) -> String {
    let units = value.encode_utf16().collect::<Vec<_>>();
    if units.len() <= limit {
        value.to_owned()
    } else {
        format!("{}…", String::from_utf16_lossy(&units[..limit]))
    }
}

pub fn register_web_search_tool() {
    register_tool(
        Arc::new(|accessor| {
            let service = accessor.get(WEB_SEARCH_PROVIDER_SERVICE_ID)?;
            let provider = service.get_web_search_provider().ok_or_else(|| {
                DiError::Factory(
                    "WebSearchProviderService returned no provider during tool registration."
                        .into(),
                )
            })?;
            Ok(Arc::new(WebSearchTool::new(provider)))
        }),
        ToolContributionOptions {
            when: Some(Arc::new(|accessor| {
                accessor
                    .get(WEB_SEARCH_PROVIDER_SERVICE_ID)
                    .ok()
                    .and_then(|service| service.get_web_search_provider())
                    .is_some()
            })),
            ..ToolContributionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        app::auth::web_search::{WebSearchError, WebSearchOptions, WebSearchProvider},
        tool::ExecutableToolOutput,
    };

    struct StubProvider;

    #[async_trait]
    impl WebSearchProvider for StubProvider {
        async fn search(
            &self,
            query: &str,
            options: Option<WebSearchOptions>,
        ) -> Result<Vec<WebSearchResult>, WebSearchError> {
            assert_eq!(query, "rust async");
            assert_eq!(
                options
                    .as_ref()
                    .and_then(|value| value.tool_call_id.as_deref()),
                Some("call-1")
            );
            Ok(vec![WebSearchResult {
                title: "Async Rust".into(),
                url: "https://example.test/rust".into(),
                snippet: "A guide".into(),
                date: Some("2026-07-27".into()),
                site_name: Some("Example".into()),
            }])
        }
    }

    #[tokio::test]
    async fn resolves_display_rules_and_formats_provider_results() {
        let tool = WebSearchTool::new(Arc::new(StubProvider));
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(WebSearchInput {
                query: "rust async".into(),
            })
            .await
        else {
            panic!("WebSearch must resolve to a runnable execution");
        };
        assert_eq!(
            execution.description.as_deref(),
            Some("Searching: rust async")
        );
        assert!(matches!(
            execution.display,
            Some(ToolInputDisplay::Search { ref query, scope: None }) if query == "rust async"
        ));
        assert!(execution.matches_rule("rust *"));
        let result = execution
            .execute(ExecutableToolContext {
                turn_id: crate::agent::TurnId::new(1),
                tool_call_id: "call-1".into(),
                trace: None,
                metadata: None,
                signal: AbortController::new().signal(),
                on_update: None,
                on_foreground_task_start: None,
            })
            .await;
        assert!(!result.is_error);
        assert!(matches!(
            result.output,
            ExecutableToolOutput::Text(ref output)
                if output.contains("Title: Async Rust")
                    && output.contains("Site: Example")
                    && output.contains("[title](url)")
        ));
    }

    #[test]
    fn empty_results_errors_and_utf16_preview_match_source_behavior() {
        assert!(matches!(
            format_search_results(&[]).output,
            ExecutableToolOutput::Text(ref output) if output == "No search results found."
        ));
        let authentication = std::io::Error::other("HTTP 401 unauthorized");
        assert!(
            classify_search_error(&authentication).starts_with("Search failed (authentication):")
        );
        let timeout = std::io::Error::new(std::io::ErrorKind::TimedOut, "request timed out");
        assert!(classify_search_error(&timeout).starts_with("Search timed out:"));
        assert_eq!(
            preview_query(&"a".repeat(41)),
            format!("{}…", "a".repeat(40))
        );
    }
}
