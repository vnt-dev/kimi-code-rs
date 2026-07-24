//! Model-catalog discovery configuration section.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/discoveryConfigSection.ts`.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::app::config::{
    ConfigSchema, ConfigValidationError, RegisterSectionOptions, register_config_section,
};

pub const MODEL_CATALOG_SECTION: &str = "modelCatalog";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_on_start: Option<bool>,
}

pub static MODEL_CATALOG_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let config = serde_json::from_value::<ModelCatalogConfig>(value.clone())
            .map_err(|error| ConfigValidationError::new(error.to_string()))?;
        serde_json::to_value(config).map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

// Original: module-load `registerConfigSection(MODEL_CATALOG_SECTION, ...)`.
// Registration is explicit at the Rust composition root rather than import
// side effect.
pub fn register_model_catalog_config_section() {
    register_config_section(
        MODEL_CATALOG_SECTION,
        MODEL_CATALOG_CONFIG_SCHEMA.clone(),
        RegisterSectionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schema_preserves_optional_refresh_settings_and_rejects_invalid_values() {
        let value = json!({"refreshIntervalMs": 0, "refreshOnStart": true});
        assert_eq!(MODEL_CATALOG_CONFIG_SCHEMA.parse(&value).unwrap(), value);

        for invalid in [
            json!({"refreshIntervalMs": -1}),
            json!({"refreshIntervalMs": 0.5}),
            json!({"refreshOnStart": "yes"}),
        ] {
            assert!(MODEL_CATALOG_CONFIG_SCHEMA.parse(&invalid).is_err());
        }
    }
}
