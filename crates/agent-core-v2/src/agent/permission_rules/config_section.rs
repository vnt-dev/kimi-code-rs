use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::app::config::{
    ConfigFromToml, ConfigSchema, ConfigToToml, ConfigValidationError, RegisterSectionOptions,
    clone_record, plain_object_to_toml, register_config_section, transform_plain_object,
};

use super::{
    matches_rule::parse_permission_pattern,
    types::{PermissionRule, PermissionRuleDecision, PermissionRuleScope},
};

pub const PERMISSION_SECTION: &str = "permission";

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct PermissionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<PermissionRule>>,
}

// Original:
//   packages/agent-core-v2/src/agent/permissionRules/configSection.ts
//   PermissionRuleSchema / PermissionConfigSchema
pub static PERMISSION_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        let object = value
            .as_object()
            .ok_or_else(|| ConfigValidationError::new("permission must be an object"))?;
        let rules = match object.get("rules") {
            None => None,
            Some(value) => {
                let entries = value.as_array().ok_or_else(|| {
                    ConfigValidationError::new("permission.rules must be an array")
                })?;
                Some(
                    entries
                        .iter()
                        .map(parse_permission_rule)
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
        };
        let mut output = Map::new();
        if let Some(rules) = rules {
            output.insert(
                "rules".into(),
                serde_json::to_value(rules)
                    .map_err(|error| ConfigValidationError::new(error.to_string()))?,
            );
        }
        Ok(Value::Object(output))
    })
});

pub fn parse_permission_rule(value: &Value) -> Result<PermissionRule, ConfigValidationError> {
    #[derive(Deserialize)]
    struct Raw {
        decision: String,
        #[serde(default = "default_permission_rule_scope")]
        scope: String,
        pattern: String,
        #[serde(default, deserialize_with = "deserialize_permission_rule_reason")]
        reason: Option<String>,
    }
    let raw = serde_json::from_value::<Raw>(value.clone())
        .map_err(|error| ConfigValidationError::new(error.to_string()))?;
    let decision = match raw.decision.as_str() {
        "allow" => PermissionRuleDecision::Allow,
        "deny" => PermissionRuleDecision::Deny,
        "ask" => PermissionRuleDecision::Ask,
        _ => {
            return Err(ConfigValidationError::new(
                "permission rule decision must be allow, deny, or ask",
            ));
        }
    };
    let scope = match raw.scope.as_str() {
        "user" => PermissionRuleScope::User,
        "turn-override" => PermissionRuleScope::TurnOverride,
        "session-runtime" => PermissionRuleScope::SessionRuntime,
        "project" => PermissionRuleScope::Project,
        _ => {
            return Err(ConfigValidationError::new("invalid permission rule scope"));
        }
    };
    let pattern = raw.pattern;
    if pattern.is_empty() || parse_permission_pattern(&pattern).is_err() {
        return Err(ConfigValidationError::new(
            "Invalid permission rule pattern",
        ));
    }
    Ok(PermissionRule {
        decision,
        scope,
        pattern,
        reason: raw.reason,
    })
}

fn default_permission_rule_scope() -> String {
    "user".into()
}

fn deserialize_permission_rule_reason<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<String>::deserialize(deserializer)? {
        Some(value) => Ok(Some(value)),
        None => Err(serde::de::Error::custom(
            "permission rule reason must be a string",
        )),
    }
}

// Original: configSection.ts, permissionFromToml().
pub fn permission_from_toml(raw_snake: &Value) -> Value {
    let Some(raw_snake) = raw_snake.as_object() else {
        return raw_snake.clone();
    };
    let raw = transform_plain_object(raw_snake);
    let mut rules = Vec::new();
    append_permission_rules(&mut rules, raw.get("rules"), None);
    append_permission_rules(&mut rules, raw.get("deny"), Some("deny"));
    append_permission_rules(&mut rules, raw.get("allow"), Some("allow"));
    append_permission_rules(&mut rules, raw.get("ask"), Some("ask"));
    if rules.is_empty() {
        Value::Object(Map::new())
    } else {
        Value::Object(
            [("rules".into(), Value::Array(rules))]
                .into_iter()
                .collect(),
        )
    }
}

fn append_permission_rules(target: &mut Vec<Value>, value: Option<&Value>, decision: Option<&str>) {
    let Some(value) = value else { return };
    if let Some(entries) = value.as_array() {
        target.extend(
            entries
                .iter()
                .map(|entry| transform_permission_rule(entry, decision)),
        );
    } else {
        target.push(transform_permission_rule(value, decision));
    }
}

fn transform_permission_rule(value: &Value, decision: Option<&str>) -> Value {
    let Some(value) = value.as_object() else {
        return value.clone();
    };
    let rule = transform_plain_object(value);
    let mut output = Map::new();
    if let Some(decision) = decision
        .map(|value| Value::String(value.into()))
        .or_else(|| rule.get("decision").cloned())
    {
        output.insert("decision".into(), decision);
    }
    for key in ["scope", "reason"] {
        if let Some(value) = rule.get(key) {
            output.insert(key.into(), value.clone());
        }
    }
    let pattern = match rule.get("tool").and_then(Value::as_str) {
        Some(tool) => Some(
            rule.get("match")
                .and_then(Value::as_str)
                .or_else(|| rule.get("pattern").and_then(Value::as_str))
                .map_or_else(
                    || Value::String(tool.into()),
                    |argument| Value::String(format!("{tool}({argument})")),
                ),
        ),
        None => rule.get("pattern").cloned(),
    };
    if let Some(pattern) = pattern {
        output.insert("pattern".into(), pattern);
    }
    Value::Object(output)
}

// Original: configSection.ts, permissionToToml().
pub fn permission_to_toml(value: &Value, raw_snake: &Value) -> Option<Value> {
    let Some(value) = value.as_object() else {
        return Some(value.clone());
    };
    let mut output = clone_record(raw_snake);
    for key in ["deny", "allow", "ask"] {
        output.shift_remove(key);
    }
    match value.get("rules").and_then(Value::as_array) {
        Some(rules) => {
            output.insert(
                "rules".into(),
                Value::Array(
                    rules
                        .iter()
                        .map(|rule| {
                            rule.as_object()
                                .map(|rule| Value::Object(plain_object_to_toml(rule, None)))
                                .unwrap_or_else(|| rule.clone())
                        })
                        .collect(),
                ),
            );
        }
        None => {
            output.shift_remove("rules");
        }
    }
    Some(Value::Object(output))
}

static PERMISSION_FROM_TOML: LazyLock<ConfigFromToml> =
    LazyLock::new(|| Arc::new(permission_from_toml));
static PERMISSION_TO_TOML: LazyLock<ConfigToToml> = LazyLock::new(|| Arc::new(permission_to_toml));

pub fn register_permission_config_section() {
    register_config_section(
        PERMISSION_SECTION,
        PERMISSION_CONFIG_SCHEMA.clone(),
        RegisterSectionOptions {
            from_toml: Some(Arc::clone(&PERMISSION_FROM_TOML)),
            to_toml: Some(Arc::clone(&PERMISSION_TO_TOML)),
            ..RegisterSectionOptions::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn schema_defaults_scope_strips_unknown_fields_and_rejects_invalid_values() {
        assert_eq!(
            PERMISSION_CONFIG_SCHEMA
                .parse(&json!({"rules": [{
                    "decision": "allow", "pattern": "Read", "future": true
                }], "future": true}))
                .unwrap(),
            json!({"rules": [{
                "decision": "allow", "scope": "user", "pattern": "Read"
            }]})
        );
        for invalid in [
            json!(null),
            json!({"rules": null}),
            json!({"rules": [{"decision": "allow", "pattern": ""}]}),
            json!({"rules": [{"decision": "allow", "pattern": "("}]}),
            json!({"rules": [{"decision": "maybe", "pattern": "Read"}]}),
            json!({"rules": [{"decision": "deny", "scope": null, "pattern": "Read"}]}),
            json!({"rules": [{"decision": "deny", "pattern": "Read", "reason": null}]}),
        ] {
            assert!(
                PERMISSION_CONFIG_SCHEMA.parse(&invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn from_toml_expands_shorthand_lists_and_tool_match_forms_in_order() {
        assert_eq!(
            permission_from_toml(&json!({
                "rules": {"decision": "ask", "pattern": "Agent(*)", "scope": "project"},
                "deny": ["invalid", {"tool": "Bash", "match": "rm *", "reason": "unsafe"}],
                "allow": {"tool": "Read", "pattern": "/repo/**"},
                "ask": {"tool": "Write"}
            })),
            json!({"rules": [
                {"decision": "ask", "scope": "project", "pattern": "Agent(*)"},
                "invalid",
                {"decision": "deny", "reason": "unsafe", "pattern": "Bash(rm *)"},
                {"decision": "allow", "pattern": "Read(/repo/**)"},
                {"decision": "ask", "pattern": "Write"}
            ]})
        );
        assert_eq!(permission_from_toml(&json!({"unknown": true})), json!({}));
        assert_eq!(permission_from_toml(&json!("raw")), json!("raw"));
    }

    #[test]
    fn to_toml_replaces_shorthand_preserves_raw_keys_and_snake_cases_rules() {
        assert_eq!(
            permission_to_toml(
                &json!({"rules": [{
                    "decision": "allow", "scope": "turn-override",
                    "pattern": "Read", "futureField": 1
                }]}),
                &json!({"deny": ["old"], "ask": ["old"], "owned_elsewhere": 2}),
            ),
            Some(json!({
                "owned_elsewhere": 2,
                "rules": [{
                    "decision": "allow", "scope": "turn-override",
                    "pattern": "Read", "future_field": 1
                }]
            }))
        );
        assert_eq!(
            permission_to_toml(&json!({}), &json!({"rules": ["old"], "allow": ["x"]})),
            Some(json!({}))
        );
        assert_eq!(
            permission_to_toml(&json!("raw"), &json!({"deny": []})),
            Some(json!("raw"))
        );
    }
}
