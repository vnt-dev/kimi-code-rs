//! Task configuration schema, legacy merge, and environment binding.
//!
//! Original: `packages/agent-core-v2/src/agent/task/configSection.ts`.

use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    _base::utils::env::parse_boolean_env,
    app::config::{
        AnyEnvBindings, ConfigSchema, ConfigServiceHandle, ConfigValidationError, EnvBinding,
        RegisterSectionOptions, register_config_section,
    },
};

pub const TASK_SECTION: &str = "task";
pub const LEGACY_BACKGROUND_SECTION: &str = "background";
pub const KEEP_ALIVE_ON_EXIT_ENV: &str = "KIMI_CODE_BACKGROUND_KEEP_ALIVE_ON_EXIT";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PrintBackgroundMode {
    Exit,
    Drain,
    Steer,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_running_tasks: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive_on_exit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bash_auto_background_on_timeout: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kill_grace_period_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_wait_ceiling_s: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_background_mode: Option<PrintBackgroundMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_max_turns: Option<u64>,
}

pub static AGENT_TASK_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let object = value
            .as_object()
            .ok_or_else(|| ConfigValidationError::new("task config must be an object"))?;
        let config = AgentTaskConfig {
            max_running_tasks: optional_integer(object, "maxRunningTasks", 1)?,
            keep_alive_on_exit: optional_boolean(object, "keepAliveOnExit")?,
            bash_auto_background_on_timeout: optional_boolean(
                object,
                "bashAutoBackgroundOnTimeout",
            )?,
            kill_grace_period_ms: optional_integer(object, "killGracePeriodMs", 0)?,
            print_wait_ceiling_s: optional_integer(object, "printWaitCeilingS", 1)?,
            print_background_mode: optional_mode(object.get("printBackgroundMode"))?,
            print_max_turns: optional_integer(object, "printMaxTurns", 1)?,
        };
        serde_json::to_value(config).map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

pub static TASK_ENV_BINDINGS: LazyLock<Arc<AnyEnvBindings>> = LazyLock::new(|| {
    Arc::new(AnyEnvBindings::Fields(IndexMap::from([(
        "keepAliveOnExit".into(),
        AnyEnvBindings::Binding(EnvBinding::Parsed {
            env: KEEP_ALIVE_ON_EXIT_ENV.into(),
            parse: Some(Arc::new(|raw| {
                Ok(parse_boolean_env(Some(raw)).map(Value::Bool))
            })),
            default: None,
        }),
    )])))
});

fn optional_integer(
    object: &Map<String, Value>,
    key: &str,
    minimum: u64,
) -> Result<Option<u64>, ConfigValidationError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let integer = value.as_u64().ok_or_else(|| {
        ConfigValidationError::new(format!("{key} must be an integer of at least {minimum}"))
    })?;
    if integer < minimum {
        return Err(ConfigValidationError::new(format!(
            "{key} must be an integer of at least {minimum}"
        )));
    }
    Ok(Some(integer))
}

fn optional_boolean(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, ConfigValidationError> {
    object.get(key).map_or(Ok(None), |value| {
        value
            .as_bool()
            .map(Some)
            .ok_or_else(|| ConfigValidationError::new(format!("{key} must be a boolean")))
    })
}

fn optional_mode(
    value: Option<&Value>,
) -> Result<Option<PrintBackgroundMode>, ConfigValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|_| {
            ConfigValidationError::new("printBackgroundMode must be exit, drain, or steer")
        })
}

// Original: configSection.ts, resolveAgentTaskConfig(). Current fields replace
// matching legacy fields while legacy-only fields remain effective.
pub fn resolve_agent_task_config(config: &ConfigServiceHandle) -> Option<AgentTaskConfig> {
    resolve_agent_task_config_values(
        config.get(LEGACY_BACKGROUND_SECTION).as_ref(),
        config.get(TASK_SECTION).as_ref(),
    )
}

fn resolve_agent_task_config_values(
    legacy: Option<&Value>,
    current: Option<&Value>,
) -> Option<AgentTaskConfig> {
    let legacy = legacy.and_then(parse_config_value);
    let current = current.and_then(parse_config_value);
    match (legacy, current) {
        (None, current) => current,
        (legacy, None) => legacy,
        (Some(mut legacy), Some(current)) => {
            overlay(&mut legacy.max_running_tasks, current.max_running_tasks);
            overlay(&mut legacy.keep_alive_on_exit, current.keep_alive_on_exit);
            overlay(
                &mut legacy.bash_auto_background_on_timeout,
                current.bash_auto_background_on_timeout,
            );
            overlay(
                &mut legacy.kill_grace_period_ms,
                current.kill_grace_period_ms,
            );
            overlay(
                &mut legacy.print_wait_ceiling_s,
                current.print_wait_ceiling_s,
            );
            overlay(
                &mut legacy.print_background_mode,
                current.print_background_mode,
            );
            overlay(&mut legacy.print_max_turns, current.print_max_turns);
            Some(legacy)
        }
    }
}

fn parse_config_value(value: &Value) -> Option<AgentTaskConfig> {
    AGENT_TASK_CONFIG_SCHEMA
        .parse(value)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn overlay<T>(target: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *target = value;
    }
}

// Original: configSection.ts, resolvePrintBackgroundMode().
pub fn resolve_print_background_mode(config: &ConfigServiceHandle) -> PrintBackgroundMode {
    resolve_print_background_mode_value(resolve_agent_task_config(config).as_ref())
}

fn resolve_print_background_mode_value(config: Option<&AgentTaskConfig>) -> PrintBackgroundMode {
    config
        .and_then(|config| config.print_background_mode)
        .unwrap_or_else(|| {
            if config.is_some_and(|config| config.keep_alive_on_exit == Some(true)) {
                PrintBackgroundMode::Drain
            } else {
                PrintBackgroundMode::Exit
            }
        })
}

// Original: configSection.ts, the two registerConfigSection() calls.
pub fn register_task_config_sections() {
    for section in [TASK_SECTION, LEGACY_BACKGROUND_SECTION] {
        register_config_section(
            section,
            AGENT_TASK_CONFIG_SCHEMA.clone(),
            RegisterSectionOptions {
                env: Some(TASK_ENV_BINDINGS.clone()),
                ..RegisterSectionOptions::default()
            },
        );
    }
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
    fn schema_strips_unknown_fields_and_enforces_every_constraint() {
        assert_eq!(
            AGENT_TASK_CONFIG_SCHEMA
                .parse(&json!({
                    "maxRunningTasks": 2,
                    "keepAliveOnExit": true,
                    "bashAutoBackgroundOnTimeout": false,
                    "killGracePeriodMs": 0,
                    "printWaitCeilingS": 1,
                    "printBackgroundMode": "steer",
                    "printMaxTurns": 3,
                    "future": true
                }))
                .unwrap(),
            json!({
                "maxRunningTasks": 2,
                "keepAliveOnExit": true,
                "bashAutoBackgroundOnTimeout": false,
                "killGracePeriodMs": 0,
                "printWaitCeilingS": 1,
                "printBackgroundMode": "steer",
                "printMaxTurns": 3
            })
        );
        for invalid in [
            json!(null),
            json!({"maxRunningTasks": 0}),
            json!({"killGracePeriodMs": -1}),
            json!({"printWaitCeilingS": 1.5}),
            json!({"printMaxTurns": null}),
            json!({"keepAliveOnExit": "true"}),
            json!({"printBackgroundMode": "wait"}),
        ] {
            assert!(
                AGENT_TASK_CONFIG_SCHEMA.parse(&invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn current_section_overrides_matching_legacy_fields_only() {
        let resolved = resolve_agent_task_config_values(
            Some(&json!({
                "maxRunningTasks": 2,
                "keepAliveOnExit": true,
                "killGracePeriodMs": 20
            })),
            Some(&json!({
                "maxRunningTasks": 4,
                "printMaxTurns": 7
            })),
        )
        .unwrap();
        assert_eq!(resolved.max_running_tasks, Some(4));
        assert_eq!(resolved.keep_alive_on_exit, Some(true));
        assert_eq!(resolved.kill_grace_period_ms, Some(20));
        assert_eq!(resolved.print_max_turns, Some(7));
    }

    #[test]
    fn print_mode_prefers_explicit_mode_then_legacy_keep_alive_mapping() {
        assert_eq!(
            resolve_print_background_mode_value(None),
            PrintBackgroundMode::Exit
        );
        let mut config = AgentTaskConfig {
            keep_alive_on_exit: Some(true),
            ..AgentTaskConfig::default()
        };
        assert_eq!(
            resolve_print_background_mode_value(Some(&config)),
            PrintBackgroundMode::Drain
        );
        config.print_background_mode = Some(PrintBackgroundMode::Steer);
        config.keep_alive_on_exit = Some(false);
        assert_eq!(
            resolve_print_background_mode_value(Some(&config)),
            PrintBackgroundMode::Steer
        );
    }

    #[test]
    fn boolean_environment_override_ignores_unrecognized_values() {
        let base = json!({"keepAliveOnExit": true});
        assert_eq!(
            apply_section_env(
                Some(&base),
                &TASK_ENV_BINDINGS,
                &getter(HashMap::from([(
                    KEEP_ALIVE_ON_EXIT_ENV.into(),
                    "off".into()
                )]))
            )
            .unwrap(),
            Some(json!({"keepAliveOnExit": false}))
        );
        assert_eq!(
            apply_section_env(
                Some(&base),
                &TASK_ENV_BINDINGS,
                &getter(HashMap::from([(
                    KEEP_ALIVE_ON_EXIT_ENV.into(),
                    "invalid".into()
                )]))
            )
            .unwrap(),
            Some(base)
        );
    }

    #[test]
    fn registers_both_sections_with_the_same_schema_and_environment_binding() {
        crate::app::config::clear_config_section_contributions_for_tests();
        register_task_config_sections();
        let contributions = crate::app::config::get_config_section_contributions();
        assert_eq!(
            contributions
                .iter()
                .map(|entry| entry.domain.as_str())
                .collect::<Vec<_>>(),
            [TASK_SECTION, LEGACY_BACKGROUND_SECTION]
        );
        assert!(
            contributions
                .iter()
                .all(|entry| entry.options.env.is_some())
        );
        crate::app::config::clear_config_section_contributions_for_tests();
    }
}
