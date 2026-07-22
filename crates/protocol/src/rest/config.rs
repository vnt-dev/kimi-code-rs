use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::display::OptionalJsonValue;
use crate::validation::optional_non_null;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfigResponse {
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub base_url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_model: Option<String>,
    pub has_api_key: bool,
}

// Original: rest/config.ts, configResponseSchema.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigResponse {
    #[serde(default)]
    pub providers: IndexMap<String, ProviderConfigResponse>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_provider: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub models: Option<IndexMap<String, Value>>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub thinking: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub plan_mode: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub yolo: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_permission_mode: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_plan_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub permission: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub hooks: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub services: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub merge_all_available_skills: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub extra_skill_dirs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub loop_control: OptionalJsonValue,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub background: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub experimental: Option<IndexMap<String, bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub telemetry: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub raw: Option<IndexMap<String, Value>>,
}

// Original: rest/config.ts, patchConfigRequestSchema.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PatchConfigRequest {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub providers: Option<IndexMap<String, Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_provider: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub models: Option<IndexMap<String, Value>>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub thinking: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub plan_mode: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub yolo: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_permission_mode: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub default_plan_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub permission: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub hooks: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub services: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub merge_all_available_skills: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub extra_skill_dirs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub loop_control: OptionalJsonValue,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub background: OptionalJsonValue,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub experimental: Option<IndexMap<String, bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub telemetry: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_defaults_and_preserves_optional_unknown_nulls() {
        let config: ConfigResponse = serde_json::from_value(serde_json::json!({
            "thinking": null,
            "plan_mode": false
        }))
        .unwrap();
        assert!(config.providers.is_empty());
        assert_eq!(config.thinking.as_value(), Some(&Value::Null));
        assert_eq!(
            serde_json::to_value(config).unwrap(),
            serde_json::json!({"providers": {}, "thinking": null, "plan_mode": false})
        );

        assert!(
            serde_json::from_value::<PatchConfigRequest>(serde_json::json!({
                "default_model": null
            }))
            .is_err()
        );
    }
}
