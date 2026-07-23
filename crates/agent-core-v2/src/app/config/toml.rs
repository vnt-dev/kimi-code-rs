//! Generic TOML snake_case/camelCase object transforms.
//!
//! Original: `packages/agent-core-v2/src/app/config/toml.ts`.

use serde_json::{Map, Value};

use super::contract::ConfigRegistryContract;

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

// Original: transformTomlData(). Unknown sections use the default shallow
// snake_case-to-camelCase transform; registered owners may replace it.
pub fn transform_toml_data(
    data: &Map<String, Value>,
    registry: &dyn ConfigRegistryContract,
) -> Map<String, Value> {
    data.iter()
        .map(|(key, value)| {
            let domain = snake_to_camel(key);
            let converted = registry
                .get_section(&domain)
                .and_then(|section| section.from_toml)
                .map_or_else(
                    || {
                        value
                            .as_object()
                            .map(transform_plain_object)
                            .map(Value::Object)
                            .unwrap_or_else(|| value.clone())
                    },
                    |from_toml| from_toml(value),
                );
            (domain, converted)
        })
        .collect()
}

// Original: applySectionToToml(). `None` represents an undefined delivered
// value and removes the section. Null/empty owner-hook results also remove it.
pub fn apply_section_to_toml(
    raw_snake: &mut Map<String, Value>,
    domain: &str,
    value: Option<&Value>,
    registry: &dyn ConfigRegistryContract,
) {
    let snake_key = camel_to_snake(domain);
    let Some(value) = value else {
        raw_snake.shift_remove(&snake_key);
        return;
    };

    if let Some(to_toml) = registry
        .get_section(domain)
        .and_then(|section| section.to_toml)
    {
        let raw_sub = Value::Object(
            raw_snake
                .get(&snake_key)
                .map_or_else(Map::new, clone_record),
        );
        match to_toml(value, &raw_sub) {
            None | Some(Value::Null) => {
                raw_snake.shift_remove(&snake_key);
            }
            Some(Value::Object(converted)) if converted.is_empty() => {
                raw_snake.shift_remove(&snake_key);
            }
            Some(converted) => {
                raw_snake.insert(snake_key, converted);
            }
        }
        return;
    }

    let Some(value) = value.as_object() else {
        raw_snake.insert(snake_key, value.clone());
        return;
    };
    let raw_sub = raw_snake.get(&snake_key);
    let converted = plain_object_to_toml(value, raw_sub);
    if converted.is_empty() {
        raw_snake.shift_remove(&snake_key);
    } else {
        raw_snake.insert(snake_key, Value::Object(converted));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::app::config::{ConfigRegistry, ConfigSchema, RegisterSectionOptions};

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

    #[test]
    fn registry_hooks_override_default_read_and_write_transforms() {
        let registry = ConfigRegistry::new().unwrap();
        registry
            .register_section(
                "experimental",
                ConfigSchema::new(|value| Ok(value.clone())),
                RegisterSectionOptions {
                    from_toml: Some(Arc::new(|value| value.clone())),
                    to_toml: Some(Arc::new(|value, _| Some(value.clone()))),
                    ..RegisterSectionOptions::default()
                },
            )
            .unwrap();
        let raw = json!({
            "agent_config": {"model_alias": "k2"},
            "experimental": {"keep_snake": true}
        });
        let transformed = transform_toml_data(raw.as_object().unwrap(), &registry);
        assert_eq!(
            transformed,
            json!({
                "agentConfig": {"modelAlias": "k2"},
                "experimental": {"keep_snake": true}
            })
            .as_object()
            .unwrap()
            .clone()
        );

        let mut write_base = raw.as_object().unwrap().clone();
        apply_section_to_toml(
            &mut write_base,
            "experimental",
            Some(&json!({"new_flag": false})),
            &registry,
        );
        assert_eq!(write_base["experimental"], json!({"new_flag": false}));
        assert!(write_base.contains_key("agent_config"));
    }

    #[test]
    fn write_transform_removes_missing_null_and_empty_sections() {
        let registry = ConfigRegistry::new().unwrap();
        let mut raw = json!({"a": {"value": 1}, "b": true})
            .as_object()
            .unwrap()
            .clone();
        apply_section_to_toml(&mut raw, "a", None, &registry);
        apply_section_to_toml(&mut raw, "b", Some(&json!({})), &registry);
        assert!(raw.is_empty());
    }
}
