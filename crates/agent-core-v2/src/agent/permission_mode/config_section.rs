use std::sync::LazyLock;

use crate::{
    agent::permission_policy::PermissionMode,
    app::config::{
        ConfigSchema, ConfigValidationError, RegisterSectionOptions, register_config_section,
    },
};

pub const DEFAULT_PERMISSION_MODE_SECTION: &str = "defaultPermissionMode";

// Original:
//   packages/agent-core-v2/src/agent/permissionMode/configSection.ts
//   DefaultPermissionModeSchema
pub static DEFAULT_PERMISSION_MODE_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        serde_json::from_value::<PermissionMode>(value.clone())
            .map_err(|_| {
                ConfigValidationError::new("defaultPermissionMode must be manual, auto, or yolo")
            })
            .and_then(|mode| {
                serde_json::to_value(mode)
                    .map_err(|error| ConfigValidationError::new(error.to_string()))
            })
    })
});

pub fn register_default_permission_mode_config_section() {
    register_config_section(
        DEFAULT_PERMISSION_MODE_SECTION,
        DEFAULT_PERMISSION_MODE_SCHEMA.clone(),
        RegisterSectionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schema_accepts_only_the_three_permission_modes() {
        for mode in ["manual", "auto", "yolo"] {
            assert_eq!(
                DEFAULT_PERMISSION_MODE_SCHEMA.parse(&json!(mode)).unwrap(),
                json!(mode)
            );
        }
        for invalid in [json!(null), json!("ask"), json!(1), json!({})] {
            assert!(
                DEFAULT_PERMISSION_MODE_SCHEMA.parse(&invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn registration_contributes_the_scalar_schema() {
        crate::app::config::clear_config_section_contributions_for_tests();
        register_default_permission_mode_config_section();
        let contributions = crate::app::config::get_config_section_contributions();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].domain, DEFAULT_PERMISSION_MODE_SECTION);
        assert_eq!(
            contributions[0].schema.parse(&json!("auto")).unwrap(),
            "auto"
        );
        crate::app::config::clear_config_section_contributions_for_tests();
    }
}
