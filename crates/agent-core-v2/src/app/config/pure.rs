//! Side-effect-free helpers for configuration values.
//!
//! Original: `packages/agent-core-v2/src/app/config/configPure.ts`.

use serde_json::{Map, Value};

// Original: isPlainObject(). All Serde JSON objects are plain objects; arrays,
// scalars, and null retain the source's negative result.
pub fn is_plain_object(value: &Value) -> bool {
    value.is_object()
}

// Original: deepEqual(). Numeric comparison deliberately goes through f64 to
// reproduce JavaScript Number equality for integer/float TOML representations.
pub fn deep_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| deep_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, left)| right.get(key).is_some_and(|right| deep_equal(left, right)))
        }
        _ => left == right,
    }
}

// Original: deepMerge(). `None` represents JavaScript `undefined`. A top-level
// null patch is nullish and therefore falls back to the base, while null fields
// inside an object remain explicit values exactly as in the source loop.
pub fn deep_merge(base: Option<&Value>, patch: Option<&Value>) -> Option<Value> {
    let (Some(Value::Object(base)), Some(Value::Object(patch))) = (base, patch) else {
        return match patch {
            Some(Value::Null) | None => base.cloned(),
            Some(patch) => Some(patch.clone()),
        };
    };

    let mut output = base.clone();
    for (key, patch_value) in patch {
        let merged = match output.get(key) {
            Some(Value::Object(base_value)) if patch_value.is_object() => {
                deep_merge(Some(&Value::Object(base_value.clone())), Some(patch_value))
                    .expect("an object patch always produces a value")
            }
            _ => patch_value.clone(),
        };
        output.insert(key.clone(), merged);
    }
    Some(Value::Object(output))
}

// Original: omitUndefined(). Serde JSON has no `undefined` value, so every
// representable entry is defined and the behavior reduces to an owned clone.
pub fn omit_undefined(value: &Map<String, Value>) -> Map<String, Value> {
    value.clone()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn recognizes_only_plain_objects() {
        assert!(is_plain_object(&json!({})));
        assert!(!is_plain_object(&json!([])));
        assert!(!is_plain_object(&Value::Null));
    }

    #[test]
    fn deep_equality_ignores_object_order_and_numeric_representation() {
        assert!(deep_equal(
            &json!({"a": [1, {"b": true}], "c": null}),
            &json!({"c": null, "a": [1.0, {"b": true}]})
        ));
        assert!(!deep_equal(&json!([1, 2]), &json!([2, 1])));
    }

    #[test]
    fn deep_merge_recurses_only_when_both_values_are_objects() {
        let base = json!({"a": {"b": 1, "c": 2}, "keep": true});
        let patch = json!({"a": {"b": 3, "d": null}, "keep": [1]});
        assert_eq!(
            deep_merge(Some(&base), Some(&patch)),
            Some(json!({"a": {"b": 3, "c": 2, "d": null}, "keep": [1]}))
        );
        assert_eq!(deep_merge(Some(&base), Some(&Value::Null)), Some(base));
        assert_eq!(deep_merge(None, None), None);
    }
}
