use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::app::config::{
    ConfigSchema, ConfigValidationError, RegisterSectionOptions, register_config_section,
};

pub const TOOLS_SECTION: &str = "tools";

// Original:
//   packages/agent-core-v2/src/agent/toolPolicy/configSection.ts
//   ToolsConfigSchema / ToolsConfig
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ToolsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<Vec<String>>,
}

pub static TOOLS_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let object = value
            .as_object()
            .ok_or_else(|| ConfigValidationError::new("tools must be an object"))?;
        let config = ToolsConfig {
            enabled: parse_string_array(object.get("enabled"), "enabled")?,
            disabled: parse_string_array(object.get("disabled"), "disabled")?,
        };
        serde_json::to_value(config).map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

fn parse_string_array(
    value: Option<&serde_json::Value>,
    field: &str,
) -> Result<Option<Vec<String>>, ConfigValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let entries = value
        .as_array()
        .ok_or_else(|| ConfigValidationError::new(format!("tools.{field} must be an array")))?;
    entries
        .iter()
        .map(|entry| {
            entry.as_str().map(str::to_owned).ok_or_else(|| {
                ConfigValidationError::new(format!("tools.{field} entries must be strings"))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

// Original: configSection.ts, registerConfigSection(TOOLS_SECTION, ...).
pub fn register_tools_config_section() {
    register_config_section(
        TOOLS_SECTION,
        TOOLS_CONFIG_SCHEMA.clone(),
        RegisterSectionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schema_accepts_optional_string_arrays_and_strips_unknown_fields() {
        assert_eq!(TOOLS_CONFIG_SCHEMA.parse(&json!({})).unwrap(), json!({}));
        assert_eq!(
            TOOLS_CONFIG_SCHEMA
                .parse(&json!({
                    "enabled": ["Read", "mcp__github__*"],
                    "disabled": [],
                    "future": true
                }))
                .unwrap(),
            json!({
                "enabled": ["Read", "mcp__github__*"],
                "disabled": []
            })
        );
    }

    #[test]
    fn schema_rejects_non_objects_and_non_string_entries() {
        for invalid in [
            json!(null),
            json!([]),
            json!({"enabled": null}),
            json!({"disabled": "Read"}),
            json!({"enabled": ["Read", 1]}),
        ] {
            assert!(TOOLS_CONFIG_SCHEMA.parse(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn registration_contributes_the_tools_schema() {
        crate::app::config::clear_config_section_contributions_for_tests();
        register_tools_config_section();
        let contributions = crate::app::config::get_config_section_contributions();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].domain, TOOLS_SECTION);
        assert_eq!(
            contributions[0]
                .schema
                .parse(&json!({"enabled": ["Bash"]}))
                .unwrap(),
            json!({"enabled": ["Bash"]})
        );
        crate::app::config::clear_config_section_contributions_for_tests();
    }
}
