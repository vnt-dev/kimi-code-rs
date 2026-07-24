//! Tool-call argument parsing used by executor preflight.
//!
//! Original: `toolExecutorService.ts`, `parseToolCallArguments()`.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::{
    agent::{
        tool_executor::{
            MissingToolDescriber, ToolCallGuard, ToolCallGuardInput, UnavailableToolDescriber,
        },
        tool_registry::AgentToolRegistryServiceContract,
    },
    kosong::contract::message::ToolCall,
    tool::{
        ErasedExecutableTool, ToolSource,
        args_validator::{compile_tool_args_validator, validate_tool_args},
    },
};

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedToolCallArguments {
    pub data: Value,
    pub parse_failed: bool,
    pub error: Option<String>,
}

// Invalid JSON is deliberately not a throwing error: the source logs it and
// continues preflight with `{}`, so validation supplies the visible result.
pub fn parse_tool_call_arguments(raw: Option<&str>) -> ParsedToolCallArguments {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return ParsedToolCallArguments {
            data: Value::Object(Map::new()),
            parse_failed: false,
            error: None,
        };
    };
    match serde_json::from_str(raw) {
        Ok(data) => ParsedToolCallArguments {
            data,
            parse_failed: false,
            error: None,
        },
        Err(error) => ParsedToolCallArguments {
            data: Value::Object(Map::new()),
            parse_failed: true,
            error: Some(error.to_string()),
        },
    }
}

pub enum PreflightedToolCall {
    Runnable {
        tool_call: ToolCall,
        tool_name: String,
        tool: Arc<dyn ErasedExecutableTool>,
        args: Value,
    },
    Rejected {
        tool_call: ToolCall,
        tool_name: String,
        args: Value,
        output: String,
    },
}

// Original: toolExecutorService.ts, preflightToolCall(). Logging of malformed
// JSON remains the responsibility of the stateful executor service.
pub fn preflight_tool_call(
    tool_registry: &dyn AgentToolRegistryServiceContract,
    tool_call: ToolCall,
    guard: Option<&ToolCallGuard>,
    describe_unavailable: Option<&UnavailableToolDescriber>,
    describe_missing: Option<&MissingToolDescriber>,
) -> PreflightedToolCall {
    let tool_name = tool_call.name.clone();
    let args = parse_tool_call_arguments(tool_call.arguments.as_deref()).data;
    let Some(tool) = tool_registry.resolve(&tool_name) else {
        return PreflightedToolCall::Rejected {
            tool_call,
            tool_name: tool_name.clone(),
            args,
            output: describe_missing
                .and_then(|describe| describe(&tool_name))
                .unwrap_or_else(|| format!("Tool \"{tool_name}\" not found")),
        };
    };
    let source = tool_registry
        .list()
        .into_iter()
        .find(|entry| entry.name == tool_name)
        .map(|entry| entry.source)
        .unwrap_or(ToolSource::Builtin);
    if let Some(output) = guard.and_then(|guard| {
        guard(&ToolCallGuardInput {
            name: tool_name.clone(),
            source,
        })
    }) {
        return PreflightedToolCall::Rejected {
            tool_call,
            tool_name,
            args,
            output,
        };
    }
    if let Some(output) = describe_unavailable.and_then(|describe| describe(&tool_name)) {
        return PreflightedToolCall::Rejected {
            tool_call,
            tool_name,
            args,
            output,
        };
    }
    let validation =
        match compile_tool_args_validator(&Value::Object(tool.tool().parameters.clone())) {
            Ok(validator) => validate_tool_args(&validator, &args),
            Err(error) => Some(error.to_string()),
        };
    if let Some(error) = validation {
        return PreflightedToolCall::Rejected {
            tool_call,
            tool_name: tool_name.clone(),
            args,
            output: format!("Invalid args for tool \"{tool_name}\": {error}"),
        };
    }
    PreflightedToolCall::Runnable {
        tool_call,
        tool_name,
        tool,
        args,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::{
        agent::tool_registry::{
            AgentToolRegistryService, AgentToolRegistryServiceContract, ToolRegistrationOptions,
        },
        kosong::contract::{message::ToolCallType, tool::Tool},
        tool::{ExecutableTool, ToolExecution},
    };

    struct TestTool {
        definition: Tool,
    }

    #[async_trait]
    impl ExecutableTool for TestTool {
        type Input = Value;

        fn tool(&self) -> &Tool {
            &self.definition
        }

        async fn resolve_execution(&self, _input: Self::Input) -> ToolExecution {
            ToolExecution::Error(crate::tool::ExecutableToolResult::success("unused"))
        }
    }

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            call_type: ToolCallType::Function,
            id: "call-1".into(),
            name: name.into(),
            arguments: Some(arguments.into()),
            extras: None,
            stream_index: None,
        }
    }

    fn registry() -> AgentToolRegistryService {
        let registry = AgentToolRegistryService::new();
        registry.register(
            Arc::new(TestTool {
                definition: Tool {
                    name: "Read".into(),
                    description: "Reads a file".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "required": ["path"],
                        "additionalProperties": false,
                        "properties": {"path": {"type": "string"}},
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                    deferred: None,
                },
            }),
            ToolRegistrationOptions {
                source: Some(ToolSource::User),
            },
        );
        registry
    }

    #[test]
    fn parser_preserves_empty_and_invalid_json_fallbacks() {
        assert_eq!(parse_tool_call_arguments(None).data, serde_json::json!({}));
        assert_eq!(
            parse_tool_call_arguments(Some("")).data,
            serde_json::json!({})
        );
        assert_eq!(
            parse_tool_call_arguments(Some("[1]")).data,
            serde_json::json!([1])
        );
        let invalid = parse_tool_call_arguments(Some("{bad"));
        assert!(invalid.parse_failed);
        assert_eq!(invalid.data, serde_json::json!({}));
        assert!(invalid.error.is_some());
    }

    #[test]
    fn preflight_preserves_rejection_precedence_and_runnable_values() {
        let registry = registry();
        let missing: MissingToolDescriber = Arc::new(|name| Some(format!("missing: {name}")));
        let missing =
            preflight_tool_call(&registry, call("Missing", "{}"), None, None, Some(&missing));
        assert!(matches!(
            missing,
            PreflightedToolCall::Rejected { output, args, .. }
                if output == "missing: Missing" && args == serde_json::json!({})
        ));

        let guard: ToolCallGuard =
            Arc::new(|input| (input.source == ToolSource::User).then_some("guard rejected".into()));
        let guarded = preflight_tool_call(
            &registry,
            call("Read", r#"{"path":"a.txt"}"#),
            Some(&guard),
            None,
            None,
        );
        assert!(matches!(
            guarded,
            PreflightedToolCall::Rejected { output, .. } if output == "guard rejected"
        ));

        let unavailable: UnavailableToolDescriber =
            Arc::new(|_| Some("temporarily unavailable".into()));
        let unavailable = preflight_tool_call(
            &registry,
            call("Read", r#"{"path":"a.txt"}"#),
            None,
            Some(&unavailable),
            None,
        );
        assert!(matches!(
            unavailable,
            PreflightedToolCall::Rejected { output, .. } if output == "temporarily unavailable"
        ));

        let invalid = preflight_tool_call(&registry, call("Read", "{}"), None, None, None);
        assert!(matches!(
            invalid,
            PreflightedToolCall::Rejected { output, .. }
                if output.starts_with("Invalid args for tool \"Read\": must have required property 'path'")
        ));

        let runnable = preflight_tool_call(
            &registry,
            call("Read", r#"{"path":"a.txt"}"#),
            None,
            None,
            None,
        );
        assert!(matches!(
            runnable,
            PreflightedToolCall::Runnable { tool_name, args, .. }
                if tool_name == "Read" && args == serde_json::json!({"path": "a.txt"})
        ));
    }
}
