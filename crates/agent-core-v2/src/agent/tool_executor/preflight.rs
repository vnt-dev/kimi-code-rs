//! Tool-call argument parsing used by executor preflight.
//!
//! Original: `toolExecutorService.ts`, `parseToolCallArguments()`.

use serde_json::{Map, Value};

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
