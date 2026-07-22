use std::{error::Error, fmt};

use serde_json::{Map, Value};

use super::{
    managed_auth::{
        KIMI_CODE_PLATFORM_ID, KIMI_CODE_PROVIDER_NAME, ManagedKimiOAuthRef, OAuthStorageBackend,
        default_base_url, managed_oauth_ref, resolve_kimi_code_oauth_ref,
    },
    managed_models::{ManagedKimiCodeModelInfo, to_managed_model_alias},
    model_alias_merge::{MANAGED_KIMI_MODEL_FIELDS, merge_refreshed_model_alias},
};

#[derive(Debug, Clone)]
pub struct ManagedKimiCodeApplyOptions<'a> {
    pub models: &'a [ManagedKimiCodeModelInfo],
    pub base_url: Option<&'a str>,
    pub oauth_key: Option<&'a str>,
    pub oauth_host: Option<&'a str>,
    pub preserve_default_model: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedKimiCodeApplyResult {
    pub default_model: String,
    pub default_thinking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedKimiCodeCleanupResult {
    pub provider_name: &'static str,
    pub removed_provider: bool,
    pub removed_models: Vec<String>,
    pub default_model_cleared: bool,
    pub removed_services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedDefaultModel {
    model_key: String,
    thinking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedConfigError {
    message: String,
}

impl fmt::Display for ManagedConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ManagedConfigError {}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   applyManagedKimiCodeConfig()
pub fn apply_managed_kimi_code_config(
    config: &mut Map<String, Value>,
    options: ManagedKimiCodeApplyOptions<'_>,
) -> Result<ManagedKimiCodeApplyResult, ManagedConfigError> {
    if options.models.is_empty() {
        return Err(config_error("No models available for Kimi Code."));
    }
    for model in options.models {
        assert_positive_context_length(model)?;
    }

    let base_url = default_base_url(options.base_url);
    let oauth = options.oauth_key.map_or_else(
        || resolve_kimi_code_oauth_ref(options.oauth_host, Some(&base_url)),
        |key| managed_oauth_ref(key, options.oauth_host, None),
    );
    let selected_default =
        select_default_model(config, options.models, options.preserve_default_model)?;

    let providers = config
        .entry("providers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !providers.is_object() {
        return Err(config_error(
            "Kimi Code config providers must be an object.",
        ));
    }
    let provider = serde_json::json!({
        "type": "kimi",
        "baseUrl": base_url,
        "apiKey": "",
        "oauth": oauth_value(&oauth),
    });
    if let Some(providers) = providers.as_object_mut() {
        providers.insert(KIMI_CODE_PROVIDER_NAME.to_owned(), provider);
    }

    let mut existing_models = config
        .remove("models")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let upstream_keys = options
        .models
        .iter()
        .map(|model| managed_model_key(&model.id))
        .collect::<std::collections::HashSet<_>>();
    existing_models.retain(|key, value| {
        value
            .get("provider")
            .and_then(Value::as_str)
            .is_none_or(|provider| {
                provider != KIMI_CODE_PROVIDER_NAME || upstream_keys.contains(key)
            })
    });
    for model in options.models {
        let key = managed_model_key(&model.id);
        let remote = to_managed_model_alias(KIMI_CODE_PROVIDER_NAME, model);
        let merged = merge_refreshed_model_alias(
            existing_models.get(&key).unwrap_or(&Value::Null),
            &remote,
            &MANAGED_KIMI_MODEL_FIELDS,
        );
        existing_models.insert(key, Value::Object(merged));
    }
    config.insert("models".to_owned(), Value::Object(existing_models));
    config.insert(
        "defaultModel".to_owned(),
        Value::String(selected_default.model_key.clone()),
    );
    let mut thinking = config
        .remove("thinking")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    thinking.insert("enabled".to_owned(), Value::Bool(selected_default.thinking));
    config.insert("thinking".to_owned(), Value::Object(thinking));

    let oauth = oauth_value(&oauth);
    config.insert(
        "services".to_owned(),
        serde_json::json!({
            "moonshotSearch": {
                "baseUrl": format!("{base_url}/search"),
                "apiKey": "",
                "oauth": oauth,
            },
            "moonshotFetch": {
                "baseUrl": format!("{base_url}/fetch"),
                "apiKey": "",
                "oauth": oauth,
            },
        }),
    );

    Ok(ManagedKimiCodeApplyResult {
        default_model: selected_default.model_key,
        default_thinking: selected_default.thinking,
    })
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   applyManagedApiKeyProviderModels()
pub fn apply_managed_api_key_provider_models(
    config: &mut Map<String, Value>,
    provider_id: &str,
    models: &[ManagedKimiCodeModelInfo],
    alias_prefix: &str,
) -> Result<(), ManagedConfigError> {
    for model in models {
        assert_positive_context_length(model)?;
    }

    let mut existing_models = config
        .remove("models")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let upstream_keys = models
        .iter()
        .map(|model| format!("{alias_prefix}{}", model.id))
        .collect::<std::collections::HashSet<_>>();
    existing_models.retain(|key, value| {
        value
            .get("provider")
            .and_then(Value::as_str)
            .is_none_or(|provider| provider != provider_id || upstream_keys.contains(key))
    });
    for model in models {
        let key = format!("{alias_prefix}{}", model.id);
        let remote = to_managed_model_alias(provider_id, model);
        let merged = merge_refreshed_model_alias(
            existing_models.get(&key).unwrap_or(&Value::Null),
            &remote,
            &MANAGED_KIMI_MODEL_FIELDS,
        );
        existing_models.insert(key, Value::Object(merged));
    }
    config.insert("models".to_owned(), Value::Object(existing_models));
    Ok(())
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   applyManagedKimiCodeLogoutConfig()
pub fn apply_managed_kimi_code_logout_config(config: &mut Map<String, Value>) {
    if let Some(providers) = config.get_mut("providers").and_then(Value::as_object_mut) {
        providers.remove(KIMI_CODE_PROVIDER_NAME);
    }

    let current_default = config
        .get("defaultModel")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut removed_default_model = false;
    let mut existing_models = config
        .remove("models")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    existing_models.retain(|key, model| {
        let remove = model.get("provider").and_then(Value::as_str) == Some(KIMI_CODE_PROVIDER_NAME);
        if remove && current_default.as_deref() == Some(key) {
            removed_default_model = true;
        }
        !remove
    });
    config.insert("models".to_owned(), Value::Object(existing_models));
    if removed_default_model {
        config.remove("defaultModel");
    }
    if config.get("defaultProvider").and_then(Value::as_str) == Some(KIMI_CODE_PROVIDER_NAME) {
        config.remove("defaultProvider");
    }
    remove_managed_services(config);
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   clearManagedKimiCodeConfig()
pub fn clear_managed_kimi_code_config(
    config: &mut Map<String, Value>,
) -> ManagedKimiCodeCleanupResult {
    let removed_provider = config
        .get_mut("providers")
        .and_then(Value::as_object_mut)
        .is_some_and(|providers| providers.remove(KIMI_CODE_PROVIDER_NAME).is_some());

    let mut removed_models = Vec::new();
    if let Some(models) = config.get_mut("models").and_then(Value::as_object_mut) {
        models.retain(|key, model| {
            let remove =
                model.get("provider").and_then(Value::as_str) == Some(KIMI_CODE_PROVIDER_NAME);
            if remove {
                removed_models.push(key.clone());
            }
            !remove
        });
    }

    let default_model_cleared = config
        .get("defaultModel")
        .and_then(Value::as_str)
        .is_some_and(|default_model| removed_models.iter().any(|model| model == default_model));
    if default_model_cleared {
        config.remove("defaultModel");
    }
    let removed_services = remove_managed_services(config);

    ManagedKimiCodeCleanupResult {
        provider_name: KIMI_CODE_PROVIDER_NAME,
        removed_provider,
        removed_models,
        default_model_cleared,
        removed_services,
    }
}

fn assert_positive_context_length(
    model: &ManagedKimiCodeModelInfo,
) -> Result<(), ManagedConfigError> {
    if model.context_length == 0 {
        return Err(ManagedConfigError {
            message: format!(
                "Kimi Code model \"{}\" must include a positive context_length.",
                model.id
            ),
        });
    }
    Ok(())
}

fn config_error(message: impl Into<String>) -> ManagedConfigError {
    ManagedConfigError {
        message: message.into(),
    }
}

fn managed_model_key(model_id: &str) -> String {
    format!("{KIMI_CODE_PLATFORM_ID}/{model_id}")
}

fn oauth_value(reference: &ManagedKimiOAuthRef) -> Value {
    let mut value = Map::from_iter([
        (
            "storage".to_owned(),
            Value::String(
                match reference.storage {
                    OAuthStorageBackend::File => "file",
                    OAuthStorageBackend::Keyring => "keyring",
                }
                .to_owned(),
            ),
        ),
        ("key".to_owned(), Value::String(reference.key.clone())),
    ]);
    if let Some(oauth_host) = &reference.oauth_host {
        value.insert("oauthHost".to_owned(), Value::String(oauth_host.clone()));
    }
    Value::Object(value)
}

// Original: forcedThinking()
fn forced_thinking(model: Option<&ManagedKimiCodeModelInfo>, fallback: bool) -> bool {
    use super::managed_models::SupportsThinkingType;

    match model.and_then(|model| model.supports_thinking_type) {
        Some(SupportsThinkingType::Only) => true,
        Some(SupportsThinkingType::No) => false,
        _ => fallback,
    }
}

// Original: selectDefaultModel()
fn select_default_model(
    config: &Map<String, Value>,
    models: &[ManagedKimiCodeModelInfo],
    preserve_existing: bool,
) -> Result<SelectedDefaultModel, ManagedConfigError> {
    let first_model = models
        .first()
        .ok_or_else(|| config_error("No models available for Kimi Code."))?;
    let current_default = config
        .get("defaultModel")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let existing_models = config.get("models").and_then(Value::as_object);

    if let Some(current_default) = current_default.filter(|default_model| {
        preserve_existing && can_preserve_default_model(existing_models, default_model, models)
    }) {
        let preserved_model = models
            .iter()
            .find(|model| managed_model_key(&model.id) == current_default);
        let fallback = thinking_enabled(config)
            .or_else(|| preserved_model.map(|model| model.supports_reasoning))
            .unwrap_or(false);
        return Ok(SelectedDefaultModel {
            model_key: current_default.to_owned(),
            thinking: forced_thinking(preserved_model, fallback),
        });
    }

    let fallback = thinking_enabled(config).unwrap_or(first_model.supports_reasoning);
    Ok(SelectedDefaultModel {
        model_key: managed_model_key(&first_model.id),
        thinking: forced_thinking(Some(first_model), fallback),
    })
}

fn can_preserve_default_model(
    existing_models: Option<&Map<String, Value>>,
    default_model: &str,
    managed_models: &[ManagedKimiCodeModelInfo],
) -> bool {
    if managed_models
        .iter()
        .any(|model| managed_model_key(&model.id) == default_model)
    {
        return true;
    }
    existing_models
        .and_then(|models| models.get(default_model))
        .and_then(Value::as_object)
        .is_some_and(|model| {
            model.get("provider").and_then(Value::as_str) != Some(KIMI_CODE_PROVIDER_NAME)
        })
}

fn thinking_enabled(config: &Map<String, Value>) -> Option<bool> {
    config
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("enabled"))
        .and_then(Value::as_bool)
}

fn remove_managed_services(config: &mut Map<String, Value>) -> Vec<String> {
    let mut removed = Vec::new();
    let remove_services =
        if let Some(services) = config.get_mut("services").and_then(Value::as_object_mut) {
            if services.remove("moonshotSearch").is_some() {
                removed.push("moonshotSearch".to_owned());
            }
            if services.remove("moonshotFetch").is_some() {
                removed.push("moonshotFetch".to_owned());
            }
            services.is_empty()
        } else {
            false
        };
    if remove_services {
        config.remove("services");
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_models::{ManagedKimiCodeProtocol, SupportsThinkingType};

    fn model(id: &str) -> ManagedKimiCodeModelInfo {
        ManagedKimiCodeModelInfo {
            id: id.to_owned(),
            context_length: 200_000,
            supports_reasoning: false,
            supports_image_in: false,
            supports_video_in: false,
            supports_tool_use: true,
            supports_thinking_type: None,
            support_efforts: None,
            default_effort: None,
            display_name: None,
            protocol: None,
        }
    }

    #[test]
    fn applies_managed_provider_models_services_and_selected_default() {
        let mut config = serde_json::json!({
            "providers": {
                "managed:kimi-code": { "old": true },
                "other": { "type": "openai" }
            },
            "models": {
                "kimi-code/kimi-k2": {
                    "provider": "managed:kimi-code",
                    "model": "kimi-k2",
                    "maxContextSize": 1000,
                    "userField": "keep"
                },
                "kimi-code/stale": {
                    "provider": "managed:kimi-code",
                    "model": "stale"
                },
                "other/model": { "provider": "other", "model": "model" }
            },
            "thinking": { "enabled": false, "effort": "high" },
            "services": { "custom": { "baseUrl": "https://old" } },
            "customTopLevel": true
        });
        let mut refreshed = model("kimi-k2");
        refreshed.supports_reasoning = true;
        refreshed.display_name = Some("Kimi K2".to_owned());

        let result = apply_managed_kimi_code_config(
            config.as_object_mut().expect("config object"),
            ManagedKimiCodeApplyOptions {
                models: &[refreshed],
                base_url: Some("https://api.dev.example/coding/v1///"),
                oauth_key: Some("oauth/custom"),
                oauth_host: Some(" https://auth.dev.example/ "),
                preserve_default_model: false,
            },
        )
        .expect("apply config");

        assert_eq!(
            result,
            ManagedKimiCodeApplyResult {
                default_model: "kimi-code/kimi-k2".to_owned(),
                default_thinking: false,
            }
        );
        assert_eq!(
            config["providers"]["managed:kimi-code"],
            serde_json::json!({
                "type": "kimi",
                "baseUrl": "https://api.dev.example/coding/v1",
                "apiKey": "",
                "oauth": {
                    "storage": "file",
                    "key": "oauth/custom",
                    "oauthHost": "https://auth.dev.example"
                }
            })
        );
        assert_eq!(config["providers"]["other"]["type"], "openai");
        assert!(config["models"].get("kimi-code/stale").is_none());
        assert!(config["models"].get("other/model").is_some());
        assert_eq!(config["models"]["kimi-code/kimi-k2"]["userField"], "keep");
        assert_eq!(
            config["models"]["kimi-code/kimi-k2"]["maxContextSize"],
            200_000
        );
        assert_eq!(
            config["thinking"],
            serde_json::json!({ "enabled": false, "effort": "high" })
        );
        assert_eq!(
            config["services"],
            serde_json::json!({
                "moonshotSearch": {
                    "baseUrl": "https://api.dev.example/coding/v1/search",
                    "apiKey": "",
                    "oauth": {
                        "storage": "file",
                        "key": "oauth/custom",
                        "oauthHost": "https://auth.dev.example"
                    }
                },
                "moonshotFetch": {
                    "baseUrl": "https://api.dev.example/coding/v1/fetch",
                    "apiKey": "",
                    "oauth": {
                        "storage": "file",
                        "key": "oauth/custom",
                        "oauthHost": "https://auth.dev.example"
                    }
                }
            })
        );
        assert_eq!(config["customTopLevel"], true);
    }

    #[test]
    fn preserves_an_available_custom_default_when_requested() {
        let mut config = serde_json::json!({
            "providers": {},
            "models": {
                "custom-default": { "provider": "other", "model": "custom" }
            },
            "defaultModel": "custom-default",
            "thinking": { "enabled": false }
        });
        let mut incoming = model("kimi-k2");
        incoming.supports_reasoning = true;

        let result = apply_managed_kimi_code_config(
            config.as_object_mut().expect("config object"),
            ManagedKimiCodeApplyOptions {
                models: &[incoming],
                base_url: Some("https://api.example/coding/v1"),
                oauth_key: None,
                oauth_host: None,
                preserve_default_model: true,
            },
        )
        .expect("apply config");

        assert_eq!(result.default_model, "custom-default");
        assert!(!result.default_thinking);
        assert_eq!(config["defaultModel"], "custom-default");
        assert_eq!(config["thinking"]["enabled"], false);
    }

    #[test]
    fn server_thinking_type_overrides_preserved_thinking_state() {
        let mut config = serde_json::json!({
            "providers": {},
            "models": {
                "kimi-code/always": {
                    "provider": "managed:kimi-code",
                    "model": "always"
                }
            },
            "defaultModel": "kimi-code/always",
            "thinking": { "enabled": false }
        });
        let mut always = model("always");
        always.supports_thinking_type = Some(SupportsThinkingType::Only);

        let result = apply_managed_kimi_code_config(
            config.as_object_mut().expect("config object"),
            ManagedKimiCodeApplyOptions {
                models: &[always],
                base_url: Some("https://api.example/coding/v1"),
                oauth_key: None,
                oauth_host: None,
                preserve_default_model: true,
            },
        )
        .expect("apply config");
        assert!(result.default_thinking);
        assert_eq!(config["thinking"]["enabled"], true);

        let mut never = model("never");
        never.supports_reasoning = true;
        never.supports_thinking_type = Some(SupportsThinkingType::No);
        let result = apply_managed_kimi_code_config(
            config.as_object_mut().expect("config object"),
            ManagedKimiCodeApplyOptions {
                models: &[never],
                base_url: Some("https://api.example/coding/v1"),
                oauth_key: None,
                oauth_host: None,
                preserve_default_model: false,
            },
        )
        .expect("apply config");
        assert!(!result.default_thinking);
        assert_eq!(config["thinking"]["enabled"], false);
    }

    #[test]
    fn default_oauth_environment_omits_redundant_host() {
        let mut config = serde_json::json!({ "providers": {} });
        apply_managed_kimi_code_config(
            config.as_object_mut().expect("config object"),
            ManagedKimiCodeApplyOptions {
                models: &[model("kimi-k2")],
                base_url: Some("https://api.kimi.com/coding/v1"),
                oauth_key: None,
                oauth_host: None,
                preserve_default_model: false,
            },
        )
        .expect("apply config");
        assert_eq!(
            config["providers"]["managed:kimi-code"]["oauth"],
            serde_json::json!({ "storage": "file", "key": "oauth/kimi-code" })
        );
    }

    #[test]
    fn validation_errors_leave_config_unchanged() {
        let mut config = serde_json::json!({ "providers": {}, "custom": true });
        let original = config.clone();
        let error = apply_managed_kimi_code_config(
            config.as_object_mut().expect("config object"),
            ManagedKimiCodeApplyOptions {
                models: &[],
                base_url: None,
                oauth_key: None,
                oauth_host: None,
                preserve_default_model: false,
            },
        )
        .expect_err("empty models");
        assert_eq!(error.to_string(), "No models available for Kimi Code.");
        assert_eq!(config, original);

        let mut invalid = model("invalid");
        invalid.context_length = 0;
        let error = apply_managed_kimi_code_config(
            config.as_object_mut().expect("config object"),
            ManagedKimiCodeApplyOptions {
                models: &[invalid],
                base_url: None,
                oauth_key: None,
                oauth_host: None,
                preserve_default_model: false,
            },
        )
        .expect_err("invalid context");
        assert!(error.to_string().contains("positive context_length"));
        assert_eq!(config, original);
    }

    #[test]
    fn logout_removes_managed_entries_but_keeps_thinking_and_custom_state() {
        let mut config = serde_json::json!({
            "providers": {
                "managed:kimi-code": { "type": "kimi" },
                "custom": { "type": "kimi", "apiKey": "sk-existing" }
            },
            "models": {
                "kimi-code/kimi-k2": {
                    "provider": "managed:kimi-code",
                    "model": "kimi-k2"
                },
                "custom/default": { "provider": "custom", "model": "default" }
            },
            "defaultModel": "kimi-code/kimi-k2",
            "defaultProvider": "managed:kimi-code",
            "thinking": { "enabled": true, "effort": "high" },
            "services": {
                "moonshotSearch": { "baseUrl": "https://api.example/search" },
                "moonshotFetch": { "baseUrl": "https://api.example/fetch" },
                "customService": { "baseUrl": "https://service.example" }
            },
            "raw": { "default_model": "kimi-code/kimi-k2" }
        });

        apply_managed_kimi_code_logout_config(config.as_object_mut().expect("config object"));

        assert!(config["providers"].get("managed:kimi-code").is_none());
        assert!(config["providers"].get("custom").is_some());
        assert!(config["models"].get("kimi-code/kimi-k2").is_none());
        assert!(config["models"].get("custom/default").is_some());
        assert!(config.get("defaultModel").is_none());
        assert!(config.get("defaultProvider").is_none());
        assert_eq!(
            config["thinking"],
            serde_json::json!({ "enabled": true, "effort": "high" })
        );
        assert!(config["services"].get("moonshotSearch").is_none());
        assert!(config["services"].get("moonshotFetch").is_none());
        assert!(config["services"].get("customService").is_some());
        assert_eq!(config["raw"]["default_model"], "kimi-code/kimi-k2");
    }

    #[test]
    fn logout_only_clears_default_model_when_its_alias_was_removed() {
        let mut config = serde_json::json!({
            "providers": { "managed:kimi-code": {} },
            "models": {
                "custom/default": { "provider": "custom", "model": "default" }
            },
            "defaultModel": "custom/default",
            "services": {
                "moonshotSearch": {},
                "moonshotFetch": {}
            }
        });

        apply_managed_kimi_code_logout_config(config.as_object_mut().expect("config object"));

        assert_eq!(config["defaultModel"], "custom/default");
        assert!(config.get("services").is_none());
    }

    #[test]
    fn clear_reports_each_removed_managed_entry() {
        let mut config = serde_json::json!({
            "providers": {
                "managed:kimi-code": { "type": "kimi" },
                "custom": { "apiKey": "sk-existing" }
            },
            "models": {
                "kimi-code/kimi-k2": {
                    "provider": "managed:kimi-code",
                    "model": "kimi-k2"
                },
                "custom/default": { "provider": "custom", "model": "default" }
            },
            "defaultModel": "kimi-code/kimi-k2",
            "defaultProvider": "managed:kimi-code",
            "services": {
                "moonshotSearch": {},
                "moonshotFetch": {},
                "otherService": { "baseUrl": "https://service.example" }
            }
        });

        let result = clear_managed_kimi_code_config(config.as_object_mut().expect("config object"));

        assert_eq!(
            result,
            ManagedKimiCodeCleanupResult {
                provider_name: "managed:kimi-code",
                removed_provider: true,
                removed_models: vec!["kimi-code/kimi-k2".to_owned()],
                default_model_cleared: true,
                removed_services: vec!["moonshotSearch".to_owned(), "moonshotFetch".to_owned()],
            }
        );
        assert!(config.get("defaultModel").is_none());
        assert_eq!(config["defaultProvider"], "managed:kimi-code");
        assert!(config["providers"].get("custom").is_some());
        assert!(config["models"].get("custom/default").is_some());
        assert!(config["services"].get("otherService").is_some());
    }

    #[test]
    fn clear_reports_no_changes_without_materializing_optional_sections() {
        let mut config = serde_json::json!({ "providers": {}, "custom": true });
        let result = clear_managed_kimi_code_config(config.as_object_mut().expect("config object"));

        assert_eq!(
            result,
            ManagedKimiCodeCleanupResult {
                provider_name: "managed:kimi-code",
                removed_provider: false,
                removed_models: Vec::new(),
                default_model_cleared: false,
                removed_services: Vec::new(),
            }
        );
        assert!(config.get("models").is_none());
        assert!(config.get("services").is_none());
    }

    #[test]
    fn refreshes_only_provider_models_and_preserves_user_owned_state() {
        let mut config = serde_json::json!({
            "providers": {
                "my-kimi": {
                    "type": "kimi",
                    "baseUrl": "https://api.example/coding/v1",
                    "apiKey": "sk-distributed",
                    "custom": true
                }
            },
            "models": {
                "my-kimi/kimi-k2": {
                    "provider": "my-kimi",
                    "model": "kimi-k2",
                    "maxContextSize": 1000,
                    "supportEfforts": ["old"],
                    "maxOutputSize": 4096,
                    "overrides": { "supportEfforts": ["low"] }
                },
                "my-kimi/stale": { "provider": "my-kimi", "model": "stale" },
                "other/model": { "provider": "other", "model": "model" }
            },
            "defaultModel": "my-kimi/kimi-k2",
            "thinking": { "enabled": false },
            "services": { "custom": { "baseUrl": "https://service" } }
        });
        let before_provider = config["providers"].clone();
        let before_default = config["defaultModel"].clone();
        let before_thinking = config["thinking"].clone();
        let before_services = config["services"].clone();
        let mut refreshed = model("kimi-k2");
        refreshed.display_name = Some("Fresh K2".to_owned());
        refreshed.support_efforts = Some(vec!["low".to_owned(), "high".to_owned()]);
        let mut added = model("kimi-k2.5");
        added.protocol = Some(ManagedKimiCodeProtocol::Anthropic);
        added.supports_thinking_type = Some(SupportsThinkingType::Only);

        apply_managed_api_key_provider_models(
            config.as_object_mut().expect("config object"),
            "my-kimi",
            &[refreshed, added],
            "my-kimi/",
        )
        .expect("apply models");

        assert_eq!(config["providers"], before_provider);
        assert_eq!(config["defaultModel"], before_default);
        assert_eq!(config["thinking"], before_thinking);
        assert_eq!(config["services"], before_services);
        assert!(config["models"].get("my-kimi/stale").is_none());
        assert!(config["models"].get("other/model").is_some());
        let alias = &config["models"]["my-kimi/kimi-k2"];
        assert_eq!(alias["displayName"], "Fresh K2");
        assert_eq!(alias["maxOutputSize"], 4_096);
        assert_eq!(alias["supportEfforts"], serde_json::json!(["low", "high"]));
        assert_eq!(
            alias["overrides"],
            serde_json::json!({ "supportEfforts": ["low"] })
        );
        let added = &config["models"]["my-kimi/kimi-k2.5"];
        assert_eq!(added["betaApi"], true);
        assert_eq!(added["adaptiveThinking"], true);
    }

    #[test]
    fn alias_prefix_controls_generated_scope() {
        let mut config = serde_json::json!({ "providers": {}, "models": {} });
        apply_managed_api_key_provider_models(
            config.as_object_mut().expect("config object"),
            "managed:kimi-code",
            &[model("kimi-for-coding")],
            "kimi-code/",
        )
        .expect("apply models");
        assert!(config["models"].get("kimi-code/kimi-for-coding").is_some());
        assert_eq!(
            config["models"]["kimi-code/kimi-for-coding"]["provider"],
            "managed:kimi-code"
        );
    }

    #[test]
    fn rejects_zero_context_before_mutating_config() {
        let mut config = serde_json::json!({ "models": { "keep": { "provider": "x" } } });
        let original = config.clone();
        let mut invalid = model("bad");
        invalid.context_length = 0;
        let error = apply_managed_api_key_provider_models(
            config.as_object_mut().expect("config object"),
            "my-kimi",
            &[invalid],
            "my-kimi/",
        )
        .expect_err("invalid context");
        assert_eq!(
            error.to_string(),
            "Kimi Code model \"bad\" must include a positive context_length."
        );
        assert_eq!(config, original);
    }
}
