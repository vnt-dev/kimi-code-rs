//! `[services]` config schema and TOML transforms.
//!
//! Original: `packages/agent-core-v2/src/app/auth/configSection.ts`.

use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    app::config::{
        ConfigFromToml, ConfigSchema, ConfigToToml, ConfigValidationError, RegisterSectionOptions,
        camel_to_snake, clone_record, plain_object_to_toml, register_config_section, set_defined,
        snake_to_camel, transform_plain_object,
    },
    kosong::provider::config::OAuthRef,
};

pub const SERVICES_SECTION: &str = "services";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoonshotServiceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_headers: Option<IndexMap<String, String>>,
}

pub type ServicesConfig = Map<String, Value>;

static SERVICES_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let Some(input) = value.as_object() else {
            return Err(ConfigValidationError::new("services must be an object"));
        };
        let mut output = input.clone();
        for key in ["moonshotSearch", "moonshotFetch"] {
            if let Some(value) = input.get(key) {
                let parsed: MoonshotServiceConfig = serde_json::from_value(value.clone())
                    .map_err(|error| ConfigValidationError::new(error.to_string()))?;
                output.insert(
                    key.into(),
                    serde_json::to_value(parsed)
                        .map_err(|error| ConfigValidationError::new(error.to_string()))?,
                );
            }
        }
        Ok(Value::Object(output))
    })
});

static SERVICES_FROM_TOML: LazyLock<ConfigFromToml> =
    LazyLock::new(|| Arc::new(services_from_toml));
static SERVICES_TO_TOML: LazyLock<ConfigToToml> = LazyLock::new(|| Arc::new(services_to_toml));

// Original: servicesFromToml().
pub fn services_from_toml(raw_snake: &Value) -> Value {
    let Some(raw_snake) = raw_snake.as_object() else {
        return raw_snake.clone();
    };
    Value::Object(
        raw_snake
            .iter()
            .map(|(name, entry)| {
                (
                    snake_to_camel(name),
                    entry
                        .as_object()
                        .map(service_entry_from_toml)
                        .map(Value::Object)
                        .unwrap_or_else(|| entry.clone()),
                )
            })
            .collect(),
    )
}

fn service_entry_from_toml(data: &Map<String, Value>) -> Map<String, Value> {
    data.iter()
        .map(|(key, value)| {
            let target_key = snake_to_camel(key);
            let value = match (target_key.as_str(), value.as_object()) {
                ("oauth", Some(value)) => Value::Object(transform_plain_object(value)),
                ("customHeaders", Some(_)) => Value::Object(clone_record(value)),
                _ => value.clone(),
            };
            (target_key, value)
        })
        .collect()
}

// Original: servicesToToml().
pub fn services_to_toml(value: &Value, raw_snake: &Value) -> Option<Value> {
    let Some(value) = value.as_object() else {
        return Some(value.clone());
    };
    let mut output = clone_record(raw_snake);
    write_service(&mut output, "moonshot_search", value.get("moonshotSearch"));
    write_service(&mut output, "moonshot_fetch", value.get("moonshotFetch"));
    Some(Value::Object(output))
}

fn write_service(output: &mut Map<String, Value>, snake_key: &str, service: Option<&Value>) {
    match service.and_then(Value::as_object) {
        Some(service) => {
            output.insert(
                snake_key.into(),
                Value::Object(service_entry_to_toml(service)),
            );
        }
        None => {
            output.shift_remove(snake_key);
        }
    }
}

fn service_entry_to_toml(service: &Map<String, Value>) -> Map<String, Value> {
    let mut output = Map::new();
    for (key, value) in service {
        let snake_key = camel_to_snake(key);
        if key == "oauth"
            && let Some(value) = value.as_object()
        {
            output.insert(snake_key, Value::Object(plain_object_to_toml(value, None)));
        } else if key == "customHeaders" {
            output.insert(snake_key, Value::Object(clone_record(value)));
        } else {
            set_defined(&mut output, &snake_key, Some(value));
        }
    }
    output
}

pub fn register_services_config_section() {
    register_config_section(
        SERVICES_SECTION,
        SERVICES_SCHEMA.clone(),
        RegisterSectionOptions {
            from_toml: Some(Arc::clone(&SERVICES_FROM_TOML)),
            to_toml: Some(Arc::clone(&SERVICES_TO_TOML)),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn transforms_nested_oauth_but_preserves_custom_header_names() {
        let raw = json!({
            "moonshot_search": {
                "base_url": "https://example.test",
                "oauth": {"storage": "file", "key": "token", "oauth_host": "https://oauth.test"},
                "custom_headers": {"X-Custom-Header": "value"}
            },
            "future_service": {"raw_key": true}
        });
        let memory = services_from_toml(&raw);
        assert_eq!(memory["moonshotSearch"]["baseUrl"], "https://example.test");
        assert_eq!(
            memory["moonshotSearch"]["oauth"]["oauthHost"],
            "https://oauth.test"
        );
        assert_eq!(
            memory["moonshotSearch"]["customHeaders"]["X-Custom-Header"],
            "value"
        );
        assert_eq!(memory["futureService"]["rawKey"], true);

        let written = services_to_toml(&memory, &raw).unwrap();
        assert_eq!(
            written["moonshot_search"]["oauth"]["oauth_host"],
            "https://oauth.test"
        );
        assert_eq!(
            written["moonshot_search"]["custom_headers"]["X-Custom-Header"],
            "value"
        );
        assert!(written.get("future_service").is_some());
    }

    #[test]
    fn schema_validates_known_services_and_passes_through_unknown_top_level_keys() {
        let valid = json!({
            "moonshotSearch": {
                "apiKey": "secret",
                "oauth": {"storage": "file", "key": "token"},
                "customHeaders": {"X-Test": "yes"},
                "discarded": true
            },
            "futureService": {"anything": true}
        });
        let parsed = SERVICES_SCHEMA.parse(&valid).unwrap();
        assert!(parsed["moonshotSearch"].get("discarded").is_none());
        assert_eq!(parsed["futureService"], json!({"anything": true}));
        assert!(
            SERVICES_SCHEMA
                .parse(&json!({"moonshotFetch": {"apiKey": 1}}))
                .is_err()
        );
        assert!(SERVICES_SCHEMA.parse(&json!([])).is_err());
    }
}
