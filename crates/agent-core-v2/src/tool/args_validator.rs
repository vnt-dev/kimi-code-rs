use jsonschema::{Draft, Validator};
use serde_json::Value;

const DRAFT_2019_KEYWORDS: &[&str] = &[
    "dependentRequired",
    "dependentSchemas",
    "maxContains",
    "minContains",
    "unevaluatedItems",
    "unevaluatedProperties",
    "$recursiveAnchor",
    "$recursiveRef",
];
const DRAFT_2020_KEYWORDS: &[&str] = &["prefixItems", "$dynamicAnchor", "$dynamicRef"];

pub struct ToolArgsValidator {
    validator: Validator,
    schema: Value,
}

#[derive(Debug, thiserror::Error)]
#[error("failed to compile tool parameter schema: {message}")]
pub struct ToolArgsSchemaError {
    message: String,
}

// Original: packages/agent-core-v2/src/tool/args-validator.ts, compileToolArgsValidator().
pub fn compile_tool_args_validator(
    schema: &Value,
) -> Result<ToolArgsValidator, ToolArgsSchemaError> {
    let draft = draft_for(schema);
    let validator = jsonschema::options()
        .with_draft(draft)
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| ToolArgsSchemaError {
            message: error.to_string(),
        })?;
    Ok(ToolArgsValidator {
        validator,
        schema: schema.clone(),
    })
}

pub fn validate_tool_args(validator: &ToolArgsValidator, arguments: &Value) -> Option<String> {
    let errors = validator
        .validator
        .iter_errors(arguments)
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return None;
    }
    Some(
        errors
            .iter()
            .flat_map(|error| format_validation_error(error, &validator.schema))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn draft_for(schema: &Value) -> Draft {
    if let Some(schema_uri) = schema.get("$schema").and_then(Value::as_str) {
        if schema_uri.contains("2020-12") {
            return Draft::Draft202012;
        }
        if schema_uri.contains("2019-09") {
            return Draft::Draft201909;
        }
        return Draft::Draft7;
    }
    if contains_schema_keyword(schema, DRAFT_2020_KEYWORDS) {
        Draft::Draft202012
    } else if contains_schema_keyword(schema, DRAFT_2019_KEYWORDS) {
        Draft::Draft201909
    } else {
        Draft::Draft7
    }
}

fn contains_schema_keyword(value: &Value, keywords: &[&str]) -> bool {
    match value {
        Value::Array(values) => values
            .iter()
            .any(|value| contains_schema_keyword(value, keywords)),
        Value::Object(values) => values.iter().any(|(key, value)| {
            keywords.contains(&key.as_str()) || contains_schema_keyword(value, keywords)
        }),
        _ => false,
    }
}

fn format_validation_error(error: &jsonschema::ValidationError<'_>, schema: &Value) -> Vec<String> {
    let path = error.instance_path().to_string();
    let display = error.to_string();
    let schema_path = error.schema_path().to_string();
    let keyword = schema_path.rsplit('/').next().unwrap_or_default();
    if keyword == "required"
        && let Some(property) = quoted_fragments(&display).into_iter().next()
    {
        return vec![format!("must have required property '{property}'")];
    }
    if keyword == "additionalProperties" {
        let properties = quoted_fragments(&display);
        if !properties.is_empty() {
            return properties
                .into_iter()
                .map(|property| format!("must NOT have additional property '{property}'"))
                .collect();
        }
    }
    let schema_value = schema.pointer(&schema_path);
    let message = match keyword {
        "type" => schema_value.map_or(display.clone(), |expected| match expected {
            Value::String(expected) => format!("must be {expected}"),
            Value::Array(expected) => format!(
                "must be {}",
                expected
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            _ => display.clone(),
        }),
        "enum" => "must be equal to one of the allowed values".into(),
        "const" => "must be equal to constant".into(),
        "format" => schema_value
            .and_then(Value::as_str)
            .map_or(display.clone(), |format| {
                format!("must match format \"{format}\"")
            }),
        "minLength" => schema_value
            .and_then(Value::as_u64)
            .map_or(display.clone(), |limit| {
                format!("must NOT have fewer than {limit} characters")
            }),
        "maxLength" => schema_value
            .and_then(Value::as_u64)
            .map_or(display.clone(), |limit| {
                format!("must NOT have more than {limit} characters")
            }),
        _ => display,
    };
    if path.is_empty() {
        vec![message]
    } else {
        vec![format!("{path} {message}")]
    }
}

fn quoted_fragments(message: &str) -> Vec<&str> {
    let quote = if message.contains('\'') { '\'' } else { '"' };
    let mut fragments = Vec::new();
    let mut remainder = message;
    while let Some(start) = remainder.find(quote) {
        remainder = &remainder[start + quote.len_utf8()..];
        let Some(end) = remainder.find(quote) else {
            break;
        };
        fragments.push(&remainder[..end]);
        remainder = &remainder[end + quote.len_utf8()..];
    }
    fragments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_required_additional_and_nested_failures_for_the_model() {
        let validator = compile_tool_args_validator(&serde_json::json!({
            "type": "object",
            "required": ["path"],
            "additionalProperties": false,
            "properties": {"path": {"type": "string", "minLength": 2}}
        }))
        .unwrap();
        let errors = validate_tool_args(&validator, &serde_json::json!({"extra": true})).unwrap();
        assert!(errors.contains("must have required property 'path'"));
        assert!(errors.contains("must NOT have additional property 'extra'"));
        let nested = validate_tool_args(&validator, &serde_json::json!({"path": "x"})).unwrap();
        assert!(nested.starts_with("/path "));
    }

    #[test]
    fn detects_newer_drafts_by_keywords_and_validates_formats() {
        let draft_2020 = compile_tool_args_validator(&serde_json::json!({
            "type": "array",
            "prefixItems": [{"type": "string"}]
        }))
        .unwrap();
        assert!(validate_tool_args(&draft_2020, &serde_json::json!([1])).is_some());

        let format = compile_tool_args_validator(&serde_json::json!({
            "type": "string",
            "format": "email"
        }))
        .unwrap();
        assert!(validate_tool_args(&format, &serde_json::json!("not-an-email")).is_some());
    }

    #[test]
    fn preserves_ajv_keyword_vocabulary() {
        let cases = [
            (
                serde_json::json!({"type":"integer"}),
                serde_json::json!(1.5),
                "must be integer",
            ),
            (
                serde_json::json!({"enum":["a","b"]}),
                serde_json::json!("c"),
                "allowed values",
            ),
            (
                serde_json::json!({"const":"x"}),
                serde_json::json!("y"),
                "constant",
            ),
        ];
        for (schema, value, expected) in cases {
            let error =
                validate_tool_args(&compile_tool_args_validator(&schema).unwrap(), &value).unwrap();
            assert!(error.contains(expected), "{error}");
        }
    }
}
