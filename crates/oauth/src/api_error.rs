use serde_json::Value;

const DIRECT_ERROR_KEYS: [&str; 3] = ["error_description", "message", "detail"];
const NESTED_ERROR_KEYS: [&str; 5] = ["message", "error_description", "detail", "code", "type"];

// Original:
//   packages/oauth/src/api-error.ts
//   extractApiErrorMessage()
pub fn extract_api_error_message(value: &Value) -> Option<String> {
    if let Some(items) = value.as_array() {
        return items.iter().find_map(extract_api_error_message);
    }
    let record = value.as_object()?;
    for key in DIRECT_ERROR_KEYS {
        if let Some(message) = nonempty_string(record.get(key)) {
            return Some(message);
        }
    }

    let error = record.get("error");
    if let Some(message) = nonempty_string(error) {
        return Some(message);
    }
    if let Some(error) = error.and_then(Value::as_object) {
        for key in NESTED_ERROR_KEYS {
            if let Some(message) = nonempty_string(error.get(key)) {
                return Some(message);
            }
        }
    }

    record
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find_map(extract_api_error_message))
}

// Original: readApiErrorMessage()
pub async fn read_api_error_message(response: reqwest::Response, fallback: &str) -> String {
    match response.json::<Value>().await {
        Ok(value) => extract_api_error_message(&value).unwrap_or_else(|| fallback.to_owned()),
        Err(_) => fallback.to_owned(),
    }
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_direct_nested_string_and_array_errors_in_original_priority() {
        for (value, expected) in [
            (
                serde_json::json!({
                    "detail": "direct detail",
                    "message": " direct message ",
                    "error_description": "direct description",
                    "error": { "message": "nested" }
                }),
                "direct description",
            ),
            (
                serde_json::json!({
                    "error": { "detail": "nested detail", "code": "nested-code" }
                }),
                "nested detail",
            ),
            (
                serde_json::json!({
                    "errors": [null, { "error": " array error " }]
                }),
                "array error",
            ),
            (
                serde_json::json!([{}, { "message": "top array" }]),
                "top array",
            ),
        ] {
            assert_eq!(extract_api_error_message(&value).as_deref(), Some(expected));
        }
    }

    #[test]
    fn ignores_blank_non_string_and_unrecognized_values() {
        for value in [
            Value::Null,
            serde_json::json!("error"),
            serde_json::json!({ "message": " ", "error": 42 }),
            serde_json::json!({ "errors": [false, {}] }),
        ] {
            assert_eq!(extract_api_error_message(&value), None);
        }
    }
}
