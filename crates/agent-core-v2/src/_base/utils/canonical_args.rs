use serde_json::{Map, Value};

// Original: packages/agent-core-v2/src/_base/utils/canonical-args.ts,
// canonicalTelemetryArgs()/sortJsonValue().
pub fn canonical_telemetry_args(arguments: &Value) -> String {
    serde_json::to_string(&sort_json_value(arguments)).unwrap_or_else(|_| arguments.to_string())
}

fn sort_json_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(sort_json_value).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), sort_json_value(&values[key])))
                    .collect::<Map<_, _>>(),
            )
        }
        value => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursively_sorts_object_keys_without_reordering_arrays() {
        let arguments = serde_json::json!({
            "z": [{"b": 2, "a": 1}, 3],
            "a": {"d": true, "c": null}
        });

        assert_eq!(
            canonical_telemetry_args(&arguments),
            r#"{"a":{"c":null,"d":true},"z":[{"a":1,"b":2},3]}"#
        );
    }
}
