//! Provider config-section schema, environment bindings, and TOML transforms.
//!
//! Original: `packages/agent-core-v2/src/kosong/provider/configSection.ts`.

use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::app::config::{
    AnyEnvBindings, ConfigFromToml, ConfigSchema, ConfigStripEnv, ConfigToToml,
    ConfigValidationError, EnvBinding, RegisterSectionOptions, camel_to_snake, clone_record,
    plain_object_to_toml, register_config_section, set_defined, snake_to_camel,
    transform_plain_object,
};

use super::config::{ENV_MODEL_PROVIDER_KEY, PROVIDERS_SECTION, ProviderConfig, ProvidersSection};

pub static PROVIDERS_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let providers = serde_json::from_value::<ProvidersSection>(value.clone())
            .map_err(|error| ConfigValidationError::new(error.to_string()))?;
        serde_json::to_value(providers)
            .map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

pub static PROVIDER_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let provider = serde_json::from_value::<ProviderConfig>(value.clone())
            .map_err(|error| ConfigValidationError::new(error.to_string()))?;
        serde_json::to_value(provider)
            .map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

pub static PROVIDERS_ENV_BINDINGS: LazyLock<Arc<AnyEnvBindings>> = LazyLock::new(|| {
    Arc::new(AnyEnvBindings::Fields(IndexMap::from([(
        ENV_MODEL_PROVIDER_KEY.to_owned(),
        AnyEnvBindings::Fields(IndexMap::from([
            (
                "apiKey".to_owned(),
                AnyEnvBindings::Binding(EnvBinding::Name("KIMI_MODEL_API_KEY".to_owned())),
            ),
            (
                "type".to_owned(),
                AnyEnvBindings::Binding(EnvBinding::Name("KIMI_MODEL_PROVIDER_TYPE".to_owned())),
            ),
            (
                "baseUrl".to_owned(),
                AnyEnvBindings::Binding(EnvBinding::Name("KIMI_MODEL_BASE_URL".to_owned())),
            ),
        ])),
    )])))
});

static PROVIDERS_FROM_TOML: LazyLock<ConfigFromToml> =
    LazyLock::new(|| Arc::new(providers_from_toml));
static PROVIDERS_TO_TOML: LazyLock<ConfigToToml> = LazyLock::new(|| Arc::new(providers_to_toml));
static STRIP_PROVIDERS_ENV: LazyLock<ConfigStripEnv> =
    LazyLock::new(|| Arc::new(strip_providers_env));

// Original: configSection.ts, stripProvidersEnv().
pub fn strip_providers_env(value: &Value, _raw: Option<&Value>) -> Option<Value> {
    let Some(value) = value.as_object() else {
        return Some(value.clone());
    };
    if !value.contains_key(ENV_MODEL_PROVIDER_KEY) {
        return Some(Value::Object(value.clone()));
    }
    let mut output = value.clone();
    output.shift_remove(ENV_MODEL_PROVIDER_KEY);
    Some(Value::Object(output))
}

// Original: configSection.ts, providersFromToml().
pub fn providers_from_toml(raw_snake: &Value) -> Value {
    let Some(raw_snake) = raw_snake.as_object() else {
        return raw_snake.clone();
    };
    Value::Object(
        raw_snake
            .iter()
            .map(|(name, entry)| {
                (
                    name.clone(),
                    entry
                        .as_object()
                        .map(provider_entry_from_toml)
                        .map(Value::Object)
                        .unwrap_or_else(|| entry.clone()),
                )
            })
            .collect(),
    )
}

fn provider_entry_from_toml(data: &Map<String, Value>) -> Map<String, Value> {
    data.iter()
        .map(|(key, value)| {
            let target_key = snake_to_camel(key);
            let value = match (target_key.as_str(), value.as_object()) {
                ("oauth", Some(value)) => Value::Object(transform_plain_object(value)),
                ("env" | "customHeaders", Some(_)) => Value::Object(clone_record(value)),
                _ => value.clone(),
            };
            (target_key, value)
        })
        .collect()
}

// Original: configSection.ts, providersToToml().
pub fn providers_to_toml(value: &Value, raw_snake: &Value) -> Option<Value> {
    let Some(value) = value.as_object() else {
        return Some(value.clone());
    };
    let raw_sub = clone_record(raw_snake);
    Some(Value::Object(
        value
            .iter()
            .map(|(name, entry)| {
                (
                    name.clone(),
                    entry
                        .as_object()
                        .map(|entry| provider_entry_to_toml(entry, raw_sub.get(name)))
                        .map(Value::Object)
                        .unwrap_or_else(|| entry.clone()),
                )
            })
            .collect(),
    ))
}

fn provider_entry_to_toml(
    provider: &Map<String, Value>,
    raw_provider: Option<&Value>,
) -> Map<String, Value> {
    let mut output = raw_provider.map_or_else(Map::new, clone_record);
    for (key, value) in provider {
        let snake_key = camel_to_snake(key);
        if key == "oauth"
            && let Some(value) = value.as_object()
        {
            output.insert(snake_key, Value::Object(plain_object_to_toml(value, None)));
        } else if matches!(key.as_str(), "env" | "customHeaders") {
            output.insert(snake_key, Value::Object(clone_record(value)));
        } else {
            set_defined(&mut output, &snake_key, Some(value));
        }
    }
    output
}

pub fn register_provider_config_section() {
    register_config_section(
        PROVIDERS_SECTION,
        PROVIDERS_SCHEMA.clone(),
        RegisterSectionOptions {
            default_value: Some(Value::Object(Map::new())),
            env: Some(Arc::clone(&PROVIDERS_ENV_BINDINGS)),
            strip_env: Some(Arc::clone(&STRIP_PROVIDERS_ENV)),
            from_toml: Some(Arc::clone(&PROVIDERS_FROM_TOML)),
            to_toml: Some(Arc::clone(&PROVIDERS_TO_TOML)),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use crate::app::config::apply_section_env;

    use super::*;

    #[test]
    fn validates_provider_records_and_strips_unknown_provider_fields() {
        let parsed = PROVIDERS_SCHEMA
            .parse(&json!({
                "kimi": {"type": "kimi", "baseUrl": "https://api.test", "future": true}
            }))
            .unwrap();
        assert_eq!(parsed["kimi"]["type"], "kimi");
        assert!(parsed["kimi"].get("future").is_none());
        assert!(PROVIDERS_SCHEMA.parse(&json!([])).is_err());
    }

    #[test]
    fn env_binding_synthesizes_reserved_provider_and_strip_removes_only_it() {
        let env = HashMap::from([
            ("KIMI_MODEL_PROVIDER_TYPE".to_owned(), "kimi".to_owned()),
            ("KIMI_MODEL_API_KEY".to_owned(), "secret".to_owned()),
        ]);
        let applied = apply_section_env(None, &PROVIDERS_ENV_BINDINGS, &|name| {
            env.get(name).cloned()
        })
        .unwrap()
        .unwrap();
        assert_eq!(applied[ENV_MODEL_PROVIDER_KEY]["type"], "kimi");
        assert_eq!(applied[ENV_MODEL_PROVIDER_KEY]["apiKey"], "secret");
        assert_eq!(strip_providers_env(&applied, None), Some(json!({})));
    }

    #[test]
    fn toml_round_trip_transforms_owned_fields_and_preserves_header_env_and_unknown_keys() {
        let raw = json!({
            "kimi": {
                "base_url": "https://api.test",
                "oauth": {"storage": "file", "key": "kimi", "oauth_host": "https://oauth.test"},
                "custom_headers": {"X-Custom": "yes"},
                "env": {"Mixed_Case": "value"},
                "future_key": true
            }
        });
        let memory = providers_from_toml(&raw);
        assert_eq!(memory["kimi"]["baseUrl"], "https://api.test");
        assert_eq!(memory["kimi"]["oauth"]["oauthHost"], "https://oauth.test");
        assert_eq!(memory["kimi"]["customHeaders"]["X-Custom"], "yes");
        assert_eq!(memory["kimi"]["env"]["Mixed_Case"], "value");
        let written = providers_to_toml(&memory, &raw).unwrap();
        assert_eq!(written["kimi"]["oauth"]["oauth_host"], "https://oauth.test");
        assert!(written["kimi"].get("future_key").is_some());
    }
}
