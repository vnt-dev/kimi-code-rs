//! Loop-control configuration schema and TOML key transformations.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/configSection.ts`.

use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::app::config::{
    ConfigSchema, ConfigValidationError, RegisterSectionOptions, plain_object_to_toml,
    register_config_section, transform_plain_object,
};

pub const LOOP_CONTROL_SECTION: &str = "loopControl";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopControl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps_per_turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries_per_step: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ralph_iterations: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved_context_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_trigger_ratio: Option<f64>,
}

pub static LOOP_CONTROL_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let object = value
            .as_object()
            .ok_or_else(|| ConfigValidationError::new("loopControl config must be an object"))?;
        let config = LoopControl {
            max_steps_per_turn: optional_unsigned_integer(object, "maxStepsPerTurn")?,
            max_retries_per_step: optional_unsigned_integer(object, "maxRetriesPerStep")?,
            max_ralph_iterations: optional_signed_integer(object, "maxRalphIterations", -1)?,
            reserved_context_size: optional_unsigned_integer(object, "reservedContextSize")?,
            compaction_trigger_ratio: optional_ratio(object, "compactionTriggerRatio")?,
        };
        serde_json::to_value(config).map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

fn optional_unsigned_integer(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, ConfigValidationError> {
    object.get(key).map_or(Ok(None), |value| {
        value.as_u64().map(Some).ok_or_else(|| {
            ConfigValidationError::new(format!("{key} must be a non-negative integer"))
        })
    })
}

fn optional_signed_integer(
    object: &Map<String, Value>,
    key: &str,
    minimum: i64,
) -> Result<Option<i64>, ConfigValidationError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let value = value.as_i64().ok_or_else(|| {
        ConfigValidationError::new(format!("{key} must be an integer of at least {minimum}"))
    })?;
    if value < minimum {
        return Err(ConfigValidationError::new(format!(
            "{key} must be an integer of at least {minimum}"
        )));
    }
    Ok(Some(value))
}

fn optional_ratio(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, ConfigValidationError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let value = value.as_f64().ok_or_else(|| {
        ConfigValidationError::new(format!("{key} must be a number from 0.5 through 0.99"))
    })?;
    if !(0.5..=0.99).contains(&value) {
        return Err(ConfigValidationError::new(format!(
            "{key} must be a number from 0.5 through 0.99"
        )));
    }
    Ok(Some(value))
}

// Original: configSection.ts, loopControlFromToml().
pub fn loop_control_from_toml(raw_snake: &Value) -> Value {
    let Some(object) = raw_snake.as_object() else {
        return raw_snake.clone();
    };
    let mut output = transform_plain_object(object);
    if !output.contains_key("maxStepsPerTurn")
        && let Some(legacy) = output.get("maxStepsPerRun").cloned()
    {
        output.insert("maxStepsPerTurn".into(), legacy);
    }
    output.remove("maxStepsPerRun");
    Value::Object(output)
}

// Original: configSection.ts, loopControlToToml().
pub fn loop_control_to_toml(value: &Value, raw_snake: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    Value::Object(plain_object_to_toml(object, Some(raw_snake)))
}

// Original: configSection.ts, registerConfigSection().
pub fn register_loop_control_config_section() {
    register_config_section(
        LOOP_CONTROL_SECTION,
        LOOP_CONTROL_SCHEMA.clone(),
        RegisterSectionOptions {
            from_toml: Some(Arc::new(loop_control_from_toml)),
            to_toml: Some(Arc::new(|value, raw| {
                Some(loop_control_to_toml(value, raw))
            })),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schema_strips_unknowns_and_enforces_every_source_constraint() {
        assert_eq!(
            LOOP_CONTROL_SCHEMA
                .parse(&json!({
                    "maxStepsPerTurn": 0,
                    "maxRetriesPerStep": 2,
                    "maxRalphIterations": -1,
                    "reservedContextSize": 0,
                    "compactionTriggerRatio": 0.75,
                    "unknown": true
                }))
                .unwrap(),
            json!({
                "maxStepsPerTurn": 0,
                "maxRetriesPerStep": 2,
                "maxRalphIterations": -1,
                "reservedContextSize": 0,
                "compactionTriggerRatio": 0.75
            })
        );
        for invalid in [
            json!(null),
            json!({"maxStepsPerTurn": -1}),
            json!({"maxRetriesPerStep": 1.5}),
            json!({"maxRalphIterations": -2}),
            json!({"reservedContextSize": "0"}),
            json!({"compactionTriggerRatio": 0.49}),
            json!({"compactionTriggerRatio": 1.0}),
        ] {
            assert!(LOOP_CONTROL_SCHEMA.parse(&invalid).is_err(), "{invalid}");
        }
        assert!(
            LOOP_CONTROL_SCHEMA
                .parse(&json!({"compactionTriggerRatio": 0.5}))
                .is_ok()
        );
        assert!(
            LOOP_CONTROL_SCHEMA
                .parse(&json!({"compactionTriggerRatio": 0.99}))
                .is_ok()
        );
    }

    #[test]
    fn toml_read_renames_legacy_only_when_current_key_is_absent() {
        assert_eq!(
            loop_control_from_toml(&json!({
                "max_steps_per_run": 5,
                "reserved_context_size": 100
            })),
            json!({"maxStepsPerTurn": 5, "reservedContextSize": 100})
        );
        assert_eq!(
            loop_control_from_toml(&json!({
                "max_steps_per_turn": 7,
                "max_steps_per_run": 5
            })),
            json!({"maxStepsPerTurn": 7})
        );
        assert_eq!(loop_control_from_toml(&json!([1])), json!([1]));
    }

    #[test]
    fn toml_write_preserves_unknown_raw_keys_and_overlays_snake_case_values() {
        assert_eq!(
            loop_control_to_toml(
                &json!({"maxStepsPerTurn": 8, "reservedContextSize": 10}),
                &json!({"max_steps_per_turn": 4, "future_key": true})
            ),
            json!({
                "max_steps_per_turn": 8,
                "future_key": true,
                "reserved_context_size": 10
            })
        );
        assert_eq!(
            loop_control_to_toml(&json!(false), &json!({})),
            json!(false)
        );
    }

    #[test]
    fn contribution_uses_both_toml_transforms() {
        crate::app::config::clear_config_section_contributions_for_tests();
        register_loop_control_config_section();
        let contributions = crate::app::config::get_config_section_contributions();
        let contribution = contributions
            .iter()
            .find(|entry| entry.domain == LOOP_CONTROL_SECTION)
            .unwrap();
        assert!(contribution.options.from_toml.is_some());
        assert!(contribution.options.to_toml.is_some());
        crate::app::config::clear_config_section_contributions_for_tests();
    }
}
