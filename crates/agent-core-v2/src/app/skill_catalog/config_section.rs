//! Skill catalog configuration sections.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/configSection.ts`.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::app::config::{
    ConfigSchema, ConfigValidationError, RegisterSectionOptions, register_config_section,
};

pub const EXTRA_SKILL_DIRS_SECTION: &str = "extraSkillDirs";
pub const MERGE_ALL_AVAILABLE_SKILLS_SECTION: &str = "mergeAllAvailableSkills";

pub static EXTRA_SKILL_DIRS_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let Some(directories) = value.as_array() else {
            return Err(ConfigValidationError::new(
                "expected extra skill directories to be an array",
            ));
        };

        if directories.iter().all(Value::is_string) {
            Ok(value.clone())
        } else {
            Err(ConfigValidationError::new(
                "expected every extra skill directory to be a string",
            ))
        }
    })
});

pub static MERGE_ALL_AVAILABLE_SKILLS_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        if value.is_boolean() {
            Ok(value.clone())
        } else {
            Err(ConfigValidationError::new(
                "expected merge-all-available-skills to be a boolean",
            ))
        }
    })
});

fn extra_skill_dirs_options() -> RegisterSectionOptions {
    RegisterSectionOptions {
        default_value: Some(json!([])),
        ..RegisterSectionOptions::default()
    }
}

fn merge_all_available_skills_options() -> RegisterSectionOptions {
    RegisterSectionOptions {
        default_value: Some(Value::Bool(true)),
        ..RegisterSectionOptions::default()
    }
}

// Original top-level registerConfigSection() calls. The composition root calls
// this before constructing ConfigRegistry. Generic config TOML conversion keeps
// these domains camelCase in memory and snake_case on disk.
pub fn register_skill_catalog_config_sections() {
    register_config_section(
        EXTRA_SKILL_DIRS_SECTION,
        EXTRA_SKILL_DIRS_CONFIG_SCHEMA.clone(),
        extra_skill_dirs_options(),
    );
    register_config_section(
        MERGE_ALL_AVAILABLE_SKILLS_SECTION,
        MERGE_ALL_AVAILABLE_SKILLS_CONFIG_SCHEMA.clone(),
        merge_all_available_skills_options(),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::app::config::camel_to_snake;

    use super::*;

    #[test]
    fn extra_skill_dirs_accepts_only_string_arrays() {
        let valid = json!(["/one", "relative/two"]);
        assert_eq!(EXTRA_SKILL_DIRS_CONFIG_SCHEMA.parse(&valid).unwrap(), valid);
        assert!(
            EXTRA_SKILL_DIRS_CONFIG_SCHEMA
                .parse(&json!(["/one", 2]))
                .is_err()
        );
        assert!(EXTRA_SKILL_DIRS_CONFIG_SCHEMA.parse(&json!(null)).is_err());
        assert_eq!(extra_skill_dirs_options().default_value, Some(json!([])));
    }

    #[test]
    fn merge_all_available_skills_accepts_only_booleans() {
        assert_eq!(
            MERGE_ALL_AVAILABLE_SKILLS_CONFIG_SCHEMA
                .parse(&json!(false))
                .unwrap(),
            json!(false)
        );
        assert!(
            MERGE_ALL_AVAILABLE_SKILLS_CONFIG_SCHEMA
                .parse(&json!("false"))
                .is_err()
        );
        assert_eq!(
            merge_all_available_skills_options().default_value,
            Some(json!(true))
        );
    }

    #[test]
    fn section_names_use_original_memory_and_toml_forms() {
        assert_eq!(EXTRA_SKILL_DIRS_SECTION, "extraSkillDirs");
        assert_eq!(camel_to_snake(EXTRA_SKILL_DIRS_SECTION), "extra_skill_dirs");
        assert_eq!(
            camel_to_snake(MERGE_ALL_AVAILABLE_SKILLS_SECTION),
            "merge_all_available_skills"
        );
    }
}
