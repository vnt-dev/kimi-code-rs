use std::collections::HashMap;

pub const KIMI_V2_ENV: &str = "KIMI_CODE_EXPERIMENTAL_FLAG";

// Original:
//   apps/kimi-code/src/cli/experimental-v2.ts
//   isKimiV2Enabled()
pub fn is_kimi_v2_enabled(environment: &HashMap<String, String>) -> bool {
    environment.get(KIMI_V2_ENV).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{KIMI_V2_ENV, is_kimi_v2_enabled};

    #[test]
    fn recognizes_only_trimmed_case_insensitive_truthy_values() {
        for value in ["1", "true", "TRUE", " yes ", "On"] {
            let environment = HashMap::from([(KIMI_V2_ENV.to_owned(), value.to_owned())]);
            assert!(is_kimi_v2_enabled(&environment), "{value}");
        }
        for value in ["", "0", "false", "enabled"] {
            let environment = HashMap::from([(KIMI_V2_ENV.to_owned(), value.to_owned())]);
            assert!(!is_kimi_v2_enabled(&environment), "{value}");
        }
        assert!(!is_kimi_v2_enabled(&HashMap::new()));
    }
}
