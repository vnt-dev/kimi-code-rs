//! Cron operational configuration and environment bindings.
//!
//! Original: `packages/agent-core-v2/src/app/cron/configSection.ts`.

use std::{
    str::FromStr,
    sync::{Arc, LazyLock},
};

use indexmap::IndexMap;
use serde_json::{Number, Value};

use crate::app::config::{
    AnyEnvBindings, ConfigSchema, ConfigStripEnv, EnvBinding, RegisterSectionOptions,
    register_config_section,
};

pub const CRON_SECTION: &str = "cron";

// Original: DEFAULT_CRON_CONFIG.
pub static DEFAULT_CRON_CONFIG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::json!({
        "debug": false,
        "noJitter": false,
        "noStale": false,
        "disabled": false,
        "manualTick": false,
    })
});

// The source schema is an unchecked TypeScript cast, so no Rust structural
// validation is introduced at this boundary.
pub static CRON_CONFIG_SCHEMA: LazyLock<ConfigSchema> =
    LazyLock::new(|| ConfigSchema::new(|value| Ok(value.clone())));

fn on(raw: &str) -> Option<Value> {
    Some(Value::Bool(raw == "1"))
}

// Original: parsePollIntervalMs(). `None` deliberately means JavaScript
// undefined, which the config env layer preserves as the existing field.
fn parse_poll_interval_ms(raw: &str) -> Option<Value> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value == "null" {
        return Some(Value::Null);
    }
    let parsed = f64::from_str(value).ok()?;
    if !parsed.is_finite() || parsed.fract() != 0.0 || parsed < 0.0 {
        return None;
    }
    if parsed <= u64::MAX as f64 {
        Some(Value::Number(Number::from(parsed as u64)))
    } else {
        Number::from_f64(parsed).map(Value::Number)
    }
}

pub static CRON_ENV_BINDINGS: LazyLock<Arc<AnyEnvBindings>> = LazyLock::new(|| {
    let bool_binding = |env: &str| {
        AnyEnvBindings::Binding(EnvBinding::Parsed {
            env: env.into(),
            parse: Some(Arc::new(|raw| Ok(on(raw)))),
            default: None,
        })
    };
    Arc::new(AnyEnvBindings::Fields(IndexMap::from([
        ("debug".into(), bool_binding("KIMI_CRON_DEBUG")),
        ("noJitter".into(), bool_binding("KIMI_CRON_NO_JITTER")),
        ("noStale".into(), bool_binding("KIMI_CRON_NO_STALE")),
        ("disabled".into(), bool_binding("KIMI_DISABLE_CRON")),
        ("manualTick".into(), bool_binding("KIMI_CRON_MANUAL_TICK")),
        (
            "clock".into(),
            AnyEnvBindings::Binding(EnvBinding::Name("KIMI_CRON_CLOCK".into())),
        ),
        (
            "pollIntervalMs".into(),
            AnyEnvBindings::Binding(EnvBinding::Parsed {
                env: "KIMI_CRON_POLL_INTERVAL_MS".into(),
                parse: Some(Arc::new(|raw| Ok(parse_poll_interval_ms(raw)))),
                default: None,
            }),
        ),
    ])))
});

// Original: stripCronEnv(). No cron operational value is persisted.
pub static STRIP_CRON_ENV: LazyLock<ConfigStripEnv> = LazyLock::new(|| Arc::new(|_, _| None));

pub fn register_cron_config_section() {
    register_config_section(
        CRON_SECTION,
        CRON_CONFIG_SCHEMA.clone(),
        RegisterSectionOptions {
            default_value: Some(DEFAULT_CRON_CONFIG.clone()),
            env: Some(Arc::clone(&CRON_ENV_BINDINGS)),
            strip_env: Some(STRIP_CRON_ENV.clone()),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::apply_section_env;
    use std::collections::HashMap;

    #[test]
    fn environment_binds_boolean_poll_and_clock_values_without_persisting_them() {
        let env: HashMap<String, String> = HashMap::from([
            ("KIMI_CRON_DEBUG".into(), "1".into()),
            ("KIMI_CRON_NO_JITTER".into(), "true".into()),
            ("KIMI_CRON_CLOCK".into(), "file:/tmp/clock".into()),
            ("KIMI_CRON_POLL_INTERVAL_MS".into(), "2500".into()),
        ]);
        let value = apply_section_env(Some(&DEFAULT_CRON_CONFIG), &CRON_ENV_BINDINGS, &|key| {
            env.get(key).cloned()
        })
        .unwrap()
        .unwrap();
        assert_eq!(value["debug"], true);
        assert_eq!(value["noJitter"], false);
        assert_eq!(value["pollIntervalMs"], 2500.0);
        assert_eq!(value["clock"], "file:/tmp/clock");
        assert_eq!((STRIP_CRON_ENV)(&value, None), None);
    }

    #[test]
    fn poll_parser_preserves_null_and_ignores_invalid_values() {
        assert_eq!(parse_poll_interval_ms("null"), Some(Value::Null));
        assert_eq!(
            parse_poll_interval_ms("0"),
            Some(Value::Number(Number::from(0)))
        );
        for invalid in ["", "-1", "1.5", "Infinity", "bad"] {
            assert_eq!(parse_poll_interval_ms(invalid), None);
        }
    }
}
