// Original:
//   apps/kimi-code/src/built-in-catalog.ts
//   BUILT_IN_CATALOG_JSON
//
// Rust adaptation:
//   The release bundler's injected string maps to a Cargo compile-time
//   environment value. Development builds intentionally leave it absent.
pub const BUILT_IN_CATALOG_JSON: Option<&'static str> = option_env!("KIMI_CODE_BUILT_IN_CATALOG");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflects_the_compile_time_catalog_without_runtime_fallbacks() {
        assert_eq!(
            BUILT_IN_CATALOG_JSON,
            option_env!("KIMI_CODE_BUILT_IN_CATALOG")
        );
    }
}
