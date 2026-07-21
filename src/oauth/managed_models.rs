use std::{error::Error, fmt};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedKimiCodeProtocol {
    Anthropic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportsThinkingType {
    Only,
    No,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedKimiCodeModelInfo {
    pub id: String,
    pub context_length: u64,
    pub supports_reasoning: bool,
    pub supports_image_in: bool,
    pub supports_video_in: bool,
    pub supports_tool_use: bool,
    pub supports_thinking_type: Option<SupportsThinkingType>,
    pub support_efforts: Option<Vec<String>>,
    pub default_effort: Option<String>,
    pub display_name: Option<String>,
    pub protocol: Option<ManagedKimiCodeProtocol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkEfforts {
    pub support_efforts: Option<Vec<String>>,
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelParseError {
    message: String,
}

impl fmt::Display for ModelParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ModelParseError {}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   parseModelProtocol()
pub fn parse_model_protocol(value: Option<&Value>) -> Option<ManagedKimiCodeProtocol> {
    (value?.as_str()? == "anthropic").then_some(ManagedKimiCodeProtocol::Anthropic)
}

// Original: parseStringArray()
pub fn parse_string_array(value: Option<&Value>) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    let parsed = values
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!parsed.is_empty()).then_some(parsed)
}

// Original: parseSupportsThinkingType()
pub fn parse_supports_thinking_type(value: Option<&Value>) -> Option<SupportsThinkingType> {
    match value?.as_str()? {
        "only" => Some(SupportsThinkingType::Only),
        "no" => Some(SupportsThinkingType::No),
        "both" => Some(SupportsThinkingType::Both),
        _ => None,
    }
}

// Original: parseThinkEfforts()
pub fn parse_think_efforts(value: Option<&Value>) -> ThinkEfforts {
    let Some(record) = value.and_then(Value::as_object) else {
        return empty_think_efforts();
    };
    if record.get("support").and_then(Value::as_bool) != Some(true) {
        return empty_think_efforts();
    }
    ThinkEfforts {
        support_efforts: parse_string_array(record.get("valid_efforts")),
        default_effort: record
            .get("default_effort")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    }
}

fn empty_think_efforts() -> ThinkEfforts {
    ThinkEfforts {
        support_efforts: None,
        default_effort: None,
    }
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts toModelInfo()
//   packages/oauth/src/open-platform.ts toModelInfo()
pub fn parse_model_info(
    item: &Value,
    model_label: &str,
) -> Result<Option<ManagedKimiCodeModelInfo>, ModelParseError> {
    let Some(record) = item.as_object() else {
        return Ok(None);
    };
    let Some(id) = record
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    let context_length =
        js_positive_integer(record.get("context_length")).ok_or_else(|| ModelParseError {
            message: format!("{model_label} \"{id}\" must include a positive context_length."),
        })?;
    let think_efforts = parse_think_efforts(record.get("think_efforts"));
    Ok(Some(ManagedKimiCodeModelInfo {
        id: id.to_owned(),
        context_length,
        supports_reasoning: js_boolean(record.get("supports_reasoning")),
        supports_image_in: js_boolean(record.get("supports_image_in")),
        supports_video_in: js_boolean(record.get("supports_video_in")),
        supports_tool_use: record
            .get("supports_tool_use")
            .is_none_or(|value| js_boolean(Some(value))),
        supports_thinking_type: parse_supports_thinking_type(record.get("supports_thinking_type")),
        support_efforts: think_efforts.support_efforts,
        default_effort: think_efforts.default_effort,
        display_name: record
            .get("display_name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        protocol: parse_model_protocol(record.get("protocol")),
    }))
}

fn js_positive_integer(value: Option<&Value>) -> Option<u64> {
    let number = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(value) if value.trim().is_empty() => 0.0,
        Value::String(value) => value.trim().parse().ok()?,
        Value::Bool(value) => u8::from(*value) as f64,
        Value::Null => 0.0,
        Value::Array(values) if values.is_empty() => 0.0,
        Value::Array(values) if values.len() == 1 => return js_positive_integer(values.first()),
        Value::Array(_) | Value::Object(_) => return None,
    };
    (number.is_finite() && number > 0.0 && number.fract() == 0.0 && number <= u64::MAX as f64)
        .then_some(number as u64)
}

fn js_boolean(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_protocol_thinking_type_and_nonempty_string_arrays() {
        assert_eq!(
            parse_model_protocol(Some(&Value::String("anthropic".to_owned()))),
            Some(ManagedKimiCodeProtocol::Anthropic)
        );
        assert_eq!(
            parse_model_protocol(Some(&Value::String("kimi".to_owned()))),
            None
        );
        for (value, expected) in [
            ("only", Some(SupportsThinkingType::Only)),
            ("no", Some(SupportsThinkingType::No)),
            ("both", Some(SupportsThinkingType::Both)),
            ("unknown", None),
        ] {
            assert_eq!(
                parse_supports_thinking_type(Some(&Value::String(value.to_owned()))),
                expected
            );
        }
        assert_eq!(
            parse_string_array(Some(&serde_json::json!(["low", "", 3, "high"]))),
            Some(vec!["low".to_owned(), "high".to_owned()])
        );
    }

    #[test]
    fn think_efforts_are_gated_by_literal_true_support() {
        let enabled = parse_think_efforts(Some(&serde_json::json!({
            "support": true,
            "valid_efforts": ["low", "high", "max"],
            "default_effort": "high"
        })));
        assert_eq!(
            enabled,
            ThinkEfforts {
                support_efforts: Some(vec!["low".to_owned(), "high".to_owned(), "max".to_owned()]),
                default_effort: Some("high".to_owned())
            }
        );
        for value in [
            Value::Null,
            serde_json::json!({ "support": false, "valid_efforts": ["low"] }),
            serde_json::json!({ "support": 1, "default_effort": "high" }),
        ] {
            assert_eq!(parse_think_efforts(Some(&value)), empty_think_efforts());
        }
    }

    #[test]
    fn parses_model_fields_with_javascript_number_and_boolean_rules() {
        let parsed = parse_model_info(
            &serde_json::json!({
                "id": "kimi-k2",
                "context_length": "256000",
                "supports_reasoning": 1,
                "supports_image_in": "false",
                "supports_video_in": 0,
                "supports_tool_use": false,
                "supports_thinking_type": "only",
                "display_name": "Kimi K2",
                "protocol": "anthropic",
                "think_efforts": {
                    "support": true,
                    "valid_efforts": ["low", "high"],
                    "default_effort": "high"
                }
            }),
            "Kimi Code model",
        )
        .expect("valid model")
        .expect("model exists");
        assert_eq!(parsed.context_length, 256_000);
        assert!(parsed.supports_reasoning);
        assert!(
            parsed.supports_image_in,
            "nonempty strings are JavaScript truthy"
        );
        assert!(!parsed.supports_video_in);
        assert!(!parsed.supports_tool_use);
        assert_eq!(
            parsed.supports_thinking_type,
            Some(SupportsThinkingType::Only)
        );
        assert_eq!(parsed.protocol, Some(ManagedKimiCodeProtocol::Anthropic));
    }

    #[test]
    fn skips_missing_ids_and_rejects_non_positive_or_fractional_context() {
        assert_eq!(
            parse_model_info(&serde_json::json!({ "context_length": 1 }), "")
                .expect("missing id is skipped"),
            None
        );
        for context in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
        ] {
            let error = parse_model_info(
                &serde_json::json!({ "id": "bad", "context_length": context }),
                "Kimi Code model",
            )
            .expect_err("invalid context");
            assert_eq!(
                error.to_string(),
                "Kimi Code model \"bad\" must include a positive context_length."
            );
        }
    }
}
