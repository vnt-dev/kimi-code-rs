//! Experimental feature resolution contract and config section.
//!
//! Original: `packages/agent-core-v2/src/app/flag/flag.ts`.

use std::{
    ops::Deref,
    sync::{Arc, LazyLock},
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    _base::di::instantiation::ServiceIdentifier,
    app::config::{
        ConfigFromToml, ConfigSchema, ConfigToToml, ConfigValidationError, RegisterSectionOptions,
        register_config_section,
    },
};

use super::flag_registry::{FlagId, FlagRegistry, FlagSurface};

pub type ExperimentalFlagMap = IndexMap<String, bool>;
pub type ExperimentalFlagConfig = IndexMap<FlagId, bool>;

pub const EXPERIMENTAL_SECTION: &str = "experimental";

static EXPERIMENTAL_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let Some(object) = value.as_object() else {
            return Err(ConfigValidationError::new(
                "expected a record of boolean flags",
            ));
        };
        if object.values().all(Value::is_boolean) {
            Ok(value.clone())
        } else {
            Err(ConfigValidationError::new(
                "expected every experimental flag value to be boolean",
            ))
        }
    })
});

static EXPERIMENTAL_FROM_TOML: LazyLock<ConfigFromToml> =
    LazyLock::new(|| Arc::new(experimental_from_toml));
static EXPERIMENTAL_TO_TOML: LazyLock<ConfigToToml> =
    LazyLock::new(|| Arc::new(experimental_to_toml));

// Original: experimentalFromToml(). Flag ids are preserved verbatim.
pub fn experimental_from_toml(raw_snake: &Value) -> Value {
    raw_snake.clone()
}

// Original: experimentalToToml(). `Some` represents every source value here;
// undefined object entries cannot exist in Serde JSON and are already absent.
pub fn experimental_to_toml(value: &Value, _raw_snake: &Value) -> Option<Value> {
    Some(match value.as_object() {
        Some(object) => Value::Object(object.clone()),
        None => value.clone(),
    })
}

// Original top-level registerConfigSection(...). Rust composition roots call
// this before constructing ConfigRegistry; repeated calls are idempotent when
// drained because all contributed Arc identities are stable.
pub fn register_experimental_config_section() {
    register_config_section(
        EXPERIMENTAL_SECTION,
        EXPERIMENTAL_SCHEMA.clone(),
        RegisterSectionOptions {
            from_toml: Some(Arc::clone(&EXPERIMENTAL_FROM_TOML)),
            to_toml: Some(Arc::clone(&EXPERIMENTAL_TO_TOML)),
            ..RegisterSectionOptions::default()
        },
    );
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExperimentalFlagSource {
    MasterEnv,
    Env,
    Config,
    Default,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentalFeatureState {
    pub id: FlagId,
    pub title: String,
    pub description: String,
    pub surface: FlagSurface,
    pub env: String,
    pub default_enabled: bool,
    pub enabled: bool,
    pub source: ExperimentalFlagSource,
    pub config_value: Option<bool>,
}

pub trait FlagServiceContract: crate::_base::di::lifecycle::Disposable + Send + Sync {
    fn registry(&self) -> Arc<dyn FlagRegistry>;
    fn enabled(&self, id: &str) -> bool;
    fn snapshot(&self) -> ExperimentalFlagMap;
    fn enabled_ids(&self) -> Vec<FlagId>;
    fn explain(&self, id: &str) -> Option<ExperimentalFeatureState>;
    fn explain_all(&self) -> Vec<ExperimentalFeatureState>;
    fn set_config_overrides(&self, overrides: Option<ExperimentalFlagConfig>);
}

#[derive(Clone)]
pub struct FlagServiceHandle(pub Arc<dyn FlagServiceContract>);

impl Deref for FlagServiceHandle {
    type Target = dyn FlagServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl crate::_base::di::lifecycle::Disposable for FlagServiceHandle {
    fn dispose(&self) -> crate::_base::di::lifecycle::DisposeResult {
        self.0.dispose()
    }
}

pub const FLAG_SERVICE_ID: ServiceIdentifier<FlagServiceHandle> =
    ServiceIdentifier::new("flagService");

pub fn experimental_config_to_value(config: &ExperimentalFlagConfig) -> Value {
    Value::Object(Map::from_iter(
        config
            .iter()
            .map(|(id, enabled)| (id.clone(), Value::Bool(*enabled))),
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schema_accepts_only_boolean_records_and_preserves_ids() {
        let valid = json!({"keep_snake": true, "Mixed-ID": false});
        assert_eq!(EXPERIMENTAL_SCHEMA.parse(&valid).unwrap(), valid);
        assert!(EXPERIMENTAL_SCHEMA.parse(&json!({"bad": "yes"})).is_err());
        assert!(EXPERIMENTAL_SCHEMA.parse(&json!([])).is_err());
        assert_eq!(experimental_from_toml(&valid), valid);
        assert_eq!(experimental_to_toml(&valid, &json!({})), Some(valid));
    }

    #[test]
    fn service_identifier_preserves_original_name() {
        assert_eq!(FLAG_SERVICE_ID.to_string(), "flagService");
    }
}
