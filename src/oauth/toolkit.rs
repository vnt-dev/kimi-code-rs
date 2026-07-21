use std::{error::Error, fmt};

use super::managed_auth::KIMI_CODE_OAUTH_KEY;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidKimiOAuthTokenKey {
    key: String,
}

impl InvalidKimiOAuthTokenKey {
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl fmt::Display for InvalidKimiOAuthTokenKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Invalid Kimi OAuth token key: \"{}\".", self.key)
    }
}

impl Error for InvalidKimiOAuthTokenKey {}

// Original:
//   packages/oauth/src/toolkit.ts
//   resolveKimiTokenStorageName()
pub fn resolve_kimi_token_storage_name(
    _provider_name: Option<&str>,
    oauth_key: Option<&str>,
) -> Result<String, InvalidKimiOAuthTokenKey> {
    let key = oauth_key.unwrap_or(KIMI_CODE_OAUTH_KEY);
    if key == "kimi-code" || key == KIMI_CODE_OAUTH_KEY {
        return Ok("kimi-code".to_owned());
    }

    if let Some(name) = key.strip_prefix("oauth/")
        && !name.is_empty()
    {
        return Ok(name.to_owned());
    }
    if !key.contains('/') && !key.starts_with('.') {
        return Ok(key.to_owned());
    }
    Err(InvalidKimiOAuthTokenKey {
        key: key.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_default_config_keys_to_the_legacy_storage_name() {
        for (provider, key) in [
            (Some("managed:kimi-code"), Some("oauth/kimi-code")),
            (None, Some("kimi-code")),
            (Some("custom"), None),
        ] {
            assert_eq!(
                resolve_kimi_token_storage_name(provider, key).expect("storage name"),
                "kimi-code"
            );
        }
    }

    #[test]
    fn provider_name_does_not_change_explicit_key_mapping() {
        for provider in ["custom", "kimi-code-anthropic", "managed:kimi-code"] {
            assert_eq!(
                resolve_kimi_token_storage_name(Some(provider), Some("oauth/shared-slot"))
                    .expect("storage name"),
                "shared-slot"
            );
        }
        assert_eq!(
            resolve_kimi_token_storage_name(Some("custom"), Some("custom-slot"))
                .expect("storage name"),
            "custom-slot"
        );
    }

    #[test]
    fn preserves_javascript_prefix_behavior_and_rejects_other_path_keys() {
        assert_eq!(
            resolve_kimi_token_storage_name(None, Some("oauth/nested/name"))
                .expect("prefix is stripped exactly like JavaScript"),
            "nested/name"
        );
        for key in ["../kimi-code", ".hidden", "nested/name"] {
            let error = resolve_kimi_token_storage_name(None, Some(key))
                .expect_err("unsafe unprefixed key");
            assert_eq!(error.key(), key);
            assert_eq!(
                error.to_string(),
                format!("Invalid Kimi OAuth token key: \"{key}\".")
            );
        }
    }

    #[test]
    fn keeps_the_original_empty_key_edge_case() {
        assert_eq!(
            resolve_kimi_token_storage_name(None, Some("")).expect("empty key is accepted here"),
            ""
        );
    }
}
