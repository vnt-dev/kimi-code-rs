//! `KIMI_MODEL_*` effective-configuration overlay.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/envOverlay.ts`.

use std::sync::{Arc, LazyLock};

use serde_json::{Map, Number, Value};

use crate::{
    _base::{errors::errors::Error2, utils::env::parse_boolean_env},
    app::config::{
        ConfigEffectiveOverlay, ConfigValidationError, GetEnv, ValidateConfig,
        overlay_contributions::register_config_overlay,
    },
    kosong::provider::{
        ENV_MODEL_PROVIDER_KEY, provider_definition::resolve_provider_endpoint_with_env,
    },
};

pub const ENV_MODEL_ALIAS_KEY: &str = "__kimi_env_model__";
const DEFAULT_MAX_CONTEXT_SIZE: u64 = 262_144;
const DEFAULT_CAPABILITIES: [&str; 2] = ["image_in", "thinking"];

pub struct KimiModelEnvOverlay;

pub static KIMI_MODEL_ENV_OVERLAY: KimiModelEnvOverlay = KimiModelEnvOverlay;

static REGISTER_OVERLAY: LazyLock<()> = LazyLock::new(|| {
    register_config_overlay(Arc::new(KimiModelEnvOverlay));
});

// Original: module-load registerConfigOverlay(kimiModelEnvOverlay).
// Rust registration is explicit at the composition boundary; LazyLock keeps
// repeated composition calls equivalent to one TypeScript module evaluation.
pub fn register_kimi_model_env_overlay() {
    LazyLock::force(&REGISTER_OVERLAY);
}

impl ConfigEffectiveOverlay for KimiModelEnvOverlay {
    // Original: kimiModelEnvOverlay.apply().
    fn apply(
        &self,
        effective: &mut Map<String, Value>,
        get_env: &GetEnv<'_>,
        validate: &ValidateConfig<'_>,
    ) -> Result<Vec<String>, ConfigValidationError> {
        let model = trimmed(get_env("KIMI_MODEL_NAME").as_deref());
        let temperature = parse_float_env(
            get_env("KIMI_MODEL_TEMPERATURE").as_deref(),
            "KIMI_MODEL_TEMPERATURE",
        )?;
        let top_p = parse_float_env(get_env("KIMI_MODEL_TOP_P").as_deref(), "KIMI_MODEL_TOP_P")?;
        let thinking_keep = trimmed(get_env("KIMI_MODEL_THINKING_KEEP").as_deref());
        let max_completion_tokens =
            parse_completion_tokens(get_env("KIMI_MODEL_MAX_COMPLETION_TOKENS").as_deref())?.or(
                parse_completion_tokens(get_env("KIMI_MODEL_MAX_TOKENS").as_deref())?,
            );
        let mut changed = Vec::new();

        let overrides =
            collect_model_overrides(temperature, top_p, thinking_keep, max_completion_tokens);
        let Some(model) = model else {
            if let Some(overrides) = overrides {
                effective.insert("modelOverrides".into(), Value::Object(overrides));
                changed.push("modelOverrides".into());
            }
            return Ok(changed);
        };

        let max_context_size = match trimmed(get_env("KIMI_MODEL_MAX_CONTEXT_SIZE").as_deref()) {
            Some(raw) => parse_positive_int(&raw, "KIMI_MODEL_MAX_CONTEXT_SIZE")?,
            None => DEFAULT_MAX_CONTEXT_SIZE,
        };
        let max_output_size = match trimmed(get_env("KIMI_MODEL_MAX_OUTPUT_SIZE").as_deref()) {
            Some(raw) => Some(parse_positive_int(&raw, "KIMI_MODEL_MAX_OUTPUT_SIZE")?),
            None => None,
        };
        let capabilities = parse_capabilities(get_env("KIMI_MODEL_CAPABILITIES").as_deref())
            .unwrap_or_else(|| {
                DEFAULT_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            });
        let display_name = trimmed(get_env("KIMI_MODEL_DISPLAY_NAME").as_deref());
        let reasoning_key = trimmed(get_env("KIMI_MODEL_REASONING_KEY").as_deref());
        let reasoning_history = trimmed(get_env("KIMI_MODEL_REASONING_HISTORY").as_deref());
        let adaptive_thinking = parse_boolean_var(
            get_env("KIMI_MODEL_ADAPTIVE_THINKING").as_deref(),
            "KIMI_MODEL_ADAPTIVE_THINKING",
        )?;

        let mut alias = Map::from_iter([
            (
                "provider".into(),
                Value::String(ENV_MODEL_PROVIDER_KEY.into()),
            ),
            ("model".into(), Value::String(model)),
            (
                "maxContextSize".into(),
                Value::Number(max_context_size.into()),
            ),
            (
                "capabilities".into(),
                Value::Array(capabilities.into_iter().map(Value::String).collect()),
            ),
        ]);
        insert_string(&mut alias, "displayName", display_name);
        if let Some(max_output_size) = max_output_size {
            alias.insert(
                "maxOutputSize".into(),
                Value::Number(max_output_size.into()),
            );
        }
        insert_string(&mut alias, "reasoningKey", reasoning_key);
        insert_string(&mut alias, "reasoningHistory", reasoning_history);
        if let Some(adaptive_thinking) = adaptive_thinking {
            alias.insert("adaptiveThinking".into(), Value::Bool(adaptive_thinking));
        }
        let mut models = as_record(effective.get("models"));
        models.insert(ENV_MODEL_ALIAS_KEY.into(), Value::Object(alias));
        effective.insert("models".into(), validate("models", &Value::Object(models))?);
        changed.push("models".into());

        let providers = as_record(effective.get("providers"));
        let env_provider = providers
            .get(ENV_MODEL_PROVIDER_KEY)
            .map(|value| as_record(Some(value)))
            .unwrap_or_default();
        let provider_type = env_provider
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("kimi");
        let provider_base_url = env_provider
            .get("baseUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                resolve_provider_endpoint_with_env(provider_type, &|name| get_env(name))
                    .ok()
                    .and_then(|endpoint| endpoint.base_url)
            });
        let mut provider_patch = Map::new();
        if !env_provider.contains_key("type") {
            provider_patch.insert("type".into(), Value::String("kimi".into()));
        }
        if let Some(base_url) = provider_base_url
            && !env_provider.contains_key("baseUrl")
        {
            provider_patch.insert("baseUrl".into(), Value::String(base_url));
        }
        if !provider_patch.is_empty() {
            let mut merged_provider = env_provider;
            merged_provider.extend(provider_patch);
            let mut next_providers = providers;
            next_providers.insert(
                ENV_MODEL_PROVIDER_KEY.into(),
                Value::Object(merged_provider),
            );
            effective.insert(
                "providers".into(),
                validate("providers", &Value::Object(next_providers))?,
            );
            changed.push("providers".into());
        }

        effective.insert(
            "defaultModel".into(),
            Value::String(ENV_MODEL_ALIAS_KEY.into()),
        );
        changed.push("defaultModel".into());
        if let Some(overrides) = overrides {
            effective.insert("modelOverrides".into(), Value::Object(overrides));
            changed.push("modelOverrides".into());
        }
        Ok(changed)
    }

    // Original: kimiModelEnvOverlay.strip().
    fn strip(&self, domain: &str, value: &Value, raw_snake: &Map<String, Value>) -> Option<Value> {
        match domain {
            "models" => Some(without_key(value, ENV_MODEL_ALIAS_KEY)),
            "defaultModel" if value == ENV_MODEL_ALIAS_KEY => raw_snake
                .get("default_model")
                .and_then(Value::as_str)
                .map(|value| Value::String(value.into())),
            "defaultModel" => Some(value.clone()),
            "modelOverrides" => None,
            _ => Some(value.clone()),
        }
    }
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn config_error(message: impl Into<String>) -> ConfigValidationError {
    let error = Error2::new(crate::app::config::CONFIG_INVALID, message.into());
    ConfigValidationError::new(error.to_string())
}

fn parse_positive_int(raw: &str, variable: &str) -> Result<u64, ConfigValidationError> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(config_error(format!(
            "{variable} must be a positive integer, got \"{raw}\"."
        )));
    }
    match raw.parse::<u64>() {
        Ok(value) if value > 0 => Ok(value),
        _ => Err(config_error(format!(
            "{variable} must be a positive integer, got \"{raw}\"."
        ))),
    }
}

fn parse_float_env(
    raw: Option<&str>,
    variable: &str,
) -> Result<Option<f64>, ConfigValidationError> {
    let Some(value) = trimmed(raw) else {
        return Ok(None);
    };
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            config_error(format!(
                "{variable} must be a number, got \"{}\".",
                raw.unwrap_or_default()
            ))
        })
        .map(Some)
}

fn parse_completion_tokens(raw: Option<&str>) -> Result<Option<u64>, ConfigValidationError> {
    let Some(value) = trimmed(raw) else {
        return Ok(None);
    };
    Ok(value.parse::<u64>().ok())
}

fn parse_capabilities(raw: Option<&str>) -> Option<Vec<String>> {
    raw.map(|raw| {
        raw.split(',')
            .map(|capability| capability.trim().to_ascii_lowercase())
            .filter(|capability| !capability.is_empty())
            .collect::<Vec<_>>()
    })
    .filter(|capabilities| !capabilities.is_empty())
}

fn parse_boolean_var(
    raw: Option<&str>,
    variable: &str,
) -> Result<Option<bool>, ConfigValidationError> {
    let Some(value) = trimmed(raw) else {
        return Ok(None);
    };
    parse_boolean_env(Some(&value)).map(Some).ok_or_else(|| {
        config_error(format!(
            "{variable} must be a boolean (true/false/1/0/yes/no/on/off), got \"{}\".",
            raw.unwrap_or_default()
        ))
    })
}

fn as_record(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn without_key(value: &Value, key: &str) -> Value {
    let mut value = value.clone();
    if let Some(record) = value.as_object_mut() {
        record.remove(key);
    }
    value
}

fn collect_model_overrides(
    temperature: Option<f64>,
    top_p: Option<f64>,
    thinking_keep: Option<String>,
    max_completion_tokens: Option<u64>,
) -> Option<Map<String, Value>> {
    let mut overrides = Map::new();
    if let Some(temperature) = temperature {
        insert_number(&mut overrides, "temperature", temperature);
    }
    if let Some(top_p) = top_p {
        insert_number(&mut overrides, "topP", top_p);
    }
    insert_string(&mut overrides, "thinkingKeep", thinking_keep);
    if let Some(max_completion_tokens) = max_completion_tokens {
        overrides.insert(
            "maxCompletionTokens".into(),
            Value::Number(max_completion_tokens.into()),
        );
    }
    (!overrides.is_empty()).then_some(overrides)
}

fn insert_string(record: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        record.insert(key.into(), Value::String(value));
    }
}

fn insert_number(record: &mut Map<String, Value>, key: &str, value: f64) {
    if let Some(value) = Number::from_f64(value) {
        record.insert(key.into(), Value::Number(value));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn apply(
        effective: &mut Map<String, Value>,
        environment: HashMap<&str, &str>,
    ) -> Result<Vec<String>, ConfigValidationError> {
        KIMI_MODEL_ENV_OVERLAY.apply(
            effective,
            &|name| environment.get(name).map(|value| (*value).to_owned()),
            &|_, value| Ok(value.clone()),
        )
    }

    #[test]
    fn model_name_synthesizes_effective_entries_and_strip_never_persists_them() {
        let mut effective = Map::new();
        let changed = apply(
            &mut effective,
            HashMap::from([
                ("KIMI_MODEL_NAME", "  kimi-k2  "),
                ("KIMI_MODEL_MAX_CONTEXT_SIZE", "128"),
                ("KIMI_MODEL_CAPABILITIES", " image_in, TOOL_USE "),
                ("KIMI_MODEL_TEMPERATURE", "0.25"),
            ]),
        )
        .unwrap();
        assert_eq!(
            changed,
            ["models", "providers", "defaultModel", "modelOverrides"]
        );
        assert_eq!(effective["models"][ENV_MODEL_ALIAS_KEY]["model"], "kimi-k2");
        assert_eq!(
            effective["models"][ENV_MODEL_ALIAS_KEY]["maxContextSize"],
            128
        );
        assert_eq!(effective["defaultModel"], ENV_MODEL_ALIAS_KEY);
        assert_eq!(effective["modelOverrides"]["temperature"], 0.25);
        assert_eq!(
            KIMI_MODEL_ENV_OVERLAY.strip("models", &effective["models"], &Map::new()),
            Some(serde_json::json!({}))
        );
        assert_eq!(
            KIMI_MODEL_ENV_OVERLAY.strip(
                "modelOverrides",
                &effective["modelOverrides"],
                &Map::new()
            ),
            None
        );
        assert_eq!(
            KIMI_MODEL_ENV_OVERLAY.strip(
                "defaultModel",
                &effective["defaultModel"],
                &Map::from_iter([("default_model".into(), Value::String("saved".into()))])
            ),
            Some(Value::String("saved".into()))
        );
    }

    #[test]
    fn overrides_apply_without_model_and_invalid_values_fail_with_source_messages() {
        let mut effective = Map::new();
        assert_eq!(
            apply(
                &mut effective,
                HashMap::from([
                    ("KIMI_MODEL_TOP_P", "0.9"),
                    ("KIMI_MODEL_MAX_TOKENS", "8192")
                ])
            )
            .unwrap(),
            ["modelOverrides"]
        );
        assert_eq!(
            effective["modelOverrides"],
            serde_json::json!({"topP": 0.9, "maxCompletionTokens": 8192})
        );
        let error = apply(
            &mut Map::new(),
            HashMap::from([
                ("KIMI_MODEL_NAME", "x"),
                ("KIMI_MODEL_MAX_CONTEXT_SIZE", "0"),
            ]),
        )
        .unwrap_err();
        assert_eq!(
            error.message,
            "KIMI_MODEL_MAX_CONTEXT_SIZE must be a positive integer, got \"0\"."
        );
        let error = apply(
            &mut Map::new(),
            HashMap::from([
                ("KIMI_MODEL_NAME", "x"),
                ("KIMI_MODEL_ADAPTIVE_THINKING", "maybe"),
            ]),
        )
        .unwrap_err();
        assert_eq!(
            error.message,
            "KIMI_MODEL_ADAPTIVE_THINKING must be a boolean (true/false/1/0/yes/no/on/off), got \"maybe\"."
        );
    }
}
