//! External hook configuration section and TOML transforms.
//!
//! Original: `packages/agent-core-v2/src/agent/externalHooks/configSection.ts`.

use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    agent::external_hooks::types::HookEventType,
    app::config::{
        ConfigFromToml, ConfigSchema, ConfigToToml, ConfigValidationError, RegisterSectionOptions,
        plain_object_to_toml, register_config_section, transform_plain_object,
    },
};

pub const HOOKS_SECTION: &str = "hooks";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookDefConfig {
    pub event: HookEventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

pub type HooksConfig = Vec<HookDefConfig>;

pub static HOOKS_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let entries = value
            .as_array()
            .ok_or_else(|| ConfigValidationError::new("hooks must be an array"))?;
        let hooks = entries
            .iter()
            .map(parse_hook_def_config)
            .collect::<Result<HooksConfig, _>>()?;
        serde_json::to_value(hooks).map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

// Original: configSection.ts, HookDefSchema.safeParse() for one entry.
pub fn parse_hook_def_config(value: &Value) -> Result<HookDefConfig, ConfigValidationError> {
    let hook = serde_json::from_value::<HookDefConfig>(value.clone())
        .map_err(|error| ConfigValidationError::new(error.to_string()))?;
    if hook.command.is_empty() {
        return Err(ConfigValidationError::new(
            "hook command must contain at least one character",
        ));
    }
    if hook
        .timeout
        .is_some_and(|timeout| !(1..=600).contains(&timeout))
    {
        return Err(ConfigValidationError::new(
            "hook timeout must be an integer from 1 through 600",
        ));
    }
    Ok(hook)
}

static HOOKS_FROM_TOML: LazyLock<ConfigFromToml> = LazyLock::new(|| Arc::new(hooks_from_toml));
static HOOKS_TO_TOML: LazyLock<ConfigToToml> = LazyLock::new(|| Arc::new(hooks_to_toml));

// Original: hooksFromToml().
pub fn hooks_from_toml(raw_snake: &Value) -> Value {
    let Some(hooks) = raw_snake.as_array() else {
        return raw_snake.clone();
    };
    Value::Array(
        hooks
            .iter()
            .map(|hook| {
                hook.as_object()
                    .map(transform_plain_object)
                    .map(Value::Object)
                    .unwrap_or_else(|| hook.clone())
            })
            .collect(),
    )
}

// Original: hooksToToml(). The raw value is intentionally ignored: hook
// entries are wholly owned strict objects.
pub fn hooks_to_toml(value: &Value, _raw_snake: &Value) -> Option<Value> {
    let Some(hooks) = value.as_array() else {
        return Some(value.clone());
    };
    Some(Value::Array(
        hooks
            .iter()
            .map(|hook| {
                hook.as_object()
                    .map(|hook| plain_object_to_toml(hook, None))
                    .map(Value::Object)
                    .unwrap_or_else(|| hook.clone())
            })
            .collect(),
    ))
}

pub fn register_hooks_config_section() {
    register_config_section(
        HOOKS_SECTION,
        HOOKS_CONFIG_SCHEMA.clone(),
        RegisterSectionOptions {
            from_toml: Some(Arc::clone(&HOOKS_FROM_TOML)),
            to_toml: Some(Arc::clone(&HOOKS_TO_TOML)),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schema_accepts_strict_valid_hooks_and_rejects_constraints() {
        let valid = json!([{
            "event": "PreToolUse", "matcher": "Bash", "command": "check",
            "timeout": 600
        }]);
        assert_eq!(HOOKS_CONFIG_SCHEMA.parse(&valid).unwrap(), valid);
        for invalid in [
            json!({"event": "Stop", "command": "x"}),
            json!([{"event": "Unknown", "command": "x"}]),
            json!([{"event": "Stop", "command": ""}]),
            json!([{"event": "Stop", "command": "x", "timeout": 0}]),
            json!([{"event": "Stop", "command": "x", "future": true}]),
        ] {
            assert!(HOOKS_CONFIG_SCHEMA.parse(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn parses_one_hook_for_manifest_consumers() {
        let hook = parse_hook_def_config(&json!({
            "event": "Stop", "command": "cleanup", "timeout": 2
        }))
        .unwrap();
        assert_eq!(hook.event, HookEventType::Stop);
        assert_eq!(hook.command, "cleanup");
        assert!(parse_hook_def_config(&json!({"event": "Stop", "command": ""})).is_err());
    }

    #[test]
    fn transforms_each_hook_shallowly_between_toml_and_memory_keys() {
        let toml = json!([{
            "event": "PostToolUse", "command": "check", "timeout": 10,
            "future_key": {"nested_key": true}
        }]);
        let memory = hooks_from_toml(&toml);
        assert_eq!(memory[0]["futureKey"]["nested_key"], true);
        let written = hooks_to_toml(&memory, &json!(null)).unwrap();
        assert_eq!(written[0]["future_key"]["nested_key"], true);
    }
}
