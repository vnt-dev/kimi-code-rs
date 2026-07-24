//! Image configuration schema and environment bindings.
//!
//! Original: `packages/agent-core-v2/src/agent/media/configSection.ts`.

use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::app::config::{
    AnyEnvBindings, ConfigSchema, ConfigValidationError, EnvBinding, RegisterSectionOptions,
    register_config_section,
};

pub const IMAGE_SECTION: &str = "image";
pub const IMAGE_MAX_EDGE_ENV: &str = "KIMI_IMAGE_MAX_EDGE_PX";
pub const IMAGE_READ_BYTE_BUDGET_ENV: &str = "KIMI_IMAGE_READ_BYTE_BUDGET";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_edge_px: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_byte_budget: Option<u64>,
}

pub static IMAGE_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let object = value
            .as_object()
            .ok_or_else(|| ConfigValidationError::new("image config must be an object"))?;
        let config = ImageConfig {
            max_edge_px: optional_positive_integer(object, "maxEdgePx")?,
            read_byte_budget: optional_positive_integer(object, "readByteBudget")?,
        };
        serde_json::to_value(config).map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

pub static IMAGE_ENV_BINDINGS: LazyLock<Arc<AnyEnvBindings>> = LazyLock::new(|| {
    Arc::new(AnyEnvBindings::Fields(IndexMap::from([
        (
            "maxEdgePx".into(),
            parsed_positive_integer_binding(IMAGE_MAX_EDGE_ENV),
        ),
        (
            "readByteBudget".into(),
            parsed_positive_integer_binding(IMAGE_READ_BYTE_BUDGET_ENV),
        ),
    ])))
});

fn optional_positive_integer(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, ConfigValidationError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    value
        .as_u64()
        .filter(|value| *value > 0)
        .map(Some)
        .ok_or_else(|| ConfigValidationError::new(format!("{key} must be a positive integer")))
}

fn parsed_positive_integer_binding(env: &str) -> AnyEnvBindings {
    AnyEnvBindings::Binding(EnvBinding::Parsed {
        env: env.into(),
        parse: Some(Arc::new(|raw| Ok(parse_positive_int(raw).map(Value::from)))),
        default: None,
    })
}

// Original: parsePositiveInt(). It accepts only ASCII decimal digits after
// trimming; signed, decimal, and whitespace-only values produce undefined.
pub fn parse_positive_int(raw: &str) -> Option<u64> {
    let value = raw.trim();
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<u64>().ok())
        .flatten()
        .filter(|value| *value > 0)
}

// Original: registerConfigSection(IMAGE_SECTION, ...).
pub fn register_image_config_section() {
    register_config_section(
        IMAGE_SECTION,
        IMAGE_CONFIG_SCHEMA.clone(),
        RegisterSectionOptions {
            default_value: Some(Value::Object(Map::new())),
            env: Some(IMAGE_ENV_BINDINGS.clone()),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::app::config::apply_section_env;

    fn getter(values: HashMap<String, String>) -> impl Fn(&str) -> Option<String> {
        move |name| values.get(name).cloned()
    }

    #[test]
    fn schema_keeps_only_positive_integer_fields() {
        assert_eq!(
            IMAGE_CONFIG_SCHEMA
                .parse(&json!({"maxEdgePx": 2000, "readByteBudget": 262144, "future": true}))
                .unwrap(),
            json!({"maxEdgePx": 2000, "readByteBudget": 262144})
        );
        for invalid in [
            json!(null),
            json!({"maxEdgePx": 0}),
            json!({"maxEdgePx": -1}),
            json!({"maxEdgePx": 1.5}),
            json!({"readByteBudget": "1"}),
        ] {
            assert!(IMAGE_CONFIG_SCHEMA.parse(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn environment_parser_matches_source_decimal_rules() {
        assert_eq!(parse_positive_int(" 001 "), Some(1));
        for invalid in ["", " ", "0", "-1", "+1", "1.0", "1e3", "١"] {
            assert_eq!(parse_positive_int(invalid), None, "{invalid}");
        }
        let effective = apply_section_env(
            Some(&json!({"maxEdgePx": 1000, "readByteBudget": 10})),
            &IMAGE_ENV_BINDINGS,
            &getter(HashMap::from([
                (IMAGE_MAX_EDGE_ENV.into(), " 2048 ".into()),
                (IMAGE_READ_BYTE_BUDGET_ENV.into(), "invalid".into()),
            ])),
        )
        .unwrap();
        assert_eq!(
            effective,
            Some(json!({"maxEdgePx": 2048, "readByteBudget": 10}))
        );
    }

    #[test]
    fn registration_uses_empty_default_and_environment_overlay() {
        crate::app::config::clear_config_section_contributions_for_tests();
        register_image_config_section();
        let contributions = crate::app::config::get_config_section_contributions();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].domain, IMAGE_SECTION);
        assert_eq!(contributions[0].options.default_value, Some(json!({})));
        assert!(contributions[0].options.env.is_some());
        crate::app::config::clear_config_section_contributions_for_tests();
    }
}
