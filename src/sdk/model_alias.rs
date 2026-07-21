use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

const BUDGET_THINKING_EFFORTS: [&str; 3] = ["low", "medium", "high"];
const ADAPTIVE_MAX_EFFORTS: [&str; 4] = ["low", "medium", "high", "max"];
const LATEST_OPUS_THINKING_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

static FAMILY_FIRST_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(opus|sonnet|haiku|fable|mythos)[-._](\d{1,2})(?:[-._](\d{1,2}))?(?:[^0-9]|$)")
        .expect("Anthropic family-first model regex must compile")
});
static VERSION_FIRST_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{1,2})[-._](\d{1,2})[-._](opus|sonnet|haiku)")
        .expect("Anthropic version-first model regex must compile")
});
static BARE_FAMILY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{1,2})[-._](opus|sonnet|haiku)")
        .expect("Anthropic bare-family model regex must compile")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Anthropic,
    Openai,
    Kimi,
    #[serde(rename = "google-genai")]
    GoogleGenai,
    OpenaiResponses,
    Vertexai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelProtocol {
    Anthropic,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAliasOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAlias {
    pub provider: String,
    pub model: String,
    pub max_context_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ModelProtocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beta_api: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overrides: Option<ModelAliasOverrides>,
}

impl ModelAlias {
    fn apply_overrides(&self) -> Self {
        let mut effective = self.clone();
        let Some(overrides) = &self.overrides else {
            return effective;
        };
        effective.overrides = None;
        if let Some(value) = overrides.max_context_size {
            effective.max_context_size = value;
        }
        if let Some(value) = overrides.max_output_size {
            effective.max_output_size = Some(value);
        }
        if let Some(value) = &overrides.capabilities {
            effective.capabilities = Some(value.clone());
        }
        if let Some(value) = &overrides.display_name {
            effective.display_name = Some(value.clone());
        }
        if let Some(value) = &overrides.reasoning_key {
            effective.reasoning_key = Some(value.clone());
        }
        if let Some(value) = overrides.adaptive_thinking {
            effective.adaptive_thinking = Some(value);
        }
        if let Some(value) = &overrides.support_efforts {
            effective.support_efforts = Some(value.clone());
        }
        if let Some(value) = &overrides.default_effort {
            effective.default_effort = Some(value.clone());
        }
        if overrides.support_efforts.is_some()
            && overrides.default_effort.is_none()
            && effective.default_effort.as_ref().is_some_and(|default| {
                !overrides
                    .support_efforts
                    .as_ref()
                    .is_some_and(|efforts| efforts.contains(default))
            })
        {
            effective.default_effort = None;
        }
        effective
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnthropicModelFamily {
    Opus,
    Sonnet,
    Haiku,
    Fable,
    Mythos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AnthropicModelVersion {
    family: AnthropicModelFamily,
    major: u8,
    minor: Option<u8>,
}

#[derive(Debug, Clone, Copy)]
struct AnthropicModelProfile {
    efforts: &'static [&'static str],
    can_disable_thinking: bool,
}

const BUDGET_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    efforts: &BUDGET_THINKING_EFFORTS,
    can_disable_thinking: true,
};
const OPUS_45_PROFILE: AnthropicModelProfile = BUDGET_PROFILE;
const ADAPTIVE_MAX_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    efforts: &ADAPTIVE_MAX_EFFORTS,
    can_disable_thinking: true,
};
const LATEST_OPUS_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    efforts: &LATEST_OPUS_THINKING_EFFORTS,
    can_disable_thinking: true,
};
const ALWAYS_ADAPTIVE_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    efforts: &LATEST_OPUS_THINKING_EFFORTS,
    can_disable_thinking: false,
};
const ALWAYS_ADAPTIVE_MAX_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    efforts: &ADAPTIVE_MAX_EFFORTS,
    can_disable_thinking: false,
};

fn parse_family(value: &str) -> Option<AnthropicModelFamily> {
    match value {
        "opus" => Some(AnthropicModelFamily::Opus),
        "sonnet" => Some(AnthropicModelFamily::Sonnet),
        "haiku" => Some(AnthropicModelFamily::Haiku),
        "fable" => Some(AnthropicModelFamily::Fable),
        "mythos" => Some(AnthropicModelFamily::Mythos),
        _ => None,
    }
}

fn capture_u8(captures: &regex::Captures<'_>, index: usize) -> Option<u8> {
    captures.get(index)?.as_str().parse().ok()
}

fn parse_anthropic_model_version(model: &str) -> Option<AnthropicModelVersion> {
    let normalized = model.to_ascii_lowercase();
    if let Some(captures) = FAMILY_FIRST_RE.captures(&normalized) {
        return Some(AnthropicModelVersion {
            family: parse_family(captures.get(1)?.as_str())?,
            major: capture_u8(&captures, 2)?,
            minor: capture_u8(&captures, 3),
        });
    }
    if let Some(captures) = VERSION_FIRST_RE.captures(&normalized) {
        return Some(AnthropicModelVersion {
            family: parse_family(captures.get(3)?.as_str())?,
            major: capture_u8(&captures, 1)?,
            minor: capture_u8(&captures, 2),
        });
    }
    let captures = BARE_FAMILY_RE.captures(&normalized)?;
    Some(AnthropicModelVersion {
        family: parse_family(captures.get(2)?.as_str())?,
        major: capture_u8(&captures, 1)?,
        minor: None,
    })
}

fn match_known_anthropic_model_profile(model: &str) -> Option<AnthropicModelProfile> {
    let normalized = model.to_ascii_lowercase();
    if normalized.contains("mythos-preview")
        || normalized.contains("mythos.preview")
        || normalized.contains("mythos_preview")
    {
        return Some(ALWAYS_ADAPTIVE_MAX_PROFILE);
    }
    let version = parse_anthropic_model_version(model)?;
    match version.family {
        AnthropicModelFamily::Opus => match (version.major, version.minor) {
            (4, Some(7 | 8)) => Some(LATEST_OPUS_PROFILE),
            (4, Some(6)) => Some(ADAPTIVE_MAX_PROFILE),
            (4, Some(5)) => Some(OPUS_45_PROFILE),
            (major, _) if major < 4 => Some(BUDGET_PROFILE),
            (4, minor) if minor.unwrap_or(0) < 5 => Some(BUDGET_PROFILE),
            _ => None,
        },
        AnthropicModelFamily::Sonnet => match (version.major, version.minor) {
            (5, _) => Some(LATEST_OPUS_PROFILE),
            (4, Some(6)) => Some(ADAPTIVE_MAX_PROFILE),
            (major, _) if major < 4 => Some(BUDGET_PROFILE),
            (4, minor) if minor.unwrap_or(0) <= 5 => Some(BUDGET_PROFILE),
            _ => None,
        },
        AnthropicModelFamily::Haiku => match (version.major, version.minor) {
            (major, _) if major < 4 => Some(BUDGET_PROFILE),
            (4, minor) if minor.unwrap_or(0) <= 5 => Some(BUDGET_PROFILE),
            _ => None,
        },
        AnthropicModelFamily::Fable | AnthropicModelFamily::Mythos => {
            (version.major == 5).then_some(ALWAYS_ADAPTIVE_PROFILE)
        }
    }
}

fn with_anthropic_profile(
    mut model: ModelAlias,
    provider_type: Option<ProviderType>,
) -> ModelAlias {
    let protocol = model.protocol.or_else(|| {
        (provider_type == Some(ProviderType::Anthropic)).then_some(ModelProtocol::Anthropic)
    });
    let profile = if provider_type.is_some()
        && provider_type != Some(ProviderType::Kimi)
        && protocol == Some(ModelProtocol::Anthropic)
    {
        Some(match_known_anthropic_model_profile(&model.model).unwrap_or(LATEST_OPUS_PROFILE))
    } else {
        match_known_anthropic_model_profile(&model.model)
    };
    let Some(profile) = profile else {
        return model;
    };

    let capability = if profile.can_disable_thinking {
        "thinking"
    } else {
        "always_thinking"
    };
    let capabilities = model.capabilities.get_or_insert_with(Vec::new);
    if !capabilities
        .iter()
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(capability))
    {
        capabilities.push(capability.to_owned());
    }
    let efforts = model.support_efforts.get_or_insert_with(|| {
        if model.adaptive_thinking == Some(false) {
            BUDGET_THINKING_EFFORTS.map(str::to_owned).to_vec()
        } else {
            profile
                .efforts
                .iter()
                .map(|effort| (*effort).to_owned())
                .collect()
        }
    });
    if model.default_effort.is_none() && efforts.iter().any(|effort| effort == "high") {
        model.default_effort = Some("high".to_owned());
    }
    model
}

// Original:
//   packages/agent-core/src/config/model.ts
//   effectiveModelAlias()
pub fn effective_model_alias(
    alias: &ModelAlias,
    provider_type: Option<ProviderType>,
) -> ModelAlias {
    with_anthropic_profile(alias.apply_overrides(), provider_type)
}

// Original: effectiveModelAliases()
pub fn effective_model_aliases(
    models: &BTreeMap<String, ModelAlias>,
) -> BTreeMap<String, ModelAlias> {
    models
        .iter()
        .map(|(name, model)| (name.clone(), effective_model_alias(model, None)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias(overrides: Option<ModelAliasOverrides>) -> ModelAlias {
        ModelAlias {
            provider: "managed:kimi-code".to_owned(),
            model: "kimi-k2".to_owned(),
            max_context_size: 262_144,
            max_output_size: None,
            capabilities: Some(vec!["thinking".to_owned()]),
            display_name: None,
            reasoning_key: None,
            protocol: None,
            adaptive_thinking: None,
            support_efforts: Some(vec!["low".to_owned(), "high".to_owned(), "max".to_owned()]),
            default_effort: Some("max".to_owned()),
            beta_api: None,
            overrides,
        }
    }

    fn anthropic_alias(model: &str) -> ModelAlias {
        ModelAlias {
            provider: "anthropic".to_owned(),
            model: model.to_owned(),
            max_context_size: 200_000,
            max_output_size: None,
            capabilities: None,
            display_name: None,
            reasoning_key: None,
            protocol: None,
            adaptive_thinking: None,
            support_efforts: None,
            default_effort: None,
            beta_api: None,
            overrides: None,
        }
    }

    #[test]
    fn applies_overrides_and_drops_an_incompatible_default() {
        let original = alias(Some(ModelAliasOverrides {
            max_context_size: Some(128_000),
            support_efforts: Some(vec!["low".to_owned(), "high".to_owned()]),
            ..ModelAliasOverrides::default()
        }));
        let effective = effective_model_alias(&original, None);
        assert_eq!(effective.max_context_size, 128_000);
        assert_eq!(
            effective.support_efforts,
            Some(vec!["low".to_owned(), "high".to_owned()])
        );
        assert_eq!(effective.default_effort, None);
        assert_eq!(effective.overrides, None);
    }

    #[test]
    fn keeps_an_explicit_valid_default_effort_override() {
        let original = alias(Some(ModelAliasOverrides {
            support_efforts: Some(vec!["low".to_owned(), "high".to_owned()]),
            default_effort: Some("high".to_owned()),
            ..ModelAliasOverrides::default()
        }));
        assert_eq!(
            effective_model_alias(&original, None)
                .default_effort
                .as_deref(),
            Some("high")
        );
    }

    #[test]
    fn derives_official_claude_profiles() {
        let opus = effective_model_alias(&anthropic_alias("claude-opus-4-6"), None);
        assert_eq!(opus.capabilities, Some(vec!["thinking".to_owned()]));
        assert_eq!(
            opus.support_efforts,
            Some(
                vec!["low", "medium", "high", "max"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );
        assert_eq!(opus.default_effort.as_deref(), Some("high"));

        let fable = effective_model_alias(&anthropic_alias("claude-fable-5"), None);
        assert_eq!(fable.capabilities, Some(vec!["always_thinking".to_owned()]));
        assert_eq!(
            fable.support_efforts,
            Some(
                vec!["low", "medium", "high", "xhigh", "max"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );
    }

    #[test]
    fn infers_only_for_an_explicit_non_kimi_anthropic_provider() {
        let mut custom = anthropic_alias("custom-anthropic-model");
        custom.provider = "custom".to_owned();
        custom.protocol = Some(ModelProtocol::Anthropic);
        assert_eq!(effective_model_alias(&custom, None), custom);

        let inferred = effective_model_alias(&custom, Some(ProviderType::Anthropic));
        assert_eq!(inferred.capabilities, Some(vec!["thinking".to_owned()]));
        assert_eq!(
            inferred.support_efforts,
            Some(
                vec!["low", "medium", "high", "xhigh", "max"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );

        custom.provider = "managed:kimi-code".to_owned();
        custom.adaptive_thinking = Some(true);
        custom.capabilities = Some(vec!["thinking".to_owned(), "always_thinking".to_owned()]);
        assert_eq!(
            effective_model_alias(&custom, Some(ProviderType::Kimi)),
            custom
        );
    }

    #[test]
    fn adaptive_false_uses_budget_efforts_but_declared_efforts_win() {
        let mut custom = anthropic_alias("custom-anthropic-model");
        custom.provider = "custom".to_owned();
        custom.protocol = Some(ModelProtocol::Anthropic);
        custom.adaptive_thinking = Some(false);
        let budget = effective_model_alias(&custom, Some(ProviderType::Anthropic));
        assert_eq!(
            budget.support_efforts,
            Some(
                vec!["low", "medium", "high"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );

        custom.support_efforts = Some(vec!["low".to_owned(), "high".to_owned()]);
        assert_eq!(
            effective_model_alias(&custom, Some(ProviderType::Anthropic)).support_efforts,
            custom.support_efforts
        );
    }

    #[test]
    fn keeps_declared_efforts_authoritative_for_known_models() {
        let mut opus = anthropic_alias("claude-opus-4-7");
        opus.support_efforts = Some(vec!["low".to_owned(), "max".to_owned()]);
        opus.default_effort = Some("max".to_owned());
        let effective = effective_model_alias(&opus, None);
        assert_eq!(effective.support_efforts, opus.support_efforts);
        assert_eq!(effective.default_effort, opus.default_effort);
    }
}
