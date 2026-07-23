//! Configuration registry and layered service contracts.
//!
//! Original: `packages/agent-core-v2/src/app/config/config.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::_base::{
    di::{instantiation::ServiceIdentifier, lifecycle::Disposable},
    event::Event,
};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ConfigValidationError {
    pub message: String,
}

impl ConfigValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

type SchemaParser = dyn Fn(&Value) -> Result<Value, ConfigValidationError> + Send + Sync + 'static;

#[derive(Clone)]
pub struct ConfigSchema(Arc<SchemaParser>);

impl ConfigSchema {
    pub fn new(
        parser: impl Fn(&Value) -> Result<Value, ConfigValidationError> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(parser))
    }

    // Original: ConfigSchema.parse().
    pub fn parse(&self, value: &Value) -> Result<Value, ConfigValidationError> {
        (self.0)(value)
    }
}

impl PartialEq for ConfigSchema {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ConfigSchema {}

pub type ConfigMerge = Arc<dyn Fn(Option<&Value>, Option<&Value>) -> Option<Value> + Send + Sync>;
pub type ConfigStripEnv = Arc<dyn Fn(&Value, Option<&Value>) -> Option<Value> + Send + Sync>;
pub type ConfigFromToml = Arc<dyn Fn(&Value) -> Value + Send + Sync>;
pub type ConfigToToml = Arc<dyn Fn(&Value, &Value) -> Option<Value> + Send + Sync>;
pub type EnvParser =
    Arc<dyn Fn(&str) -> Result<Option<Value>, ConfigValidationError> + Send + Sync>;

#[derive(Clone)]
pub enum EnvBinding {
    Name(String),
    Parsed {
        env: String,
        parse: Option<EnvParser>,
        default: Option<Value>,
    },
}

#[derive(Clone)]
pub enum AnyEnvBindings {
    Binding(EnvBinding),
    Fields(IndexMap<String, AnyEnvBindings>),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigScope {
    #[default]
    Core,
    Session,
    Project,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigTarget {
    #[default]
    User,
    Memory,
}

#[derive(Clone, Default)]
pub struct RegisterSectionOptions {
    pub default_value: Option<Value>,
    pub merge: Option<ConfigMerge>,
    pub scope: Option<ConfigScope>,
    pub env: Option<Arc<AnyEnvBindings>>,
    pub strip_env: Option<ConfigStripEnv>,
    pub from_toml: Option<ConfigFromToml>,
    pub to_toml: Option<ConfigToToml>,
}

#[derive(Clone)]
pub struct ConfigSection {
    pub domain: String,
    pub schema: ConfigSchema,
    pub default_value: Option<Value>,
    pub merge: ConfigMerge,
    pub scope: ConfigScope,
    pub env: Option<Arc<AnyEnvBindings>>,
    pub strip_env: Option<ConfigStripEnv>,
    pub from_toml: Option<ConfigFromToml>,
    pub to_toml: Option<ConfigToToml>,
}

pub type GetEnv<'a> = dyn Fn(&str) -> Option<String> + 'a;
pub type ValidateConfig<'a> = dyn Fn(&str, &Value) -> Result<Value, ConfigValidationError> + 'a;

pub trait ConfigEffectiveOverlay: Send + Sync {
    fn apply(
        &self,
        effective: &mut Map<String, Value>,
        get_env: &GetEnv<'_>,
        validate: &ValidateConfig<'_>,
    ) -> Result<Vec<String>, ConfigValidationError>;

    fn strip(
        &self,
        _domain: &str,
        value: &Value,
        _raw_snake: &Map<String, Value>,
    ) -> Option<Value> {
        Some(value.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigSectionRegisteredEvent {
    pub domain: String,
}

#[derive(Clone)]
pub struct ConfigOverlayRegisteredEvent {
    pub overlay: Arc<dyn ConfigEffectiveOverlay>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfigRegistryError {
    #[error("ConfigRegistry: section '{0}' is already registered")]
    AlreadyRegistered(String),

    #[error("invalid config section '{domain}': {source}")]
    Invalid {
        domain: String,
        source: ConfigValidationError,
    },
}

pub trait ConfigRegistryContract: Send + Sync {
    fn on_did_register_section(&self) -> Event<ConfigSectionRegisteredEvent>;
    fn on_did_register_overlay(&self) -> Event<ConfigOverlayRegisteredEvent>;
    fn register_section(
        &self,
        domain: &str,
        schema: ConfigSchema,
        options: RegisterSectionOptions,
    ) -> Result<(), ConfigRegistryError>;
    fn get_section(&self, domain: &str) -> Option<ConfigSection>;
    fn list_sections(&self) -> Vec<ConfigSection>;
    fn register_effective_overlay(&self, overlay: Arc<dyn ConfigEffectiveOverlay>);
    fn list_effective_overlays(&self) -> Vec<Arc<dyn ConfigEffectiveOverlay>>;
    fn validate(&self, domain: &str, value: &Value) -> Result<Value, ConfigRegistryError>;
    fn merge(&self, domain: &str, base: Option<&Value>, patch: Option<&Value>) -> Option<Value>;
    fn default_value(&self, domain: &str) -> Option<Value>;
}

#[derive(Clone)]
pub struct ConfigRegistryHandle(pub Arc<dyn ConfigRegistryContract>);

impl Deref for ConfigRegistryHandle {
    type Target = dyn ConfigRegistryContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const CONFIG_REGISTRY_SERVICE_ID: ServiceIdentifier<ConfigRegistryHandle> =
    ServiceIdentifier::new("configRegistry");

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigChangeSource {
    Load,
    Reload,
    Set,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigChangedEvent {
    pub domain: String,
    pub source: ConfigChangeSource,
    pub value: Option<Value>,
    pub previous_value: Option<Value>,
}

pub type ConfigSectionChangedEvent = ConfigChangedEvent;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigDiagnostic {
    pub domain: Option<String>,
    pub severity: ConfigDiagnosticSeverity,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigInspectValue {
    pub value: Option<Value>,
    pub default_value: Option<Value>,
    pub user_value: Option<Value>,
    pub memory_value: Option<Value>,
}

pub type ResolvedConfig = Map<String, Value>;

#[async_trait]
pub trait ConfigServiceContract: Disposable + Send + Sync {
    async fn ready(&self) -> Result<(), ConfigServiceError>;
    fn on_did_change_configuration(&self) -> Event<ConfigChangedEvent>;
    fn on_did_section_change(&self) -> Event<ConfigSectionChangedEvent>;
    fn get(&self, domain: &str) -> Option<Value>;
    fn inspect(&self, domain: &str) -> ConfigInspectValue;
    fn get_all(&self) -> ResolvedConfig;
    async fn set(
        &self,
        domain: &str,
        patch: Option<Value>,
        target: ConfigTarget,
    ) -> Result<(), ConfigServiceError>;
    async fn replace(
        &self,
        domain: &str,
        value: Option<Value>,
        target: ConfigTarget,
    ) -> Result<(), ConfigServiceError>;
    async fn reload(&self) -> Result<(), ConfigServiceError>;
    fn diagnostics(&self) -> Vec<ConfigDiagnostic>;
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigServiceError {
    #[error(transparent)]
    Registry(#[from] ConfigRegistryError),

    #[error(transparent)]
    Storage(#[from] crate::persistence::interface::storage::StorageError),
}

#[derive(Clone)]
pub struct ConfigServiceHandle(pub Arc<dyn ConfigServiceContract>);

impl Deref for ConfigServiceHandle {
    type Target = dyn ConfigServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for ConfigServiceHandle {
    fn dispose(&self) -> crate::_base::di::lifecycle::DisposeResult {
        self.0.dispose()
    }
}

pub const CONFIG_SERVICE_ID: ServiceIdentifier<ConfigServiceHandle> =
    ServiceIdentifier::new("configService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_preserves_parse_success_and_failure() {
        let schema = ConfigSchema::new(|value| match value.as_bool() {
            Some(_) => Ok(value.clone()),
            None => Err(ConfigValidationError::new("expected boolean")),
        });
        assert_eq!(schema.parse(&Value::Bool(true)).unwrap(), Value::Bool(true));
        assert_eq!(
            schema.parse(&Value::String("yes".into())).unwrap_err(),
            ConfigValidationError::new("expected boolean")
        );
        assert!(schema == schema.clone());
    }

    #[test]
    fn enums_and_identifiers_preserve_external_names() {
        assert_eq!(serde_json::to_value(ConfigScope::Core).unwrap(), "core");
        assert_eq!(
            serde_json::to_value(ConfigTarget::Memory).unwrap(),
            "memory"
        );
        assert_eq!(CONFIG_REGISTRY_SERVICE_ID.to_string(), "configRegistry");
        assert_eq!(CONFIG_SERVICE_ID.to_string(), "configService");
    }
}
