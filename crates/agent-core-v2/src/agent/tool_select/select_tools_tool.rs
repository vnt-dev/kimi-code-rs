//! Progressive-disclosure schema loading tool.
//!
//! Original: `agent/toolSelect/tools/select-tools.ts`.

use std::{sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::di::instantiation::ServicesAccessorExt,
    agent::tool_registry::{ToolContributionOptions, register_tool},
    kosong::contract::tool::Tool,
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema,
    },
};

use super::{
    AGENT_TOOL_SELECT_SERVICE_ID, AgentToolSelectServiceHandle, LoadToolsResult,
    SELECT_TOOLS_TOOL_NAME,
};

const SELECT_TOOLS_DESCRIPTION: &str = "Load one or more tools by name so you can call them. \
All available tool names are listed in the <tools_added>/<tools_removed> announcements \
in the system context — fold them in order to get the current list. \
Pass the exact name(s) you need; their full definitions become available immediately, \
so you can call them directly in your next tool call.";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SelectToolsInput {
    #[serde(deserialize_with = "deserialize_names")]
    pub names: Vec<String>,
}

fn deserialize_names<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<Value>::deserialize(deserializer)
        .map_err(|_| serde::de::Error::custom("names must be a non-empty array"))?;
    let mut names = Vec::with_capacity(values.len());
    for value in values {
        match value {
            Value::String(name) => names.push(name),
            _ => return Err(serde::de::Error::custom("names must contain only strings")),
        }
    }
    if names.is_empty() {
        return Err(serde::de::Error::custom("names must be a non-empty array"));
    }
    Ok(names)
}

pub fn parse_select_tools_input(value: &Value) -> Result<SelectToolsInput, String> {
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

pub static SELECT_TOOLS_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "names": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 1,
                    "description": "Exact tool names to load, taken from the latest announced tool list."
                }
            },
            "required": ["names"],
            "additionalProperties": false
        })
        .as_object()
        .cloned()
        .expect("select_tools schema is an object"),
    )
});

pub trait SelectToolsProvider: Send + Sync {
    fn enabled(&self) -> bool;
    fn load(&self, names: Vec<String>) -> LoadToolsResult;
}

impl SelectToolsProvider for AgentToolSelectServiceHandle {
    fn enabled(&self) -> bool {
        (**self).enabled()
    }

    fn load(&self, names: Vec<String>) -> LoadToolsResult {
        (**self).load(names)
    }
}

pub struct SelectToolsTool {
    tool_select: Arc<dyn SelectToolsProvider>,
    definition: Tool,
}

impl SelectToolsTool {
    pub fn new(tool_select: Arc<dyn SelectToolsProvider>) -> Self {
        Self {
            tool_select,
            definition: Tool {
                name: SELECT_TOOLS_TOOL_NAME.into(),
                description: SELECT_TOOLS_DESCRIPTION.into(),
                parameters: SELECT_TOOLS_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }

    pub fn from_service(tool_select: AgentToolSelectServiceHandle) -> Self {
        Self::new(Arc::new(tool_select))
    }
}

#[async_trait]
impl ExecutableTool for SelectToolsTool {
    type Input = SelectToolsInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    // Original: SelectToolsTool.resolveExecution(). This is async to satisfy
    // the executable-tool contract; its work remains synchronous.
    async fn resolve_execution(&self, input: SelectToolsInput) -> ToolExecution {
        let description = format!("Loading {}", input.names.join(", "));
        let tool_select = Arc::clone(&self.tool_select);
        let names = input.names;
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let tool_select = Arc::clone(&tool_select);
            let names = names.clone();
            Box::pin(async move {
                if !tool_select.enabled() {
                    return ExecutableToolResult::error(
                        "select_tools is not available for the current model.",
                    );
                }
                let LoadToolsResult {
                    to_load,
                    already_available,
                    unknown,
                } = tool_select.load(names);
                let is_error = to_load.is_empty() && already_available.is_empty();
                let mut lines = Vec::new();
                if !to_load.is_empty() {
                    lines.push(format!("Loaded: {}", to_load.join(", ")));
                }
                if !already_available.is_empty() {
                    lines.push(format!(
                        "Already available: {}",
                        already_available.join(", ")
                    ));
                }
                lines.extend(unknown.into_iter().map(|name| {
                    format!("Unknown tool: {name}. Pick from the latest announced tools list.")
                }));
                let output = lines.join("\n");
                if is_error {
                    ExecutableToolResult::error(output)
                } else {
                    ExecutableToolResult::success(output)
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new(SELECT_TOOLS_TOOL_NAME, execute);
        execution.description = Some(description);
        ToolExecution::Runnable(execution)
    }
}

// Original: registerTool(SelectToolsTool).
pub fn register_select_tools_tool() {
    register_tool(
        Arc::new(|accessor| {
            let tool_select = accessor.get(AGENT_TOOL_SELECT_SERVICE_ID)?;
            Ok(Arc::new(SelectToolsTool::from_service(
                (*tool_select).clone(),
            )))
        }),
        ToolContributionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        tool::{ExecutableToolOutput, ToolExecution},
    };

    struct StubToolSelect {
        enabled: bool,
        result: LoadToolsResult,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl SelectToolsProvider for StubToolSelect {
        fn enabled(&self) -> bool {
            self.enabled
        }

        fn load(&self, names: Vec<String>) -> LoadToolsResult {
            self.calls.lock().push(names);
            self.result.clone()
        }
    }

    fn execution_context() -> ExecutableToolContext {
        ExecutableToolContext {
            turn_id: crate::agent::TurnId::new(1),
            tool_call_id: "call".into(),
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
            on_foreground_task_start: None,
        }
    }

    #[test]
    fn accepts_only_non_empty_string_name_arrays() {
        assert!(parse_select_tools_input(&json!({"names": ["one"]})).is_ok());
        assert!(parse_select_tools_input(&json!({"names": []})).is_err());
        assert!(parse_select_tools_input(&json!({"names": [1]})).is_err());
        assert!(parse_select_tools_input(&json!({"names": ["one"], "extra": true})).is_err());
    }

    #[tokio::test]
    async fn reports_loaded_available_and_unknown_names_in_source_order() {
        let provider = Arc::new(StubToolSelect {
            enabled: true,
            result: LoadToolsResult {
                to_load: vec!["alpha".into()],
                already_available: vec!["beta".into()],
                unknown: vec!["gamma".into()],
            },
            calls: Mutex::new(Vec::new()),
        });
        let tool = SelectToolsTool::new(provider.clone());
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(SelectToolsInput {
                names: vec!["alpha".into(), "beta".into(), "gamma".into()],
            })
            .await
        else {
            panic!("select_tools must be runnable");
        };
        assert_eq!(
            execution.description.as_deref(),
            Some("Loading alpha, beta, gamma")
        );
        let result = execution.execute(execution_context()).await;
        assert!(!result.is_error);
        assert_eq!(
            result.output,
            ExecutableToolOutput::Text(
                "Loaded: alpha\nAlready available: beta\nUnknown tool: gamma. Pick from the latest announced tools list."
                    .into()
            )
        );
        assert_eq!(
            provider.calls.lock().as_slice(),
            &[vec![
                "alpha".to_owned(),
                "beta".to_owned(),
                "gamma".to_owned()
            ]]
        );
    }
}
