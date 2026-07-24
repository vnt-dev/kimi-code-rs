//! Subagent timeout configuration, environment binding, and presentation.
//!
//! Original: `session/subagent/configSection.ts`.

use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::config::{
    AnyEnvBindings, ConfigSchema, ConfigServiceContract, ConfigValidationError, EnvBinding,
    RegisterSectionOptions, register_config_section,
};

pub const SUBAGENT_SECTION: &str = "subagent";
pub const DEFAULT_SUBAGENT_TIMEOUT_MS: u64 = 2 * 60 * 60 * 1_000;
pub const SUBAGENT_TIMEOUT_ENV: &str = "KIMI_SUBAGENT_TIMEOUT_MS";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentConfig {
    /// Per-run subagent timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

pub static SUBAGENT_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let object = value
            .as_object()
            .ok_or_else(|| ConfigValidationError::new("subagent config must be an object"))?;
        let timeout_ms = match object.get("timeoutMs") {
            None => None,
            Some(value) => {
                let timeout_ms = value.as_u64().filter(|value| *value >= 1).ok_or_else(|| {
                    ConfigValidationError::new("timeoutMs must be an integer of at least 1")
                })?;
                Some(timeout_ms)
            }
        };
        serde_json::to_value(SubagentConfig { timeout_ms })
            .map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

pub static SUBAGENT_ENV_BINDINGS: LazyLock<Arc<AnyEnvBindings>> = LazyLock::new(|| {
    Arc::new(AnyEnvBindings::Fields(IndexMap::from([(
        "timeoutMs".into(),
        AnyEnvBindings::Binding(EnvBinding::Parsed {
            env: SUBAGENT_TIMEOUT_ENV.into(),
            parse: Some(Arc::new(|raw| {
                Ok(parse_timeout_ms_env(raw).map(Value::from))
            })),
            default: None,
        }),
    )])))
});

/// Original: `parseTimeoutMsEnv()`.
///
/// Invalid values are ignored, rather than reported as configuration errors.
pub fn parse_timeout_ms_env(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let parsed = if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok().map(|value| value as f64)
    } else if let Some(octal) = raw.strip_prefix("0o").or_else(|| raw.strip_prefix("0O")) {
        u64::from_str_radix(octal, 8).ok().map(|value| value as f64)
    } else if let Some(binary) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
        u64::from_str_radix(binary, 2)
            .ok()
            .map(|value| value as f64)
    } else {
        raw.parse::<f64>().ok()
    }?;
    (parsed.is_finite() && parsed.fract() == 0.0 && parsed >= 1.0 && parsed <= u64::MAX as f64)
        .then_some(parsed as u64)
}

/// Original: `resolveSubagentTimeoutMs()`.
pub fn resolve_subagent_timeout_ms(config: &dyn ConfigServiceContract) -> u64 {
    config
        .get(SUBAGENT_SECTION)
        .and_then(|value| serde_json::from_value::<SubagentConfig>(value).ok())
        .and_then(|config| config.timeout_ms)
        .unwrap_or(DEFAULT_SUBAGENT_TIMEOUT_MS)
}

/// Original: `formatSubagentTimeoutDescription()`.
pub fn format_subagent_timeout_description(ms: u64) -> String {
    const HOUR_MS: u64 = 60 * 60 * 1_000;
    const MINUTE_MS: u64 = 60 * 1_000;
    if ms.is_multiple_of(HOUR_MS) {
        let hours = ms / HOUR_MS;
        return format!("{hours} hour{}", if hours == 1 { "" } else { "s" });
    }
    if ms.is_multiple_of(MINUTE_MS) {
        let minutes = ms / MINUTE_MS;
        return format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" });
    }
    if ms.is_multiple_of(1_000) {
        let seconds = ms / 1_000;
        return format!("{seconds} second{}", if seconds == 1 { "" } else { "s" });
    }
    format!("{ms} ms")
}

pub fn register_subagent_config_section() {
    register_config_section(
        SUBAGENT_SECTION,
        SUBAGENT_CONFIG_SCHEMA.clone(),
        RegisterSectionOptions {
            default_value: Some(serde_json::json!({
                "timeoutMs": DEFAULT_SUBAGENT_TIMEOUT_MS,
            })),
            env: Some(Arc::clone(&SUBAGENT_ENV_BINDINGS)),
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
    fn schema_accepts_only_positive_integer_timeout_and_strips_unknown_fields() {
        assert_eq!(
            SUBAGENT_CONFIG_SCHEMA
                .parse(&json!({"timeoutMs": 10, "future": true}))
                .unwrap(),
            json!({"timeoutMs": 10})
        );
        for invalid in [
            json!(null),
            json!({"timeoutMs": 0}),
            json!({"timeoutMs": 1.5}),
        ] {
            assert!(SUBAGENT_CONFIG_SCHEMA.parse(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn environment_override_ignores_non_positive_or_non_integer_values() {
        let get_env = |value: &str| {
            let values = HashMap::from([(SUBAGENT_TIMEOUT_ENV.to_owned(), value.to_owned())]);
            move |name: &str| values.get(name).cloned()
        };
        assert_eq!(parse_timeout_ms_env(" 1e3 "), Some(1_000));
        assert_eq!(parse_timeout_ms_env("0x10"), Some(16));
        assert_eq!(parse_timeout_ms_env("1.5"), None);
        assert_eq!(parse_timeout_ms_env("0"), None);
        assert_eq!(
            apply_section_env(
                Some(&json!({"timeoutMs": 15})),
                &SUBAGENT_ENV_BINDINGS,
                &get_env("900")
            )
            .unwrap(),
            Some(json!({"timeoutMs": 900}))
        );
        assert_eq!(
            apply_section_env(
                Some(&json!({"timeoutMs": 15})),
                &SUBAGENT_ENV_BINDINGS,
                &get_env("invalid")
            )
            .unwrap(),
            Some(json!({"timeoutMs": 15}))
        );
    }

    #[test]
    fn formats_source_duration_units_and_registers_default() {
        assert_eq!(format_subagent_timeout_description(7_200_000), "2 hours");
        assert_eq!(format_subagent_timeout_description(60_000), "1 minute");
        assert_eq!(format_subagent_timeout_description(2_000), "2 seconds");
        assert_eq!(format_subagent_timeout_description(999), "999 ms");

        crate::app::config::clear_config_section_contributions_for_tests();
        register_subagent_config_section();
        let contributions = crate::app::config::get_config_section_contributions();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].domain, SUBAGENT_SECTION);
        assert_eq!(
            contributions[0].options.default_value,
            Some(json!({"timeoutMs": DEFAULT_SUBAGENT_TIMEOUT_MS}))
        );
        crate::app::config::clear_config_section_contributions_for_tests();
    }
}
