use url::{Position, Url};

// Original:
//   packages/agent-core-v2/src/kosong/model/modelAuth.ts
//   deriveProviderId()
//
// Rust adaptation:
//   The Position slice includes an explicit non-default port, matching the
//   JavaScript URL.host property. Url::host_str() alone would incorrectly
//   omit that port.
pub fn derive_provider_id(base_url: &str) -> String {
    match Url::parse(base_url) {
        Ok(url) => url[Position::BeforeHost..Position::AfterPort].to_owned(),
        Err(_) => base_url.to_owned(),
    }
}

// Original:
//   packages/agent-core-v2/src/kosong/model/modelAuth.ts
//   nonEmpty()
pub fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

// MIGRATION-TODO:
// Original: packages/agent-core-v2/src/kosong/model/modelAuth.ts
// Missing units: resolveModelAuthMaterial(), effectiveModelConfig(), and their
// Anthropic profile fold.
// Temporary behavior: only the independent URL identity and non-empty-string
// helpers are exported.
// Completion condition: migrate ModelRecord, ProviderConfig, OAuthRef,
// provider endpoint definitions, resolution tracing, and typed config errors.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_is_the_parsed_url_host() {
        assert_eq!(
            derive_provider_id("https://api.example.test/v1"),
            "api.example.test"
        );
        assert_eq!(
            derive_provider_id("https://api.example.test:8443/v1"),
            "api.example.test:8443"
        );
        assert_eq!(
            derive_provider_id("https://user:secret@api.example.test:8443/v1"),
            "api.example.test:8443"
        );
    }

    #[test]
    fn provider_id_preserves_unparseable_input_verbatim() {
        assert_eq!(derive_provider_id("not-a-url"), "not-a-url");
        assert_eq!(derive_provider_id(""), "");
        assert_eq!(derive_provider_id(" relative/path "), " relative/path ");
    }

    #[test]
    fn provider_id_matches_url_normalization_edge_cases() {
        assert_eq!(
            derive_provider_id("HTTPS://EXAMPLE.COM:443/path"),
            "example.com"
        );
        assert_eq!(derive_provider_id("https://[::1]:8080/v1"), "[::1]:8080");
        assert_eq!(derive_provider_id("mailto:user@example.com"), "");
    }

    #[test]
    fn non_empty_trims_and_filters_empty_values() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some("")), None);
        assert_eq!(non_empty(Some(" \t\n ")), None);
        assert_eq!(non_empty(Some("  key  ")), Some("key"));
    }
}
