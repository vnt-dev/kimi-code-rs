//! Environment binding resolution for registered config sections.
//!
//! Original: `packages/agent-core-v2/src/app/config/configService.ts`,
//! `resolveBinding()`, `applyEnvBindings()`, and `applySectionEnv()`.

use serde_json::{Map, Value};

use super::contract::{AnyEnvBindings, ConfigValidationError, EnvBinding, GetEnv};

// Original: resolveBinding().
fn resolve_binding(
    binding: &EnvBinding,
    get_env: &GetEnv<'_>,
    existing: Option<&Value>,
) -> Result<Option<Value>, ConfigValidationError> {
    match binding {
        EnvBinding::Name(env) => Ok(get_env(env)
            .map(Value::String)
            .or_else(|| existing.cloned())),
        EnvBinding::Parsed {
            env,
            parse,
            default,
        } => match get_env(env) {
            Some(raw) => parse
                .as_ref()
                .map_or_else(|| Ok(Value::String(raw.clone())), |parse| parse(&raw))
                .map(Some),
            None if existing.is_none() => Ok(default.clone()),
            None => Ok(existing.cloned()),
        },
    }
}

// Original: applyEnvBindings().
fn apply_env_bindings(
    target: &mut Map<String, Value>,
    bindings: &indexmap::IndexMap<String, AnyEnvBindings>,
    get_env: &GetEnv<'_>,
) -> Result<(), ConfigValidationError> {
    for (key, binding) in bindings {
        match binding {
            AnyEnvBindings::Binding(binding) => {
                if let Some(resolved) = resolve_binding(binding, get_env, target.get(key))? {
                    target.insert(key.clone(), resolved);
                }
            }
            AnyEnvBindings::Fields(fields) => {
                let mut child = target
                    .get(key)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                apply_env_bindings(&mut child, fields, get_env)?;
                if child.is_empty() {
                    target.shift_remove(key);
                } else {
                    target.insert(key.clone(), Value::Object(child));
                }
            }
        }
    }
    Ok(())
}

// Original: applySectionEnv(). `None` represents an undefined section value.
pub fn apply_section_env(
    base: Option<&Value>,
    env: &AnyEnvBindings,
    get_env: &GetEnv<'_>,
) -> Result<Option<Value>, ConfigValidationError> {
    match env {
        AnyEnvBindings::Binding(binding) => resolve_binding(binding, get_env, base),
        AnyEnvBindings::Fields(fields) => {
            let mut target = base.and_then(Value::as_object).cloned().unwrap_or_default();
            apply_env_bindings(&mut target, fields, get_env)?;
            Ok(Some(Value::Object(target)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use indexmap::IndexMap;
    use serde_json::json;

    use super::*;

    fn getter(values: HashMap<String, String>) -> impl Fn(&str) -> Option<String> {
        move |name| values.get(name).cloned()
    }

    #[test]
    fn environment_overrides_existing_and_missing_environment_keeps_it() {
        let binding = AnyEnvBindings::Binding(EnvBinding::Name("MODEL".into()));
        assert_eq!(
            apply_section_env(
                Some(&json!("config")),
                &binding,
                &getter(HashMap::from([("MODEL".into(), "env".into())]))
            )
            .unwrap(),
            Some(json!("env"))
        );
        assert_eq!(
            apply_section_env(Some(&json!("config")), &binding, &getter(HashMap::new())).unwrap(),
            Some(json!("config"))
        );
    }

    #[test]
    fn parsed_binding_uses_default_only_for_an_undefined_value() {
        let binding = AnyEnvBindings::Binding(EnvBinding::Parsed {
            env: "COUNT".into(),
            parse: Some(Arc::new(|raw| {
                raw.parse::<u64>()
                    .map(Value::from)
                    .map_err(|_| ConfigValidationError::new("invalid count"))
            })),
            default: Some(json!(3)),
        });
        assert_eq!(
            apply_section_env(None, &binding, &getter(HashMap::new())).unwrap(),
            Some(json!(3))
        );
        assert_eq!(
            apply_section_env(Some(&Value::Null), &binding, &getter(HashMap::new())).unwrap(),
            Some(Value::Null)
        );
        assert!(
            apply_section_env(
                None,
                &binding,
                &getter(HashMap::from([("COUNT".into(), "bad".into())]))
            )
            .is_err()
        );
    }

    #[test]
    fn nested_bindings_create_and_remove_child_objects_like_the_source() {
        let bindings = AnyEnvBindings::Fields(IndexMap::from([
            (
                "provider".into(),
                AnyEnvBindings::Fields(IndexMap::from([(
                    "apiKey".into(),
                    AnyEnvBindings::Binding(EnvBinding::Name("API_KEY".into())),
                )])),
            ),
            (
                "keep".into(),
                AnyEnvBindings::Binding(EnvBinding::Name("MISSING".into())),
            ),
        ]));
        assert_eq!(
            apply_section_env(
                Some(&json!({"keep": true, "provider": "invalid"})),
                &bindings,
                &getter(HashMap::from([("API_KEY".into(), "secret".into())]))
            )
            .unwrap(),
            Some(json!({"keep": true, "provider": {"apiKey": "secret"}}))
        );
        assert_eq!(
            apply_section_env(None, &bindings, &getter(HashMap::new())).unwrap(),
            Some(json!({}))
        );
    }
}
