use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{ModelThinkingCapabilities, ModelThinkingMetadata, ThinkingDefaults};
use crate::app::config::{
    AnyEnvBindings, ConfigSchema, ConfigStripEnv, ConfigValidationError, EnvBinding,
    RegisterSectionOptions, register_config_section,
};
use crate::kosong::{
    contract::provider::ThinkingEffort,
    protocol::identity::{Protocol, ProtocolAdapterRegistry},
    provider::provider_definition::{ProviderDefinitionRegistryError, get_provider_definitions},
};

pub const THINKING_SECTION: &str = "thinking";
pub const MODEL_THINKING_EFFORT_ENV: &str = "KIMI_MODEL_THINKING_EFFORT";

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<String>,
}

pub static THINKING_CONFIG_SCHEMA: LazyLock<ConfigSchema> = LazyLock::new(|| {
    ConfigSchema::new(|value| {
        if !value.is_object() {
            return Err(ConfigValidationError::new(
                "thinking config must be an object",
            ));
        }
        let config: ThinkingConfig = serde_json::from_value(value.clone())
            .map_err(|error| ConfigValidationError::new(error.to_string()))?;
        serde_json::to_value(config).map_err(|error| ConfigValidationError::new(error.to_string()))
    })
});

pub static THINKING_ENV_BINDINGS: LazyLock<Arc<AnyEnvBindings>> = LazyLock::new(|| {
    Arc::new(AnyEnvBindings::Fields(IndexMap::from([(
        "forcedEffort".into(),
        AnyEnvBindings::Binding(EnvBinding::Name(MODEL_THINKING_EFFORT_ENV.into())),
    )])))
});

pub static STRIP_THINKING_ENV: LazyLock<ConfigStripEnv> = LazyLock::new(|| {
    Arc::new(|value, _| {
        let mut result = value.as_object()?.clone();
        result.remove("forcedEffort");
        Some(Value::Object(result))
    })
});

// Original: thinking.ts, registerConfigSection() side effect.
pub fn register_thinking_config_section() {
    register_config_section(
        THINKING_SECTION,
        THINKING_CONFIG_SCHEMA.clone(),
        RegisterSectionOptions {
            env: Some(Arc::clone(&THINKING_ENV_BINDINGS)),
            strip_env: Some(Arc::clone(&STRIP_THINKING_ENV)),
            ..RegisterSectionOptions::default()
        },
    );
}

// Original: thinking.ts, drivesThinkingThroughTraits().
pub fn drives_thinking_through_traits(
    provider_type: Option<&str>,
) -> Result<bool, ProviderDefinitionRegistryError> {
    let Some(provider_type) = provider_type else {
        return Ok(false);
    };
    Ok(get_provider_definitions(provider_type)?
        .iter()
        .any(|definition| {
            definition
                .traits
                .iter()
                .any(|protocol_trait| protocol_trait.with_thinking.is_some())
        }))
}

// Original: thinking.ts, usesTraitDrivenThinking().
pub fn uses_trait_driven_thinking(
    registry: &dyn ProtocolAdapterRegistry,
    protocol: Protocol,
    provider_type: Option<&str>,
) -> bool {
    registry
        .resolve_adapter_identity(protocol, provider_type)
        .traits
        .iter()
        .any(|resolved| resolved.protocol_trait.with_thinking.is_some())
}

// Original: thinking.ts, requiresStrictThinkingValidation(). The last trait
// that declares withThinking owns the strictness verdict.
pub fn requires_strict_thinking_validation(
    registry: &dyn ProtocolAdapterRegistry,
    protocol: Protocol,
    provider_type: Option<&str>,
) -> bool {
    if provider_type.is_none() {
        return false;
    }
    let mut strict = false;
    for resolved in registry
        .resolve_adapter_identity(protocol, provider_type)
        .traits
    {
        if resolved.protocol_trait.with_thinking.is_some() {
            strict = resolved.protocol_trait.strict_thinking_validation == Some(true);
        }
    }
    strict
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

// Original:
//   packages/agent-core-v2/src/kosong/model/thinking.ts
//   normalizeRequestedThinkingEffort()
pub fn normalize_requested_thinking_effort(requested: Option<&str>) -> Option<ThinkingEffort> {
    non_empty(requested).map(|value| ThinkingEffort::new(value.to_lowercase()))
}

// Original:
//   packages/agent-core-v2/src/kosong/model/thinking.ts
//   resolveForcedThinkingEffort()
pub fn resolve_forced_thinking_effort(
    forced: Option<&str>,
    effective: &ThinkingEffort,
    trait_driven: bool,
) -> Option<ThinkingEffort> {
    if !trait_driven || effective.is_off() {
        return None;
    }
    non_empty(forced).map(ThinkingEffort::from)
}

fn has_capability(capabilities: Option<&ModelThinkingCapabilities>, capability: &str) -> bool {
    match capabilities {
        None => false,
        Some(ModelThinkingCapabilities::Names(capabilities)) => capabilities
            .iter()
            .any(|candidate| candidate.trim().eq_ignore_ascii_case(capability)),
        Some(ModelThinkingCapabilities::Structured(capabilities)) => match capability {
            "thinking" => capabilities.thinking,
            "always_thinking" => false,
            _ => false,
        },
    }
}

fn efforts_for(model: Option<&ModelThinkingMetadata>) -> Vec<&str> {
    model
        .and_then(|model| model.support_efforts.as_deref())
        .unwrap_or_default()
        .iter()
        .filter_map(|effort| non_empty(Some(effort)))
        .collect()
}

// Original:
//   packages/agent-core-v2/src/kosong/model/thinking.ts
//   modelSupportsThinking()
pub fn model_supports_thinking(model: Option<&ModelThinkingMetadata>) -> bool {
    let Some(model) = model else {
        return false;
    };
    model.always_thinking == Some(true)
        || model.adaptive_thinking == Some(true)
        || has_capability(model.capabilities.as_ref(), "thinking")
        || has_capability(model.capabilities.as_ref(), "always_thinking")
}

// Original:
//   packages/agent-core-v2/src/kosong/model/thinking.ts
//   defaultThinkingEffortForModel()
pub fn default_thinking_effort_for_model(model: Option<&ModelThinkingMetadata>) -> ThinkingEffort {
    if !model_supports_thinking(model) {
        return ThinkingEffort::from("off");
    }

    let efforts = efforts_for(model);
    if efforts.is_empty() {
        return ThinkingEffort::from("on");
    }

    let declared_default = model.and_then(|model| non_empty(model.default_effort.as_deref()));
    if let Some(default) = declared_default
        && efforts.contains(&default)
    {
        return ThinkingEffort::from(default);
    }
    ThinkingEffort::from(efforts[efforts.len() / 2])
}

// Original:
//   packages/agent-core-v2/src/kosong/model/thinking.ts
//   modelSupportsThinkingEffort()
pub fn model_supports_thinking_effort(
    effort: &ThinkingEffort,
    model: Option<&ModelThinkingMetadata>,
    strict_validation: bool,
) -> bool {
    if !strict_validation || effort.is_off() {
        return true;
    }
    if !model_supports_thinking(model) {
        return false;
    }
    let efforts = efforts_for(model);
    efforts.is_empty() || effort.as_str() == "on" || efforts.contains(&effort.as_str())
}

fn normalize_thinking_effort_for_model(
    effort: ThinkingEffort,
    model: Option<&ModelThinkingMetadata>,
    strict_validation: bool,
) -> ThinkingEffort {
    if effort.is_off() && model.and_then(|model| model.always_thinking) != Some(true) {
        return ThinkingEffort::from("off");
    }

    let efforts = efforts_for(model);
    if !strict_validation {
        if effort.as_str() == "on" && !efforts.is_empty() {
            return default_thinking_effort_for_model(model);
        }
        return effort;
    }
    if !model_supports_thinking(model) {
        return ThinkingEffort::from("off");
    }
    if efforts.is_empty() {
        return ThinkingEffort::from("on");
    }
    if effort.as_str() == "on" || !efforts.contains(&effort.as_str()) {
        return default_thinking_effort_for_model(model);
    }
    effort
}

// Original:
//   packages/agent-core-v2/src/kosong/model/thinking.ts
//   resolveThinkingEffortForModel()
pub fn resolve_thinking_effort_for_model(
    requested: Option<&str>,
    defaults: Option<&ThinkingDefaults>,
    model: Option<&ModelThinkingMetadata>,
    strict_validation: bool,
) -> ThinkingEffort {
    let configured = defaults.and_then(|defaults| non_empty(defaults.effort.as_deref()));
    let normalized = normalize_requested_thinking_effort(requested);
    let mut effort = if let Some(normalized) = normalized {
        normalized
    } else if defaults.and_then(|defaults| defaults.enabled) == Some(false) {
        ThinkingEffort::from("off")
    } else if let Some(configured) = configured {
        ThinkingEffort::from(configured)
    } else {
        default_thinking_effort_for_model(model)
    };

    if strict_validation
        && effort.is_off()
        && model.and_then(|model| model.always_thinking) == Some(true)
    {
        effort = configured.map_or_else(
            || default_thinking_effort_for_model(model),
            ThinkingEffort::from,
        );
    }
    normalize_thinking_effort_for_model(effort, model, strict_validation)
}

enum KeepResolution<'a> {
    Unspecified,
    Specified(Option<&'a str>),
}

fn parse_keep_value(raw: Option<&str>) -> KeepResolution<'_> {
    let Some(trimmed) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return KeepResolution::Unspecified;
    };
    if ["0", "false", "no", "off", "none", "null"]
        .iter()
        .any(|value| trimmed.eq_ignore_ascii_case(value))
    {
        KeepResolution::Specified(None)
    } else {
        KeepResolution::Specified(Some(trimmed))
    }
}

// Original:
//   packages/agent-core-v2/src/kosong/model/thinking.ts
//   resolveThinkingKeep()
pub fn resolve_thinking_keep(
    env_keep: Option<&str>,
    config_keep: Option<&str>,
    thinking_effort: &ThinkingEffort,
) -> Option<String> {
    if thinking_effort.is_off() {
        return None;
    }
    match parse_keep_value(env_keep) {
        KeepResolution::Specified(value) => return value.map(str::to_owned),
        KeepResolution::Unspecified => {}
    }
    match parse_keep_value(config_keep) {
        KeepResolution::Specified(value) => value.map(str::to_owned),
        KeepResolution::Unspecified => Some("all".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, OnceLock, RwLock},
    };

    use super::*;
    use crate::app::config::apply_section_env;
    use crate::kosong::{
        contract::capability::ModelCapability,
        protocol::{protocol_base::ProtocolBaseRegistry, protocol_trait::ProtocolTrait},
        provider::{
            protocol_adapter_registry::ProtocolAdapterRegistry as ConcreteRegistry,
            provider_definition::{
                ProviderDefinition, ProviderDefinitionRegistry, register_provider_definition,
            },
        },
    };

    fn thinking_trait(strict: Option<bool>) -> Arc<ProtocolTrait> {
        Arc::new(ProtocolTrait {
            strict_thinking_validation: strict,
            with_thinking: Some(Arc::new(|_, _, _, _| None)),
            ..ProtocolTrait::default()
        })
    }

    fn thinking_model() -> ModelThinkingMetadata {
        ModelThinkingMetadata {
            capabilities: Some(ModelThinkingCapabilities::Names(vec![
                "thinking".to_owned(),
            ])),
            support_efforts: Some(vec![
                "low".to_owned(),
                "medium".to_owned(),
                "high".to_owned(),
            ]),
            default_effort: Some("high".to_owned()),
            ..ModelThinkingMetadata::default()
        }
    }

    #[test]
    fn thinking_schema_env_binding_and_strip_preserve_source_behavior() {
        let parsed = THINKING_CONFIG_SCHEMA
            .parse(&serde_json::json!({
                "enabled": true,
                "effort": "high",
                "forcedEffort": "max",
                "keep": "all",
                "unknown": "discarded"
            }))
            .unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "enabled": true,
                "effort": "high",
                "forcedEffort": "max",
                "keep": "all"
            })
        );
        for invalid in [
            serde_json::json!(null),
            serde_json::json!({"enabled": "true"}),
            serde_json::json!({"effort": 3}),
        ] {
            assert!(THINKING_CONFIG_SCHEMA.parse(&invalid).is_err());
        }

        let get_env = |name: &str| {
            HashMap::from([(MODEL_THINKING_EFFORT_ENV, "xhigh")])
                .get(name)
                .map(ToString::to_string)
        };
        let effective = apply_section_env(Some(&parsed), &THINKING_ENV_BINDINGS, &get_env)
            .unwrap()
            .unwrap();
        assert_eq!(effective["forcedEffort"], "xhigh");
        assert_eq!(
            STRIP_THINKING_ENV(&effective, None).unwrap(),
            serde_json::json!({"enabled": true, "effort": "high", "keep": "all"})
        );
    }

    #[test]
    fn thinking_section_registers_schema_env_and_strip_hooks() {
        crate::app::config::clear_config_section_contributions_for_tests();
        register_thinking_config_section();
        let contributions = crate::app::config::get_config_section_contributions();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].domain, THINKING_SECTION);
        assert!(contributions[0].options.env.is_some());
        assert!(contributions[0].options.strip_env.is_some());
        crate::app::config::clear_config_section_contributions_for_tests();
    }

    #[test]
    fn registry_verdicts_follow_declared_resolved_and_last_strict_traits() {
        static GLOBAL_PROVIDER: OnceLock<()> = OnceLock::new();
        GLOBAL_PROVIDER.get_or_init(|| {
            register_provider_definition(ProviderDefinition {
                id: "thinking-verdict-global".into(),
                base_protocol: Protocol::OpenAi,
                traits: vec![thinking_trait(Some(true))],
                endpoint: None,
                host_headers: None,
                model_source: None,
                capability: None,
            })
            .unwrap();
        });
        assert!(!drives_thinking_through_traits(None).unwrap());
        assert!(!drives_thinking_through_traits(Some("unregistered")).unwrap());
        assert!(drives_thinking_through_traits(Some("thinking-verdict-global")).unwrap());

        let providers = RwLock::new(ProviderDefinitionRegistry::default());
        providers
            .write()
            .unwrap()
            .register(ProviderDefinition {
                id: "thinking-verdict-local".into(),
                base_protocol: Protocol::Anthropic,
                traits: vec![thinking_trait(Some(true)), thinking_trait(Some(false))],
                endpoint: None,
                host_headers: None,
                model_source: None,
                capability: None,
            })
            .unwrap();
        providers
            .write()
            .unwrap()
            .register(ProviderDefinition {
                id: "thinking-verdict-strict".into(),
                base_protocol: Protocol::Anthropic,
                traits: vec![thinking_trait(Some(false)), thinking_trait(Some(true))],
                endpoint: None,
                host_headers: None,
                model_source: None,
                capability: None,
            })
            .unwrap();
        let bases = RwLock::new(ProtocolBaseRegistry::default());
        let registry = ConcreteRegistry::with_registries(&providers, &bases);
        assert!(uses_trait_driven_thinking(
            &registry,
            Protocol::Anthropic,
            Some("thinking-verdict-local")
        ));
        assert!(!requires_strict_thinking_validation(
            &registry,
            Protocol::Anthropic,
            Some("thinking-verdict-local")
        ));
        assert!(requires_strict_thinking_validation(
            &registry,
            Protocol::Anthropic,
            Some("thinking-verdict-strict")
        ));
        assert!(!requires_strict_thinking_validation(
            &registry,
            Protocol::Anthropic,
            None
        ));
    }

    #[test]
    fn request_then_config_then_model_default_determine_effort() {
        let model = thinking_model();
        assert_eq!(
            resolve_thinking_effort_for_model(Some("HIGH"), None, Some(&model), true).as_str(),
            "high"
        );
        assert_eq!(
            resolve_thinking_effort_for_model(
                None,
                Some(&ThinkingDefaults {
                    enabled: None,
                    effort: Some("low".to_owned()),
                }),
                Some(&model),
                true,
            )
            .as_str(),
            "low"
        );
        assert_eq!(
            resolve_thinking_effort_for_model(None, None, Some(&model), true).as_str(),
            "high"
        );
        assert_eq!(
            resolve_thinking_effort_for_model(
                None,
                Some(&ThinkingDefaults {
                    enabled: Some(false),
                    effort: None,
                }),
                Some(&model),
                true,
            )
            .as_str(),
            "off"
        );
    }

    #[test]
    fn model_default_uses_declared_value_then_middle_then_on_or_off() {
        let mut model = thinking_model();
        model.default_effort = None;
        assert_eq!(
            default_thinking_effort_for_model(Some(&model)).as_str(),
            "medium"
        );
        model.support_efforts = None;
        assert_eq!(
            default_thinking_effort_for_model(Some(&model)).as_str(),
            "on"
        );
        assert_eq!(default_thinking_effort_for_model(None).as_str(), "off");
    }

    #[test]
    fn strict_mode_normalizes_unknown_and_on_to_model_default() {
        let model = thinking_model();
        assert_eq!(
            resolve_thinking_effort_for_model(Some("extreme"), None, Some(&model), true).as_str(),
            "high"
        );
        assert_eq!(
            resolve_thinking_effort_for_model(Some("extreme"), None, Some(&model), false).as_str(),
            "extreme"
        );
        assert_eq!(
            resolve_thinking_effort_for_model(Some("on"), None, Some(&model), true).as_str(),
            "high"
        );
    }

    #[test]
    fn strict_mode_keeps_always_thinking_models_on() {
        let always = ModelThinkingMetadata {
            capabilities: Some(ModelThinkingCapabilities::Names(vec![
                "always_thinking".to_owned(),
            ])),
            always_thinking: Some(true),
            support_efforts: Some(vec!["low".to_owned(), "high".to_owned()]),
            default_effort: Some("low".to_owned()),
            ..ModelThinkingMetadata::default()
        };
        assert_eq!(
            resolve_thinking_effort_for_model(Some("off"), None, Some(&always), true).as_str(),
            "low"
        );
        assert_eq!(
            resolve_thinking_effort_for_model(Some("off"), None, Some(&thinking_model()), true,)
                .as_str(),
            "off"
        );
    }

    #[test]
    fn support_validation_matches_declared_efforts_and_strictness() {
        let model = thinking_model();
        assert!(model_supports_thinking_effort(
            &ThinkingEffort::from("high"),
            Some(&model),
            true,
        ));
        assert!(!model_supports_thinking_effort(
            &ThinkingEffort::from("extreme"),
            Some(&model),
            true,
        ));
        assert!(model_supports_thinking_effort(
            &ThinkingEffort::from("off"),
            Some(&model),
            true,
        ));
        assert!(model_supports_thinking_effort(
            &ThinkingEffort::from("extreme"),
            Some(&model),
            false,
        ));
    }

    #[test]
    fn forced_effort_only_applies_to_trait_driven_thinking() {
        assert_eq!(
            resolve_forced_thinking_effort(Some("low"), &ThinkingEffort::from("high"), true,)
                .unwrap()
                .as_str(),
            "low"
        );
        assert_eq!(
            resolve_forced_thinking_effort(Some("low"), &ThinkingEffort::from("off"), true,),
            None
        );
        assert_eq!(
            resolve_forced_thinking_effort(Some("low"), &ThinkingEffort::from("high"), false,),
            None
        );
        assert_eq!(
            resolve_forced_thinking_effort(None, &ThinkingEffort::from("high"), true),
            None
        );
    }

    #[test]
    fn keep_resolution_preserves_disable_and_precedence_rules() {
        assert_eq!(
            resolve_thinking_keep(Some("all"), Some("all"), &ThinkingEffort::from("off"),),
            None
        );
        for (env, config) in [
            (Some("off"), None),
            (Some("0"), Some("all")),
            (None, Some("none")),
        ] {
            assert_eq!(
                resolve_thinking_keep(env, config, &ThinkingEffort::from("on")),
                None
            );
        }
        assert_eq!(
            resolve_thinking_keep(Some("summary"), Some("all"), &ThinkingEffort::from("on"),),
            Some("summary".to_owned())
        );
        assert_eq!(
            resolve_thinking_keep(None, Some("summary"), &ThinkingEffort::from("on")),
            Some("summary".to_owned())
        );
        assert_eq!(
            resolve_thinking_keep(None, None, &ThinkingEffort::from("on")),
            Some("all".to_owned())
        );
    }

    #[test]
    fn structured_capability_and_legacy_names_have_matching_semantics() {
        let structured = ModelThinkingMetadata {
            capabilities: Some(ModelThinkingCapabilities::Structured(ModelCapability {
                image_in: false,
                video_in: false,
                audio_in: false,
                thinking: true,
                tool_use: true,
                max_context_tokens: 128_000,
                dynamically_loaded_tools: None,
            })),
            ..ModelThinkingMetadata::default()
        };
        let names = ModelThinkingMetadata {
            capabilities: Some(ModelThinkingCapabilities::Names(vec![
                " THINKING ".to_owned(),
            ])),
            ..ModelThinkingMetadata::default()
        };
        assert!(model_supports_thinking(Some(&structured)));
        assert!(model_supports_thinking(Some(&names)));
    }
}
