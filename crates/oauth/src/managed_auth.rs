use std::collections::HashMap;

use sha2::{Digest, Sha256};

use super::{
    constants::DEFAULT_KIMI_CODE_OAUTH_HOST,
    managed_usage::{DEFAULT_KIMI_CODE_BASE_URL, kimi_code_base_url},
};

pub const KIMI_CODE_PLATFORM_ID: &str = "kimi-code";
pub const KIMI_CODE_PROVIDER_NAME: &str = "managed:kimi-code";
pub const KIMI_CODE_OAUTH_KEY: &str = "oauth/kimi-code";
const KIMI_CODE_SCOPED_OAUTH_KEY_PREFIX: &str = "oauth/kimi-code-env-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthStorageBackend {
    File,
    Keyring,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedKimiOAuthRef {
    pub storage: OAuthStorageBackend,
    pub key: String,
    pub oauth_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedKimiOAuthRefInput {
    pub storage: Option<OAuthStorageBackend>,
    pub key: Option<String>,
    pub oauth_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedKimiRuntimeAuth {
    pub base_url: Option<String>,
    pub oauth_ref: ManagedKimiOAuthRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedKimiLoginAuth {
    pub base_url: Option<String>,
    pub oauth_host: Option<String>,
    pub oauth_ref: Option<ManagedKimiOAuthRef>,
}

#[derive(Debug, Clone)]
pub struct RuntimeAuthOptions<'a> {
    pub configured_base_url: Option<&'a str>,
    pub configured_oauth_ref: Option<&'a ManagedKimiOAuthRefInput>,
    pub environment: &'a HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct LoginAuthOptions<'a> {
    pub configured_base_url: Option<&'a str>,
    pub configured_oauth_ref: Option<&'a ManagedKimiOAuthRefInput>,
    pub requested_base_url: Option<&'a str>,
    pub requested_oauth_host: Option<&'a str>,
    pub environment: &'a HashMap<String, String>,
}

// Original:
//   packages/oauth/src/managed-kimi-code.ts
//   kimiCodeEnvBaseUrl()
pub fn kimi_code_env_base_url(environment: &HashMap<String, String>) -> Option<&str> {
    environment.get("KIMI_CODE_BASE_URL").map(String::as_str)
}

// Original: kimiCodeEnvOAuthHost()
pub fn kimi_code_env_oauth_host(environment: &HashMap<String, String>) -> Option<&str> {
    environment
        .get("KIMI_CODE_OAUTH_HOST")
        .or_else(|| environment.get("KIMI_OAUTH_HOST"))
        .map(String::as_str)
}

// Original: resolveKimiCodeOAuthKey()
pub fn resolve_kimi_code_oauth_key(oauth_host: Option<&str>, base_url: Option<&str>) -> String {
    let oauth_host = normalize_endpoint(oauth_host.unwrap_or(DEFAULT_KIMI_CODE_OAUTH_HOST));
    let base_url = default_base_url(base_url);
    let default_oauth_host = normalize_endpoint(DEFAULT_KIMI_CODE_OAUTH_HOST);
    if oauth_host == default_oauth_host
        && base_url == normalize_endpoint(DEFAULT_KIMI_CODE_BASE_URL)
    {
        return KIMI_CODE_OAUTH_KEY.to_owned();
    }

    let serialized = serde_json::json!({
        "oauthHost": oauth_host,
        "baseUrl": base_url
    })
    .to_string();
    let digest = Sha256::digest(serialized.as_bytes());
    let prefix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{KIMI_CODE_SCOPED_OAUTH_KEY_PREFIX}{prefix}")
}

// Original: resolveKimiCodeOAuthRef()
pub fn resolve_kimi_code_oauth_ref(
    oauth_host: Option<&str>,
    base_url: Option<&str>,
) -> ManagedKimiOAuthRef {
    let key = resolve_kimi_code_oauth_key(oauth_host, base_url);
    managed_oauth_ref(&key, oauth_host, None)
}

// Original: resolveKimiCodeRuntimeAuth()
pub fn resolve_kimi_code_runtime_auth(options: RuntimeAuthOptions<'_>) -> ManagedKimiRuntimeAuth {
    let environment_base_url = kimi_code_env_base_url(options.environment);
    let environment_oauth_host = kimi_code_env_oauth_host(options.environment);
    let has_environment_override =
        environment_base_url.is_some() || environment_oauth_host.is_some();
    let base_url = environment_base_url
        .map(normalize_base_url)
        .or_else(|| options.configured_base_url.map(str::to_owned));
    let configured_host = options
        .configured_oauth_ref
        .and_then(|reference| reference.oauth_host.as_deref());
    let expected = resolve_kimi_code_oauth_ref(
        if has_environment_override {
            environment_oauth_host
        } else {
            configured_host
        },
        base_url.as_deref(),
    );
    let configured = options.configured_oauth_ref.and_then(configured_oauth_ref);
    let oauth_ref = match configured {
        Some(configured) if !has_environment_override && configured.key == expected.key => {
            configured
        }
        _ => expected,
    };
    ManagedKimiRuntimeAuth {
        base_url,
        oauth_ref,
    }
}

// Original: resolveKimiCodeLoginAuth()
pub fn resolve_kimi_code_login_auth(options: LoginAuthOptions<'_>) -> ManagedKimiLoginAuth {
    let environment_base_url = kimi_code_env_base_url(options.environment);
    let environment_oauth_host = kimi_code_env_oauth_host(options.environment);
    let has_override = options.requested_base_url.is_some()
        || options.requested_oauth_host.is_some()
        || environment_base_url.is_some()
        || environment_oauth_host.is_some();
    let base_url = options
        .requested_base_url
        .map(normalize_base_url)
        .or_else(|| environment_base_url.map(normalize_base_url))
        .or_else(|| options.configured_base_url.map(str::to_owned));
    let oauth_host = options
        .requested_oauth_host
        .or(environment_oauth_host)
        .map(str::to_owned);
    if has_override {
        return ManagedKimiLoginAuth {
            base_url,
            oauth_host,
            oauth_ref: None,
        };
    }

    let configured = options.configured_oauth_ref.and_then(configured_oauth_ref);
    let oauth_ref = configured.filter(|reference| {
        reference.key
            == resolve_kimi_code_oauth_key(reference.oauth_host.as_deref(), base_url.as_deref())
    });
    ManagedKimiLoginAuth {
        base_url,
        oauth_host,
        oauth_ref,
    }
}

pub(crate) fn default_base_url(base_url: Option<&str>) -> String {
    base_url
        .map_or_else(kimi_code_base_url, str::to_owned)
        .trim_end_matches('/')
        .to_owned()
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_owned()
}

fn normalize_endpoint(value: &str) -> String {
    value.trim().trim_end_matches('/').to_owned()
}

fn persisted_oauth_host(key: &str, oauth_host: Option<&str>) -> Option<String> {
    let normalized = normalize_endpoint(oauth_host.unwrap_or(DEFAULT_KIMI_CODE_OAUTH_HOST));
    (key != KIMI_CODE_OAUTH_KEY || normalized != normalize_endpoint(DEFAULT_KIMI_CODE_OAUTH_HOST))
        .then_some(normalized)
}

pub(crate) fn managed_oauth_ref(
    key: &str,
    oauth_host: Option<&str>,
    storage: Option<OAuthStorageBackend>,
) -> ManagedKimiOAuthRef {
    ManagedKimiOAuthRef {
        storage: storage.unwrap_or(OAuthStorageBackend::File),
        key: key.to_owned(),
        oauth_host: persisted_oauth_host(key, oauth_host),
    }
}

fn configured_oauth_ref(input: &ManagedKimiOAuthRefInput) -> Option<ManagedKimiOAuthRef> {
    let key = input.key.as_deref()?;
    Some(managed_oauth_ref(
        key,
        input.oauth_host.as_deref(),
        input.storage,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(reference: &ManagedKimiOAuthRef) -> ManagedKimiOAuthRefInput {
        ManagedKimiOAuthRefInput {
            storage: Some(reference.storage),
            key: Some(reference.key.clone()),
            oauth_host: reference.oauth_host.clone(),
        }
    }

    #[test]
    fn default_environment_keeps_the_legacy_key_and_omits_host() {
        assert_eq!(
            resolve_kimi_code_oauth_key(
                Some("https://auth.kimi.com/"),
                Some("https://api.kimi.com/coding/v1/")
            ),
            KIMI_CODE_OAUTH_KEY
        );
        assert_eq!(
            resolve_kimi_code_oauth_ref(
                Some("https://auth.kimi.com/"),
                Some("https://api.kimi.com/coding/v1/")
            ),
            ManagedKimiOAuthRef {
                storage: OAuthStorageBackend::File,
                key: KIMI_CODE_OAUTH_KEY.to_owned(),
                oauth_host: None
            }
        );
    }

    #[test]
    fn non_default_environments_get_stable_scoped_keys_and_persisted_hosts() {
        let key = resolve_kimi_code_oauth_key(
            Some(" https://auth.dev.example.test/ "),
            Some("https://api.dev.example.test/coding/v1/"),
        );
        assert!(key.starts_with(KIMI_CODE_SCOPED_OAUTH_KEY_PREFIX));
        assert_eq!(key.len(), KIMI_CODE_SCOPED_OAUTH_KEY_PREFIX.len() + 16);
        assert_eq!(key, "oauth/kimi-code-env-51d35a57390d1c7e");
        assert_eq!(
            key,
            resolve_kimi_code_oauth_key(
                Some("https://auth.dev.example.test"),
                Some("https://api.dev.example.test/coding/v1")
            )
        );
        let custom_api =
            resolve_kimi_code_oauth_ref(None, Some("https://api.example.test/coding/v1"));
        assert_eq!(
            custom_api.oauth_host.as_deref(),
            Some(DEFAULT_KIMI_CODE_OAUTH_HOST)
        );
    }

    #[test]
    fn runtime_environment_overrides_config_and_matching_keyring_refs_survive() {
        let configured_base = "https://api.configured.example/coding/v1";
        let configured = resolve_kimi_code_oauth_ref(None, Some(configured_base));
        let environment = HashMap::from([
            (
                "KIMI_CODE_BASE_URL".to_owned(),
                "https://api.env.example/coding/v1/".to_owned(),
            ),
            (
                "KIMI_CODE_OAUTH_HOST".to_owned(),
                "https://auth.env.example/".to_owned(),
            ),
        ]);
        let auth = resolve_kimi_code_runtime_auth(RuntimeAuthOptions {
            configured_base_url: Some(configured_base),
            configured_oauth_ref: Some(&input(&configured)),
            environment: &environment,
        });
        assert_eq!(
            auth.base_url.as_deref(),
            Some("https://api.env.example/coding/v1")
        );
        assert_eq!(
            auth.oauth_ref.oauth_host.as_deref(),
            Some("https://auth.env.example")
        );

        let base = "https://api.dev.example/coding/v1";
        let mut keyring = resolve_kimi_code_oauth_ref(Some("https://auth.dev.example"), Some(base));
        keyring.storage = OAuthStorageBackend::Keyring;
        let keyring_input = input(&keyring);
        let empty = HashMap::new();
        assert_eq!(
            resolve_kimi_code_runtime_auth(RuntimeAuthOptions {
                configured_base_url: Some(base),
                configured_oauth_ref: Some(&keyring_input),
                environment: &empty,
            })
            .oauth_ref,
            keyring
        );
    }

    #[test]
    fn login_reuses_matching_config_only_when_no_override_is_present() {
        let base = "https://api.configured.example/coding/v1";
        let configured = resolve_kimi_code_oauth_ref(None, Some(base));
        let configured_input = input(&configured);
        let empty = HashMap::new();
        assert_eq!(
            resolve_kimi_code_login_auth(LoginAuthOptions {
                configured_base_url: Some(base),
                configured_oauth_ref: Some(&configured_input),
                requested_base_url: None,
                requested_oauth_host: None,
                environment: &empty,
            }),
            ManagedKimiLoginAuth {
                base_url: Some(base.to_owned()),
                oauth_host: None,
                oauth_ref: Some(configured)
            }
        );

        let overridden = resolve_kimi_code_login_auth(LoginAuthOptions {
            configured_base_url: Some(base),
            configured_oauth_ref: Some(&configured_input),
            requested_base_url: Some("https://api.requested.example/v1/"),
            requested_oauth_host: None,
            environment: &empty,
        });
        assert_eq!(
            overridden.base_url.as_deref(),
            Some("https://api.requested.example/v1")
        );
        assert_eq!(overridden.oauth_ref, None);
    }

    #[test]
    fn primary_oauth_environment_name_wins_even_when_empty() {
        let environment = HashMap::from([
            ("KIMI_CODE_OAUTH_HOST".to_owned(), String::new()),
            (
                "KIMI_OAUTH_HOST".to_owned(),
                "https://legacy.example".to_owned(),
            ),
        ]);
        assert_eq!(kimi_code_env_oauth_host(&environment), Some(""));
    }
}
