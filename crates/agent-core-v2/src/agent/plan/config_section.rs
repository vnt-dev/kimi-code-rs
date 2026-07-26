//! Default plan-mode configuration.
//!
//! Original: `packages/agent-core-v2/src/agent/plan/configSection.ts`.

use std::sync::LazyLock;

use serde_json::Value;

use crate::app::config::{
    ConfigSchema, ConfigValidationError, RegisterSectionOptions, register_config_section,
};

/// Effective-config key. The config layer maps this camel-case domain to the
/// v1-compatible top-level TOML key `default_plan_mode`.
pub const DEFAULT_PLAN_MODE_SECTION: &str = "defaultPlanMode";

/// The source type is `boolean | undefined`. An absent section is represented
/// by absence in the config layer; whenever a JSON value reaches this schema,
/// it must therefore be a boolean.
pub type DefaultPlanMode = Option<bool>;

pub static DEFAULT_PLAN_MODE_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        if value.is_boolean() {
            Ok(value.clone())
        } else {
            Err(ConfigValidationError::new(
                "defaultPlanMode must be a boolean",
            ))
        }
    })
});

/// Registers `defaultPlanMode` with the same `false` default as the source.
pub fn register_default_plan_mode_config_section() {
    register_config_section(
        DEFAULT_PLAN_MODE_SECTION,
        DEFAULT_PLAN_MODE_SCHEMA.clone(),
        RegisterSectionOptions {
            default_value: Some(Value::Bool(false)),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schema_accepts_only_booleans() {
        assert_eq!(
            DEFAULT_PLAN_MODE_SCHEMA.parse(&json!(true)).unwrap(),
            json!(true)
        );
        assert_eq!(
            DEFAULT_PLAN_MODE_SCHEMA.parse(&json!(false)).unwrap(),
            json!(false)
        );

        for invalid in [json!(null), json!(0), json!("true"), json!({}), json!([])] {
            assert!(
                DEFAULT_PLAN_MODE_SCHEMA.parse(&invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn registration_contributes_the_false_default() {
        crate::app::config::clear_config_section_contributions_for_tests();
        register_default_plan_mode_config_section();

        let contributions = crate::app::config::get_config_section_contributions();
        assert_eq!(contributions.len(), 1);
        let contribution = &contributions[0];
        assert_eq!(contribution.domain, DEFAULT_PLAN_MODE_SECTION);
        assert_eq!(contribution.options.default_value, Some(json!(false)));
        assert_eq!(
            contribution.schema.parse(&json!(true)).unwrap(),
            json!(true)
        );

        crate::app::config::clear_config_section_contributions_for_tests();
    }
}
