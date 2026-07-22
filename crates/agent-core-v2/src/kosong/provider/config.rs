use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::error::Error;
use std::fmt;

// Original:
//   packages/agent-core-v2/src/kosong/provider/provider.ts
//   ProviderTypeSchema / ProviderType
//
// Rust adaptation:
//   The transparent newtype remains deliberately open to arbitrary strings;
//   it only prevents vendor identities from being confused with other string
//   fields inside Rust APIs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderType(String);

impl ProviderType {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ProviderType {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ProviderType {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for ProviderType {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ProviderType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OAuthStorage {
    File,
    Keyring,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthRef {
    pub storage: OAuthStorage,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthRefValidationError {
    field: &'static str,
}

impl OAuthRefValidationError {
    pub fn field(&self) -> &'static str {
        self.field
    }
}

impl fmt::Display for OAuthRefValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OAuth reference field '{}' must not be empty",
            self.field
        )
    }
}

impl Error for OAuthRefValidationError {}

impl OAuthRef {
    pub fn new(
        storage: OAuthStorage,
        key: impl Into<String>,
        oauth_host: Option<String>,
    ) -> Result<Self, OAuthRefValidationError> {
        let key = key.into();
        if key.is_empty() {
            return Err(OAuthRefValidationError { field: "key" });
        }
        if oauth_host.as_ref().is_some_and(String::is_empty) {
            return Err(OAuthRefValidationError { field: "oauthHost" });
        }
        Ok(Self {
            storage,
            key,
            oauth_host,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOAuthRef {
    storage: OAuthStorage,
    key: String,
    oauth_host: Option<String>,
}

impl<'de> Deserialize<'de> for OAuthRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawOAuthRef::deserialize(deserializer)?;
        Self::new(raw.storage, raw.key, raw.oauth_host).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelSource {
    #[serde(rename = "static")]
    Static,
    #[serde(rename = "discover")]
    Discover,
    #[serde(rename = "oauth-catalog")]
    OAuthCatalog,
}

// Original: provider.ts, ProviderConfigSchema / ProviderConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_source: Option<ModelSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_headers: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<ProviderType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Map<String, Value>>,
}

pub const PROVIDERS_SECTION: &str = "providers";
pub const DEFAULT_PROVIDER_SECTION: &str = "defaultProvider";
pub const ENV_MODEL_PROVIDER_KEY: &str = "__kimi_env__";
pub const PROVIDER_SERVICE_ID: &str = "providerService";

pub type ProvidersSection = IndexMap<String, ProviderConfig>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProvidersChangedEvent {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

// MIGRATION-TODO:
// Original: provider.ts, IProviderService and its providerService DI binding.
// Missing dependency: the _base Event primitive and the application-scoped
// Rust DI/service container have not been migrated.
// Temporary behavior: none; no fake event stream or in-memory service is
// exposed by this contract module.
// Completion condition: migrate Event, then define the async provider service
// trait and bind it under PROVIDER_SERVICE_ID at application scope.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_type_remains_free_form_text() {
        for value in ["kimi", "vendor-registered-elsewhere", ""] {
            let provider_type: ProviderType = serde_json::from_value(json!(value)).unwrap();
            assert_eq!(provider_type.as_str(), value);
            assert_eq!(serde_json::to_value(provider_type).unwrap(), json!(value));
        }
        assert!(serde_json::from_value::<ProviderType>(json!(42)).is_err());
    }

    #[test]
    fn oauth_reference_accepts_both_stores_and_rejects_empty_required_strings() {
        assert_eq!(
            serde_json::from_value::<OAuthRef>(json!({
                "storage": "file",
                "key": "kimi"
            }))
            .unwrap(),
            OAuthRef::new(OAuthStorage::File, "kimi", None).unwrap()
        );
        assert!(
            serde_json::from_value::<OAuthRef>(json!({
                "storage": "keyring",
                "key": "kimi",
                "oauthHost": "https://auth.example.test"
            }))
            .is_ok()
        );
        for invalid in [
            json!({"storage": "file", "key": ""}),
            json!({"storage": "file", "key": "kimi", "oauthHost": ""}),
            json!({"storage": "memory", "key": "kimi"}),
        ] {
            assert!(serde_json::from_value::<OAuthRef>(invalid).is_err());
        }
    }

    #[test]
    fn provider_config_preserves_schema_field_names_and_open_source_object() {
        let value = json!({
            "modelSource": "oauth-catalog",
            "baseUrl": "https://api.example.test/v1",
            "customHeaders": {"x-client": "kimi-code"},
            "defaultModel": "model-1",
            "type": "external-vendor",
            "apiKey": "secret",
            "oauth": {"storage": "keyring", "key": "vendor"},
            "env": {"VENDOR_API_KEY": "secret"},
            "source": {"package": "external", "priority": 2}
        });
        let config: ProviderConfig = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(config.model_source, Some(ModelSource::OAuthCatalog));
        assert_eq!(
            config.provider_type.as_ref().map(ProviderType::as_str),
            Some("external-vendor")
        );
        assert_eq!(serde_json::to_value(config).unwrap(), value);
    }

    #[test]
    fn constants_keep_external_configuration_and_service_names() {
        assert_eq!(PROVIDERS_SECTION, "providers");
        assert_eq!(DEFAULT_PROVIDER_SECTION, "defaultProvider");
        assert_eq!(ENV_MODEL_PROVIDER_KEY, "__kimi_env__");
        assert_eq!(PROVIDER_SERVICE_ID, "providerService");
    }
}
