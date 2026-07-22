use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicThinkingMode {
    Budget,
    Adaptive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicModelProfile {
    pub mode: AnthropicThinkingMode,
    pub efforts: &'static [&'static str],
    pub supports_effort_param: bool,
    pub can_disable_thinking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicModelFamily {
    Opus,
    Sonnet,
    Haiku,
    Fable,
    Mythos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnthropicModelVersion {
    pub family: AnthropicModelFamily,
    pub major: u8,
    pub minor: Option<u8>,
}

pub const BUDGET_THINKING_EFFORTS: &[&str] = &["low", "medium", "high"];
const ADAPTIVE_MAX_EFFORTS: &[&str] = &["low", "medium", "high", "max"];
pub const LATEST_OPUS_THINKING_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

const BUDGET_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    mode: AnthropicThinkingMode::Budget,
    efforts: BUDGET_THINKING_EFFORTS,
    supports_effort_param: false,
    can_disable_thinking: true,
};
const OPUS_45_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    supports_effort_param: true,
    ..BUDGET_PROFILE
};
const ADAPTIVE_MAX_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    mode: AnthropicThinkingMode::Adaptive,
    efforts: ADAPTIVE_MAX_EFFORTS,
    supports_effort_param: true,
    can_disable_thinking: true,
};
pub const LATEST_OPUS_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    mode: AnthropicThinkingMode::Adaptive,
    efforts: LATEST_OPUS_THINKING_EFFORTS,
    supports_effort_param: true,
    can_disable_thinking: true,
};
const ALWAYS_ADAPTIVE_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    can_disable_thinking: false,
    ..LATEST_OPUS_PROFILE
};
const ALWAYS_ADAPTIVE_MAX_PROFILE: AnthropicModelProfile = AnthropicModelProfile {
    can_disable_thinking: false,
    ..ADAPTIVE_MAX_PROFILE
};

// Rust regex has no lookaround. The final alternation consumes (without
// capturing) one non-digit boundary or the end, which is equivalent to the
// source's `(?!\d)` assertions for the captured version fields.
static FAMILY_FIRST_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(opus|sonnet|haiku|fable|mythos)[-._]([0-9]{1,2})(?:[-._]([0-9]{1,2})(?:[^0-9]|$)|(?:[^0-9]|$))",
    )
    .unwrap()
});
static VERSION_FIRST_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9]{1,2})[-._]([0-9]{1,2})[-._](opus|sonnet|haiku)").unwrap());
static BARE_FAMILY_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9]{1,2})[-._](opus|sonnet|haiku)").unwrap());

// Original:
//   packages/agent-core-v2/src/kosong/provider/bases/anthropic/anthropic-profile.ts
//   parseAnthropicModelVersion()
pub fn parse_anthropic_model_version(
    model: &str,
    require_claude_marker: bool,
) -> Option<AnthropicModelVersion> {
    let normalized = model.to_lowercase();
    if require_claude_marker && !normalized.contains("claude") {
        return None;
    }

    if let Some(captures) = FAMILY_FIRST_PATTERN.captures(&normalized) {
        return Some(AnthropicModelVersion {
            family: parse_family(captures.get(1)?.as_str())?,
            major: captures.get(2)?.as_str().parse().ok()?,
            minor: captures
                .get(3)
                .and_then(|value| value.as_str().parse().ok()),
        });
    }
    if let Some(captures) = VERSION_FIRST_PATTERN.captures(&normalized) {
        return Some(AnthropicModelVersion {
            major: captures.get(1)?.as_str().parse().ok()?,
            minor: Some(captures.get(2)?.as_str().parse().ok()?),
            family: parse_family(captures.get(3)?.as_str())?,
        });
    }
    let captures = BARE_FAMILY_PATTERN.captures(&normalized)?;
    Some(AnthropicModelVersion {
        major: captures.get(1)?.as_str().parse().ok()?,
        minor: None,
        family: parse_family(captures.get(2)?.as_str())?,
    })
}

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

// Original: anthropic-profile.ts, matchKnownAnthropicModelProfile()
pub fn match_known_anthropic_model_profile(model: &str) -> Option<AnthropicModelProfile> {
    let normalized = model.to_lowercase();
    if normalized.contains("mythos-preview")
        || normalized.contains("mythos.preview")
        || normalized.contains("mythos_preview")
    {
        return Some(ALWAYS_ADAPTIVE_MAX_PROFILE);
    }

    let version = parse_anthropic_model_version(model, false)?;
    match version.family {
        AnthropicModelFamily::Opus => {
            if version.major == 4 && matches!(version.minor, Some(7 | 8)) {
                Some(LATEST_OPUS_PROFILE)
            } else if version.major == 4 && version.minor == Some(6) {
                Some(ADAPTIVE_MAX_PROFILE)
            } else if version.major == 4 && version.minor == Some(5) {
                Some(OPUS_45_PROFILE)
            } else if version.major < 4 || (version.major == 4 && version.minor.unwrap_or(0) < 5) {
                Some(BUDGET_PROFILE)
            } else {
                None
            }
        }
        AnthropicModelFamily::Sonnet => {
            if version.major == 5 {
                Some(LATEST_OPUS_PROFILE)
            } else if version.major == 4 && version.minor == Some(6) {
                Some(ADAPTIVE_MAX_PROFILE)
            } else if version.major < 4 || (version.major == 4 && version.minor.unwrap_or(0) <= 5) {
                Some(BUDGET_PROFILE)
            } else {
                None
            }
        }
        AnthropicModelFamily::Haiku => {
            if version.major < 4 || (version.major == 4 && version.minor.unwrap_or(0) <= 5) {
                Some(BUDGET_PROFILE)
            } else {
                None
            }
        }
        AnthropicModelFamily::Fable | AnthropicModelFamily::Mythos => {
            (version.major == 5).then_some(ALWAYS_ADAPTIVE_PROFILE)
        }
    }
}

// Original: anthropic-profile.ts, inferAnthropicModelProfile()
pub fn infer_anthropic_model_profile(model: &str) -> AnthropicModelProfile {
    match_known_anthropic_model_profile(model).unwrap_or(LATEST_OPUS_PROFILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_supported_name_orders_and_marker_gate() {
        assert_eq!(
            parse_anthropic_model_version("claude-opus-4-6-20250514", true),
            Some(AnthropicModelVersion {
                family: AnthropicModelFamily::Opus,
                major: 4,
                minor: Some(6),
            })
        );
        assert_eq!(
            parse_anthropic_model_version("CLAUDE-4.5-SONNET", true),
            Some(AnthropicModelVersion {
                family: AnthropicModelFamily::Sonnet,
                major: 4,
                minor: Some(5),
            })
        );
        assert_eq!(
            parse_anthropic_model_version("4-haiku", false),
            Some(AnthropicModelVersion {
                family: AnthropicModelFamily::Haiku,
                major: 4,
                minor: None,
            })
        );
        assert_eq!(parse_anthropic_model_version("opus-4-6", true), None);
    }

    #[test]
    fn family_first_digit_boundaries_reject_major_but_ignore_three_digit_minor() {
        assert_eq!(
            parse_anthropic_model_version("claude-opus-456", false),
            None
        );
        assert_eq!(
            parse_anthropic_model_version("claude-opus-4-678", false),
            Some(AnthropicModelVersion {
                family: AnthropicModelFamily::Opus,
                major: 4,
                minor: None,
            })
        );
    }

    #[test]
    fn known_profile_matrix_matches_budget_and_adaptive_generations() {
        let budget = match_known_anthropic_model_profile("claude-sonnet-4-5").unwrap();
        assert_eq!(budget.mode, AnthropicThinkingMode::Budget);
        assert_eq!(budget.efforts, BUDGET_THINKING_EFFORTS);
        assert!(!budget.supports_effort_param);

        let opus_45 = match_known_anthropic_model_profile("claude-opus-4-5").unwrap();
        assert_eq!(opus_45.mode, AnthropicThinkingMode::Budget);
        assert!(opus_45.supports_effort_param);

        let adaptive = match_known_anthropic_model_profile("claude-opus-4-6").unwrap();
        assert_eq!(adaptive.efforts, ADAPTIVE_MAX_EFFORTS);
        assert!(adaptive.can_disable_thinking);

        assert_eq!(
            match_known_anthropic_model_profile("claude-opus-4-8"),
            Some(LATEST_OPUS_PROFILE)
        );
        assert_eq!(
            match_known_anthropic_model_profile("claude-sonnet-5-0"),
            Some(LATEST_OPUS_PROFILE)
        );
        assert_eq!(
            match_known_anthropic_model_profile("claude-haiku-4-6"),
            None
        );
    }

    #[test]
    fn future_always_thinking_families_and_preview_are_non_disableable() {
        let fable = match_known_anthropic_model_profile("claude-fable-5").unwrap();
        assert!(!fable.can_disable_thinking);
        assert_eq!(fable.efforts, LATEST_OPUS_THINKING_EFFORTS);

        let preview = match_known_anthropic_model_profile("vendor-mythos_preview-custom").unwrap();
        assert!(!preview.can_disable_thinking);
        assert_eq!(preview.efforts, ADAPTIVE_MAX_EFFORTS);
    }

    #[test]
    fn inference_defaults_unknown_models_to_latest_opus_profile() {
        assert_eq!(
            infer_anthropic_model_profile("vendor-custom"),
            LATEST_OPUS_PROFILE
        );
    }
}
