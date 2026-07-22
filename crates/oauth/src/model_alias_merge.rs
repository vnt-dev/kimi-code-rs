use std::collections::HashSet;

use serde_json::{Map, Value};

pub const MANAGED_KIMI_MODEL_FIELDS: [&str; 10] = [
    "provider",
    "model",
    "maxContextSize",
    "capabilities",
    "displayName",
    "protocol",
    "betaApi",
    "adaptiveThinking",
    "supportEfforts",
    "defaultEffort",
];

pub const CUSTOM_REGISTRY_MODEL_FIELDS: [&str; 7] = [
    "provider",
    "model",
    "maxContextSize",
    "capabilities",
    "displayName",
    "supportEfforts",
    "defaultEffort",
];

// Original:
//   packages/oauth/src/model-alias-merge.ts
//   mergeRefreshedModelAlias()
pub fn merge_refreshed_model_alias(
    existing: &Value,
    remote: &Map<String, Value>,
    remote_owned_fields: &[&str],
) -> Map<String, Value> {
    let empty = Map::new();
    let current = existing.as_object().unwrap_or(&empty);
    let owned = remote_owned_fields.iter().copied().collect::<HashSet<_>>();
    let overrides = current.get("overrides").and_then(Value::as_object).cloned();

    let mut merged = current
        .iter()
        .filter(|(key, _)| key.as_str() != "overrides" && !owned.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    merged.extend(
        remote
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    if let Some(overrides) = overrides {
        merged.insert("overrides".to_owned(), Value::Object(overrides));
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_overrides_and_user_extras_while_replacing_remote_fields() {
        let existing = serde_json::json!({
            "provider": "managed:kimi-code",
            "model": "kimi-k2",
            "maxContextSize": 262144,
            "supportEfforts": ["low"],
            "userNote": { "keep": true },
            "overrides": { "supportEfforts": ["low"] }
        });
        let remote = serde_json::json!({
            "provider": "managed:kimi-code",
            "model": "kimi-k2",
            "maxContextSize": 262144,
            "supportEfforts": ["low", "high", "max"]
        });
        let merged = merge_refreshed_model_alias(
            &existing,
            remote.as_object().expect("remote object"),
            &MANAGED_KIMI_MODEL_FIELDS,
        );
        assert_eq!(
            merged["supportEfforts"],
            serde_json::json!(["low", "high", "max"])
        );
        assert_eq!(
            merged["overrides"],
            serde_json::json!({ "supportEfforts": ["low"] })
        );
        assert_eq!(merged["userNote"], serde_json::json!({ "keep": true }));
    }

    #[test]
    fn drops_remote_owned_fields_that_upstream_no_longer_declares() {
        for fields in [
            MANAGED_KIMI_MODEL_FIELDS.as_slice(),
            CUSTOM_REGISTRY_MODEL_FIELDS.as_slice(),
        ] {
            let existing = serde_json::json!({
                "provider": "registry",
                "model": "gpt-5.5",
                "supportEfforts": ["low", "high"],
                "defaultEffort": "high"
            });
            let remote = serde_json::json!({
                "provider": "registry",
                "model": "gpt-5.5",
                "maxContextSize": 131072
            });
            let merged = merge_refreshed_model_alias(
                &existing,
                remote.as_object().expect("remote object"),
                fields,
            );
            assert!(!merged.contains_key("supportEfforts"));
            assert!(!merged.contains_key("defaultEffort"));
        }
    }

    #[test]
    fn ignores_non_object_existing_values_and_non_object_overrides() {
        let remote = serde_json::json!({ "provider": "p", "model": "m" });
        assert_eq!(
            merge_refreshed_model_alias(
                &Value::Null,
                remote.as_object().expect("remote object"),
                &MANAGED_KIMI_MODEL_FIELDS,
            ),
            remote.as_object().expect("remote object").clone()
        );

        let existing = serde_json::json!({ "overrides": "invalid", "custom": 1 });
        let merged = merge_refreshed_model_alias(
            &existing,
            remote.as_object().expect("remote object"),
            &MANAGED_KIMI_MODEL_FIELDS,
        );
        assert_eq!(merged.get("custom"), Some(&serde_json::json!(1)));
        assert!(!merged.contains_key("overrides"));
    }
}
