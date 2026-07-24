//! Agent-file catalog configuration sections.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/configSection.ts`.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::app::config::{
    ConfigSchema, ConfigValidationError, RegisterSectionOptions, register_config_section,
};

pub const EXTRA_AGENT_DIRS_SECTION: &str = "extraAgentDirs";

pub static EXTRA_AGENT_DIRS_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let Some(directories) = value.as_array() else {
            return Err(ConfigValidationError::new(
                "expected extra agent directories to be an array",
            ));
        };
        if directories.iter().all(Value::is_string) {
            Ok(value.clone())
        } else {
            Err(ConfigValidationError::new(
                "expected every extra agent directory to be a string",
            ))
        }
    })
});

fn extra_agent_dirs_options() -> RegisterSectionOptions {
    RegisterSectionOptions {
        default_value: Some(json!([])),
        ..RegisterSectionOptions::default()
    }
}

// Original top-level registerConfigSection().
pub fn register_agent_file_catalog_config_sections() {
    register_config_section(
        EXTRA_AGENT_DIRS_SECTION,
        EXTRA_AGENT_DIRS_CONFIG_SCHEMA.clone(),
        extra_agent_dirs_options(),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::app::config::camel_to_snake;

    use super::*;

    #[test]
    fn extra_agent_dirs_accepts_only_string_arrays_and_defaults_empty() {
        let valid = json!(["/one", "relative/two"]);
        assert_eq!(EXTRA_AGENT_DIRS_CONFIG_SCHEMA.parse(&valid).unwrap(), valid);
        assert!(
            EXTRA_AGENT_DIRS_CONFIG_SCHEMA
                .parse(&json!(["/one", 2]))
                .is_err()
        );
        assert!(EXTRA_AGENT_DIRS_CONFIG_SCHEMA.parse(&json!(null)).is_err());
        assert_eq!(extra_agent_dirs_options().default_value, Some(json!([])));
        assert_eq!(EXTRA_AGENT_DIRS_SECTION, "extraAgentDirs");
        assert_eq!(camel_to_snake(EXTRA_AGENT_DIRS_SECTION), "extra_agent_dirs");
    }
}
