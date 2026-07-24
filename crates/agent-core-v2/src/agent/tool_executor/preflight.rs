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
    use super::*;

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
}
