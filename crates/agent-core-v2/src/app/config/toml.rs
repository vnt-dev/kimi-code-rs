//! Generic TOML snake_case/camelCase object transforms.
//!
//! Original: `packages/agent-core-v2/src/app/config/toml.ts`.

use serde_json::{Map, Value};

// Original: snakeToCamel(). The source regular expression only recognizes an
// underscore followed by an ASCII lowercase letter.
pub fn snake_to_camel(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '_'
            && let Some(next) = characters.peek().copied()
            && next.is_ascii_lowercase()
        {
            characters.next();
            output.push(next.to_ascii_uppercase());
            continue;
        }
        output.push(character);
    }
    output
}

// Original: camelToSnake(). Like JavaScript's `[A-Z]`, conversion is limited
// to ASCII uppercase letters.
pub fn camel_to_snake(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            output.push('_');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

// Original: transformPlainObject(). Only the current object's keys are
// transformed; nested values remain untouched for their owning section hook.
pub fn transform_plain_object(data: &Map<String, Value>) -> Map<String, Value> {
    data.iter()
        .map(|(key, value)| (snake_to_camel(key), value.clone()))
        .collect()
}

// Original: plainObjectToToml(). Unknown raw keys are retained for lossless
// round trips and entries from `value` overwrite their snake_case counterpart.
pub fn plain_object_to_toml(value: &Map<String, Value>, raw: Option<&Value>) -> Map<String, Value> {
    let mut output = raw.map_or_else(Map::new, clone_record);
    for (key, entry) in value {
        set_defined(&mut output, &camel_to_snake(key), Some(entry));
    }
    output
}

// Original: cloneRecord(). Serde values already have deep Clone semantics.
pub fn clone_record(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

// Original: setDefined(). `None` is the Rust representation of JavaScript
// `undefined`; JSON null remains an explicitly stored value.
pub fn set_defined(target: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value {
        target.insert(key.into(), value.clone());
    } else {
        target.shift_remove(key);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn converts_only_the_same_ascii_patterns_as_the_source_regexes() {
        assert_eq!(snake_to_camel("model_alias"), "modelAlias");
        assert_eq!(snake_to_camel("a__b_A_é"), "a_B_A_é");
        assert_eq!(camel_to_snake("modelAliasURLÉ"), "model_alias_u_r_lÉ");
    }

    #[test]
    fn transforms_only_top_level_keys() {
        let source = json!({"model_alias": {"nested_key": true}, "URL": 1});
        assert_eq!(
            transform_plain_object(source.as_object().unwrap()),
            json!({"modelAlias": {"nested_key": true}, "URL": 1})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn write_transform_preserves_unknown_raw_keys_and_explicit_null() {
        let value = json!({"modelAlias": "k2", "nullable": null});
        let raw = json!({"unknown_key": 1, "model_alias": "old"});
        assert_eq!(
            plain_object_to_toml(value.as_object().unwrap(), Some(&raw)),
            json!({"unknown_key": 1, "model_alias": "k2", "nullable": null})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn set_defined_deletes_only_missing_values() {
        let mut target = json!({"a": 1}).as_object().unwrap().clone();
        set_defined(&mut target, "a", None);
        set_defined(&mut target, "b", Some(&Value::Null));
        assert_eq!(target, json!({"b": null}).as_object().unwrap().clone());
        assert!(clone_record(&json!([])).is_empty());
    }
}
