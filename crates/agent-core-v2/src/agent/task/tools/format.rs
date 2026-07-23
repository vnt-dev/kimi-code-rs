//! Plain-text object formatting shared by task tools.
//!
//! Original: `packages/agent-core-v2/src/agent/task/tools/format.ts`.

use serde_json::{Map, Number, Value};

// Original: format.ts, formatPlainObject(). Serde JSON objects preserve their
// insertion order, matching Object.entries(). JSON null maps to the source's
// null filter; JavaScript undefined is represented by an absent map entry.
pub fn format_plain_object(record: &Map<String, Value>) -> String {
    record
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| format!("{}: {}", field_name(key), format_value(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

// Original: format.ts, fieldName(). Its /[A-Z]/ expression is deliberately
// ASCII-only rather than a general Unicode case conversion.
fn field_name(key: &str) -> String {
    let mut output = String::with_capacity(key.len());
    for character in key.chars() {
        if character.is_ascii_uppercase() {
            output.push('_');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

// Original: format.ts, formatValue() / JavaScript String(value).
fn format_value(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => format_number(value),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(|value| match value {
                Value::Null => String::new(),
                _ => format_value(value),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

fn format_number(number: &Number) -> String {
    let Some(value) = number.as_f64() else {
        return number.to_string();
    };
    if value == 0.0 {
        "0".into()
    } else {
        number.to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn formats_fields_in_insertion_order_and_omits_null() {
        let record = json!({
            "taskId": "bash-12345678",
            "startedAt": 10,
            "endedAt": null,
            "outputTruncated": false
        });
        assert_eq!(
            format_plain_object(record.as_object().unwrap()),
            "task_id: bash-12345678\nstarted_at: 10\noutput_truncated: false"
        );
    }

    #[test]
    fn inserts_underscores_before_each_ascii_uppercase_only() {
        assert_eq!(field_name("URLValue"), "_u_r_l_value");
        assert_eq!(field_name("éValue"), "é_value");
        assert_eq!(field_name("already_snake"), "already_snake");
    }

    #[test]
    fn string_conversion_matches_javascript_for_supported_json_values() {
        let record = json!({
            "string": "raw",
            "array": ["a", null, 2, {"nested": true}],
            "object": {"nested": true},
            "negativeZero": -0.0
        });
        assert_eq!(
            format_plain_object(record.as_object().unwrap()),
            "string: raw\narray: a,,2,[object Object]\nobject: [object Object]\nnegative_zero: 0"
        );
    }
}
