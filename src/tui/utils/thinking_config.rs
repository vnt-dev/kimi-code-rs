use crate::sdk::types::ThinkingEffort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingConfig {
    pub enabled: Option<bool>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingConfigPatch {
    pub enabled: bool,
    pub effort: Option<String>,
}

// Original:
//   apps/kimi-code/src/tui/utils/thinking-config.ts
//   isThinkingOn()
pub fn is_thinking_on(effort: &ThinkingEffort) -> bool {
    effort.as_str() != "off"
}

// Original:
//   apps/kimi-code/src/tui/utils/thinking-config.ts
//   thinkingEffortToConfig()
pub fn thinking_effort_to_config(
    effort: &ThinkingEffort,
    supported_efforts: Option<&[String]>,
) -> ThinkingConfigPatch {
    match effort.as_str() {
        "off" => ThinkingConfigPatch {
            enabled: false,
            effort: None,
        },
        "on" => ThinkingConfigPatch {
            enabled: true,
            effort: None,
        },
        value
            if supported_efforts
                .and_then(|efforts| efforts.last())
                .is_some_and(|top| value == top) =>
        {
            ThinkingConfigPatch {
                enabled: true,
                effort: None,
            }
        }
        value => ThinkingConfigPatch {
            enabled: true,
            effort: Some(value.to_owned()),
        },
    }
}

// Original:
//   apps/kimi-code/src/tui/utils/thinking-config.ts
//   thinkingEffortFromConfig()
pub fn thinking_effort_from_config(config: Option<&ThinkingConfig>) -> Option<ThinkingEffort> {
    match config {
        Some(ThinkingConfig {
            enabled: Some(false),
            ..
        }) => Some(ThinkingEffort::from("off")),
        Some(config) => config.effort.as_deref().map(ThinkingEffort::from),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ThinkingConfig, ThinkingConfigPatch, is_thinking_on, thinking_effort_from_config,
        thinking_effort_to_config,
    };
    use crate::sdk::types::ThinkingEffort;

    #[test]
    fn maps_efforts_without_model_levels() {
        for (effort, expected) in [
            (
                "off",
                ThinkingConfigPatch {
                    enabled: false,
                    effort: None,
                },
            ),
            (
                "on",
                ThinkingConfigPatch {
                    enabled: true,
                    effort: None,
                },
            ),
            (
                "low",
                ThinkingConfigPatch {
                    enabled: true,
                    effort: Some("low".to_owned()),
                },
            ),
            (
                "max",
                ThinkingConfigPatch {
                    enabled: true,
                    effort: Some("max".to_owned()),
                },
            ),
        ] {
            assert_eq!(
                thinking_effort_to_config(&ThinkingEffort::from(effort), None),
                expected
            );
        }
    }

    #[test]
    fn keeps_the_highest_declared_level_session_only() {
        let levels = ["low".to_owned(), "high".to_owned(), "max".to_owned()];
        assert_eq!(
            thinking_effort_to_config(&ThinkingEffort::from("low"), Some(&levels)),
            ThinkingConfigPatch {
                enabled: true,
                effort: Some("low".to_owned()),
            }
        );
        assert_eq!(
            thinking_effort_to_config(&ThinkingEffort::from("max"), Some(&levels)),
            ThinkingConfigPatch {
                enabled: true,
                effort: None,
            }
        );
        assert_eq!(
            thinking_effort_to_config(&ThinkingEffort::from("ultra"), Some(&levels)),
            ThinkingConfigPatch {
                enabled: true,
                effort: Some("ultra".to_owned()),
            }
        );
        assert_eq!(
            thinking_effort_to_config(&ThinkingEffort::from("max"), Some(&["max".to_owned()])),
            ThinkingConfigPatch {
                enabled: true,
                effort: None,
            }
        );
    }

    #[test]
    fn detects_whether_thinking_is_on() {
        for (effort, expected) in [
            ("off", false),
            ("on", true),
            ("low", true),
            ("high", true),
            ("max", true),
        ] {
            assert_eq!(is_thinking_on(&ThinkingEffort::from(effort)), expected);
        }
    }

    #[test]
    fn derives_runtime_effort_from_config() {
        assert_eq!(thinking_effort_from_config(None), None);
        assert_eq!(
            thinking_effort_from_config(Some(&ThinkingConfig {
                enabled: None,
                effort: None,
            })),
            None
        );
        assert_eq!(
            thinking_effort_from_config(Some(&ThinkingConfig {
                enabled: Some(true),
                effort: None,
            })),
            None
        );
        assert_eq!(
            thinking_effort_from_config(Some(&ThinkingConfig {
                enabled: Some(false),
                effort: None,
            })),
            Some(ThinkingEffort::from("off"))
        );
        assert_eq!(
            thinking_effort_from_config(Some(&ThinkingConfig {
                enabled: Some(true),
                effort: Some("high".to_owned()),
            })),
            Some(ThinkingEffort::from("high"))
        );
        assert_eq!(
            thinking_effort_from_config(Some(&ThinkingConfig {
                enabled: None,
                effort: Some("max".to_owned()),
            })),
            Some(ThinkingEffort::from("max"))
        );
    }
}
