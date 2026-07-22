use super::types::{ModelThinkingCapabilities, ModelThinkingMetadata, ThinkingDefaults};
use crate::agent_core_v2::kosong::contract::provider::ThinkingEffort;

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

// MIGRATION-TODO:
// Original: packages/agent-core-v2/src/kosong/model/thinking.ts
// Missing units: thinking config-section registration,
// drivesThinkingThroughTraits(), usesTraitDrivenThinking(), and
// requiresStrictThinkingValidation().
// Temporary behavior: callers must supply the already-resolved
// strict_validation and trait_driven verdicts to these pure helpers.
// Completion condition: migrate the config contribution and protocol/provider
// trait registries, then port the registry-driven methods without string gates.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::capability::ModelCapability;

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
