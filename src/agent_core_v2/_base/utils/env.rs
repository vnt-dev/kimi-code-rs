// Original:
//   packages/agent-core-v2/src/_base/utils/env.ts
//   parseBooleanEnv()
pub fn parse_boolean_env(value: Option<&str>) -> Option<bool> {
    let normalized = value?.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    match normalized.as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_source_true_values() {
        for value in ["1", "true", "yes", "on"] {
            assert_eq!(parse_boolean_env(Some(value)), Some(true), "{value:?}");
        }
    }

    #[test]
    fn parses_source_false_values() {
        for value in ["0", "false", "no", "off"] {
            assert_eq!(parse_boolean_env(Some(value)), Some(false), "{value:?}");
        }
    }

    #[test]
    fn ignores_ascii_case_and_surrounding_whitespace() {
        assert_eq!(parse_boolean_env(Some("  TRUE  ")), Some(true));
        assert_eq!(parse_boolean_env(Some("\tOff\n")), Some(false));
    }

    #[test]
    fn empty_or_missing_input_is_absent() {
        for value in [None, Some(""), Some("   ")] {
            assert_eq!(parse_boolean_env(value), None);
        }
    }

    #[test]
    fn unparseable_values_are_absent() {
        for value in ["flase", "maybe", "2", "true false"] {
            assert_eq!(parse_boolean_env(Some(value)), None, "{value:?}");
        }
    }
}
