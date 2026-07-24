//! Model config-section schema and TOML transforms.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/configSection.ts`.

use std::sync::{Arc, LazyLock};

use serde_json::{Map, Value};

use crate::app::config::{
    ConfigFromToml, ConfigSchema, ConfigToToml, ConfigValidationError, RegisterSectionOptions,
    camel_to_snake, clone_record, register_config_section, set_defined, transform_plain_object,
};

use super::contract::{MODELS_SECTION, ModelsSection};

pub static MODELS_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let models = serde_json::from_value::<ModelsSection>(value.clone())
            .map_err(|error| ConfigValidationError::new(error.to_string()))?;
        serde_json::to_value(models).map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

static MODELS_FROM_TOML: LazyLock<ConfigFromToml> = LazyLock::new(|| Arc::new(models_from_toml));
static MODELS_TO_TOML: LazyLock<ConfigToToml> = LazyLock::new(|| Arc::new(models_to_toml));

// Original: configSection.ts, modelsFromToml().
pub fn models_from_toml(raw_snake: &Value) -> Value {
    let Some(raw_snake) = raw_snake.as_object() else {
        return raw_snake.clone();
    };

    Value::Object(
        raw_snake
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    entry
                        .as_object()
                        .map(model_entry_from_toml)
                        .map(Value::Object)
                        .unwrap_or_else(|| entry.clone()),
                )
            })
            .collect(),
    )
}

fn model_entry_from_toml(entry: &Map<String, Value>) -> Map<String, Value> {
    let mut converted = transform_plain_object(entry);
    if let Some(overrides) = converted.get("overrides").and_then(Value::as_object) {
        converted.insert(
            "overrides".to_owned(),
            Value::Object(transform_plain_object(overrides)),
        );
    }
    converted
}

// Original: configSection.ts, modelsToToml().
pub fn models_to_toml(value: &Value, raw_snake: &Value) -> Option<Value> {
    let Some(models) = value.as_object() else {
        return Some(value.clone());
    };
    let raw_models = clone_record(raw_snake);

    Some(Value::Object(
        models
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    entry
                        .as_object()
                        .map(|entry| model_entry_to_toml(entry, raw_models.get(id)))
                        .map(Value::Object)
                        .unwrap_or_else(|| entry.clone()),
                )
            })
            .collect(),
    ))
}

fn model_entry_to_toml(
    entry: &Map<String, Value>,
    raw_entry: Option<&Value>,
) -> Map<String, Value> {
    let mut output = raw_entry.map_or_else(Map::new, clone_record);
    for (key, field) in entry {
        let snake_key = camel_to_snake(key);
        if key == "overrides"
            && let Some(overrides) = field.as_object()
        {
            output.insert(
                snake_key,
                Value::Object(model_overrides_to_toml(
                    overrides,
                    raw_entry
                        .and_then(Value::as_object)
                        .and_then(|raw| raw.get("overrides")),
                )),
            );
        } else {
            // `capabilities` requires no value conversion, but is kept as a
            // dedicated branch in the source to preserve its string list.
            set_defined(&mut output, &snake_key, Some(field));
        }
    }
    output
}

fn model_overrides_to_toml(
    overrides: &Map<String, Value>,
    raw_overrides: Option<&Value>,
) -> Map<String, Value> {
    let mut output = raw_overrides.map_or_else(Map::new, clone_record);
    for (key, value) in overrides {
        set_defined(&mut output, &camel_to_snake(key), Some(value));
    }
    output
}

// Original: module-load `registerConfigSection(MODELS_SECTION, ...)`.
// Rust has no module-load side effects, so the model-catalog composition root
// calls this before it builds a config registry.
pub fn register_models_config_section() {
    register_config_section(
        MODELS_SECTION,
        MODELS_SCHEMA.clone(),
        RegisterSectionOptions {
            default_value: Some(Value::Object(Map::new())),
            from_toml: Some(Arc::clone(&MODELS_FROM_TOML)),
            to_toml: Some(Arc::clone(&MODELS_TO_TOML)),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn toml_round_trip_preserves_model_ids_unknown_fields_and_nested_overrides() {
        let raw = json!({
            "kimi-k2": {
                "base_url": "https://api.example.test/v1",
                "max_context_size": 262144,
                "capabilities": ["image_in", "thinking"],
                "overrides": {"max_output_size": 8192},
                "future_field": {"keep": true}
            }
        });

        let memory = models_from_toml(&raw);
        assert_eq!(memory["kimi-k2"]["baseUrl"], "https://api.example.test/v1");
        assert_eq!(memory["kimi-k2"]["maxContextSize"], 262144);
        assert_eq!(memory["kimi-k2"]["overrides"]["maxOutputSize"], 8192);
        assert_eq!(memory["kimi-k2"]["futureField"], json!({"keep": true}));

        let written = models_to_toml(&memory, &raw).unwrap();
        assert_eq!(
            written["kimi-k2"]["base_url"],
            "https://api.example.test/v1"
        );
        assert_eq!(written["kimi-k2"]["overrides"]["max_output_size"], 8192);
        assert_eq!(written["kimi-k2"]["future_field"], json!({"keep": true}));
    }

    #[test]
    fn schema_validates_known_fields_but_retains_passthrough_fields() {
        let parsed = MODELS_SCHEMA
            .parse(&json!({
                "kimi-k2": {
                    "maxContextSize": 262144,
                    "overrides": {"maxOutputSize": 8192},
                    "vendorExtension": {"enabled": true}
                }
            }))
            .unwrap();
        assert_eq!(parsed["kimi-k2"]["vendorExtension"]["enabled"], true);
        assert!(
            MODELS_SCHEMA
                .parse(&json!({"kimi-k2": {"maxContextSize": 0}}))
                .is_err()
        );
    }
}
