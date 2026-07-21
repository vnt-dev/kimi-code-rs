#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KimiBuildInfo {
    pub version: Option<&'static str>,
    pub channel: Option<&'static str>,
    pub commit: Option<&'static str>,
    pub build_target: Option<&'static str>,
}

const fn optional_build_string(value: Option<&'static str>) -> Option<&'static str> {
    match value {
        Some(value) if value.is_empty() => None,
        Some(value) => Some(value),
        None => None,
    }
}

// Original:
//   apps/kimi-code/src/cli/build-info.ts
//   KIMI_BUILD_INFO
//
// Rust adaptation:
//   Bundler-injected JavaScript globals map to compile-time Cargo environment
//   variables. Empty strings retain the original "not provided" meaning.
pub const KIMI_BUILD_INFO: KimiBuildInfo = KimiBuildInfo {
    version: optional_build_string(option_env!("KIMI_CODE_VERSION")),
    channel: optional_build_string(option_env!("KIMI_CODE_CHANNEL")),
    commit: optional_build_string(option_env!("KIMI_CODE_COMMIT")),
    build_target: optional_build_string(option_env!("KIMI_CODE_BUILD_TARGET")),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_build_strings_reject_only_absent_and_empty_values() {
        assert_eq!(optional_build_string(None), None);
        assert_eq!(optional_build_string(Some("")), None);
        assert_eq!(optional_build_string(Some("nightly")), Some("nightly"));
        assert_eq!(optional_build_string(Some(" ")), Some(" "));
    }
}
