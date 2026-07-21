use std::{error::Error, fmt};

use serde_json::{Map, Value};

use super::{
    managed_models::{ManagedKimiCodeModelInfo, to_managed_model_alias},
    model_alias_merge::{MANAGED_KIMI_MODEL_FIELDS, merge_refreshed_model_alias},
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::managed_models::{ManagedKimiCodeProtocol, SupportsThinkingType};

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
