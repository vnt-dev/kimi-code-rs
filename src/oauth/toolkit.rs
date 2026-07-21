use std::{
    collections::HashMap,
    error::Error,
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use super::{
    constants::kimi_code_flow_config,
    identity::{
        IdentityError, KimiHostIdentity, assert_kimi_host_identity, create_kimi_device_headers,
    },
    managed_auth::{KIMI_CODE_OAUTH_KEY, KIMI_CODE_PROVIDER_NAME, resolve_kimi_code_oauth_key},
    manager::{
        OAuthManager, OAuthManagerError, OAuthManagerRuntime, OAuthRefreshOutcome,
        SystemOAuthManagerRuntime,
    },
    storage::{FileTokenStorage, TokenStorage},
    types::OAuthFlowConfig,
};
use crate::utils::paths::{HomeDirectoryUnavailable, get_data_dir};

type RefreshThreshold = dyn Fn(f64) -> f64 + Send + Sync;
type RefreshObserver = dyn Fn(OAuthRefreshOutcome) + Send + Sync;

#[derive(Clone, Default)]
pub struct KimiOAuthToolkitOptions {
    pub identity: Option<KimiHostIdentity>,
    pub home_dir: Option<PathBuf>,
    pub credentials_dir: Option<PathBuf>,
    pub storage: Option<Arc<dyn TokenStorage>>,
    pub flow_config: Option<OAuthFlowConfig>,
    pub runtime: Option<Arc<dyn OAuthManagerRuntime>>,
    pub device_code_timeout: Option<Duration>,
    pub refresh_threshold: Option<Arc<RefreshThreshold>>,
    pub on_refresh: Option<Arc<RefreshObserver>>,
}

#[derive(Debug)]
pub enum KimiOAuthToolkitError {
    InvalidTokenKey(InvalidKimiOAuthTokenKey),
    Identity(IdentityError),
    Home(HomeDirectoryUnavailable),
    Manager(OAuthManagerError),
}

impl fmt::Display for KimiOAuthToolkitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTokenKey(error) => error.fmt(formatter),
            Self::Identity(error) => error.fmt(formatter),
            Self::Home(error) => error.fmt(formatter),
            Self::Manager(error) => error.fmt(formatter),
        }
    }
}

impl Error for KimiOAuthToolkitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidTokenKey(error) => Some(error),
            Self::Identity(error) => Some(error),
            Self::Home(error) => Some(error),
            Self::Manager(error) => Some(error),
        }
    }
}

impl From<InvalidKimiOAuthTokenKey> for KimiOAuthToolkitError {
    fn from(error: InvalidKimiOAuthTokenKey) -> Self {
        Self::InvalidTokenKey(error)
    }
}

impl From<IdentityError> for KimiOAuthToolkitError {
    fn from(error: IdentityError) -> Self {
        Self::Identity(error)
    }
}

impl From<HomeDirectoryUnavailable> for KimiOAuthToolkitError {
    fn from(error: HomeDirectoryUnavailable) -> Self {
        Self::Home(error)
    }
}

impl From<OAuthManagerError> for KimiOAuthToolkitError {
    fn from(error: OAuthManagerError) -> Self {
        Self::Manager(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KimiOAuthTokenRef {
    pub key: Option<String>,
    pub oauth_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthProviderStatus {
    pub provider_name: String,
    pub has_token: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub providers: Vec<AuthProviderStatus>,
}

#[derive(Clone)]
pub struct BearerTokenProvider {
    manager: Arc<OAuthManager>,
}

impl BearerTokenProvider {
    pub async fn get_access_token(&self, force: bool) -> Result<String, OAuthManagerError> {
        self.manager.ensure_fresh(force).await
    }
}

pub struct KimiOAuthToolkit {
    home_dir: PathBuf,
    _identity: Option<KimiHostIdentity>,
    storage: Arc<dyn TokenStorage>,
    flow_config: OAuthFlowConfig,
    runtime: Arc<dyn OAuthManagerRuntime>,
    device_code_timeout: Option<Duration>,
    refresh_threshold: Option<Arc<RefreshThreshold>>,
    on_refresh: Option<Arc<RefreshObserver>>,
    managers: Mutex<HashMap<String, Arc<OAuthManager>>>,
}

impl KimiOAuthToolkit {
    // Original:
    //   packages/oauth/src/toolkit.ts
    //   KimiOAuthToolkit.constructor()
    pub fn new(options: KimiOAuthToolkitOptions) -> Result<Self, KimiOAuthToolkitError> {
        if let Some(identity) = options.identity.as_ref() {
            assert_kimi_host_identity(Some(identity))?;
        }
        let home_dir = options.home_dir.map_or_else(get_data_dir, Ok)?;
        let credentials_dir = options
            .credentials_dir
            .unwrap_or_else(|| home_dir.join("credentials"));
        let storage = options
            .storage
            .unwrap_or_else(|| Arc::new(FileTokenStorage::new(credentials_dir)));
        let flow_config = options.flow_config.unwrap_or_else(kimi_code_flow_config);
        let identity = options.identity;
        let runtime = options.runtime.unwrap_or_else(|| {
            let device_headers = identity.clone().map(|identity| {
                let home_dir = home_dir.clone();
                Arc::new(move || create_kimi_device_headers(&home_dir, &identity.version).ok())
                    as Arc<dyn Fn() -> Option<indexmap::IndexMap<String, String>> + Send + Sync>
            });
            Arc::new(SystemOAuthManagerRuntime::new(device_headers))
        });
        Ok(Self {
            home_dir,
            _identity: identity,
            storage,
            flow_config,
            runtime,
            device_code_timeout: options.device_code_timeout,
            refresh_threshold: options.refresh_threshold,
            on_refresh: options.on_refresh,
            managers: Mutex::new(HashMap::new()),
        })
    }

    // Original: KimiOAuthToolkit.status()
    pub async fn status(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<AuthStatus, KimiOAuthToolkitError> {
        let name = provider_name.unwrap_or(KIMI_CODE_PROVIDER_NAME);
        let oauth_host = self.oauth_host_for(oauth_ref, None);
        let oauth_key = oauth_ref
            .and_then(|reference| reference.key.as_deref())
            .map_or_else(|| self.default_oauth_key(None, &oauth_host), str::to_owned);
        let has_token = self
            .manager_for(name, Some(&oauth_key), Some(&oauth_host))?
            .has_token()
            .await?;
        Ok(AuthStatus {
            providers: vec![AuthProviderStatus {
                provider_name: name.to_owned(),
                has_token,
            }],
        })
    }

    // Original: KimiOAuthToolkit.ensureFresh()
    pub async fn ensure_fresh(
        &self,
        provider_name: Option<&str>,
        force: bool,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<String, KimiOAuthToolkitError> {
        let name = provider_name.unwrap_or(KIMI_CODE_PROVIDER_NAME);
        let oauth_host = self.oauth_host_for(oauth_ref, None);
        let oauth_key = oauth_ref
            .and_then(|reference| reference.key.as_deref())
            .map_or_else(|| self.default_oauth_key(None, &oauth_host), str::to_owned);
        Ok(self
            .manager_for(name, Some(&oauth_key), Some(&oauth_host))?
            .ensure_fresh(force)
            .await?)
    }

    // Original: KimiOAuthToolkit.getCachedAccessToken()
    pub async fn get_cached_access_token(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<Option<String>, KimiOAuthToolkitError> {
        let name = provider_name.unwrap_or(KIMI_CODE_PROVIDER_NAME);
        let oauth_host = self.oauth_host_for(oauth_ref, None);
        let oauth_key = oauth_ref
            .and_then(|reference| reference.key.as_deref())
            .map_or_else(|| self.default_oauth_key(None, &oauth_host), str::to_owned);
        Ok(self
            .manager_for(name, Some(&oauth_key), Some(&oauth_host))?
            .get_cached_access_token()
            .await?)
    }

    // Original: KimiOAuthToolkit.tokenProvider()
    pub fn token_provider(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<BearerTokenProvider, KimiOAuthToolkitError> {
        let name = provider_name.unwrap_or(KIMI_CODE_PROVIDER_NAME);
        let oauth_host = self.oauth_host_for(oauth_ref, None);
        let oauth_key = oauth_ref
            .and_then(|reference| reference.key.as_deref())
            .map_or_else(|| self.default_oauth_key(None, &oauth_host), str::to_owned);
        Ok(BearerTokenProvider {
            manager: self.manager_for(name, Some(&oauth_key), Some(&oauth_host))?,
        })
    }

    // Original: KimiOAuthToolkit.managerFor()
    pub fn manager_for(
        &self,
        provider_name: &str,
        oauth_key: Option<&str>,
        oauth_host: Option<&str>,
    ) -> Result<Arc<OAuthManager>, KimiOAuthToolkitError> {
        let storage_name = resolve_kimi_token_storage_name(provider_name.into(), oauth_key)?;
        let effective_oauth_host = oauth_host.unwrap_or(&self.flow_config.oauth_host);
        let manager_key = format!(
            "{storage_name}\0{}",
            normalize_oauth_host(effective_oauth_host)
        );
        let mut managers = self
            .managers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(manager) = managers.get(&manager_key) {
            return Ok(Arc::clone(manager));
        }

        let mut config = self.flow_config.clone();
        config.oauth_host = effective_oauth_host.to_owned();
        config.name = storage_name;
        let mut manager =
            OAuthManager::new(config, Arc::clone(&self.storage), Arc::clone(&self.runtime))
                .with_config_dir(&self.home_dir);
        if let Some(timeout) = self.device_code_timeout {
            manager = manager.with_device_code_timeout(timeout);
        }
        if let Some(threshold) = &self.refresh_threshold {
            manager = manager.with_refresh_threshold(Arc::clone(threshold));
        }
        if let Some(observer) = &self.on_refresh {
            manager = manager.with_refresh_observer(Arc::clone(observer));
        }
        let manager = Arc::new(manager);
        managers.insert(manager_key, Arc::clone(&manager));
        Ok(manager)
    }

    fn default_oauth_key(&self, base_url: Option<&str>, oauth_host: &str) -> String {
        resolve_kimi_code_oauth_key(Some(oauth_host), base_url)
    }

    fn oauth_host_for(
        &self,
        oauth_ref: Option<&KimiOAuthTokenRef>,
        oauth_host: Option<&str>,
    ) -> String {
        oauth_ref
            .and_then(|reference| reference.oauth_host.clone())
            .or_else(|| oauth_host.map(str::to_owned))
            .unwrap_or_else(|| self.flow_config.oauth_host.clone())
    }
}

fn normalize_oauth_host(oauth_host: &str) -> String {
    oauth_host.trim().trim_end_matches('/').to_owned()
}

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
    use async_trait::async_trait;

    use super::*;
    use crate::oauth::{
        errors::OAuthError,
        flow::DevicePollResult,
        storage::TokenStorageError,
        types::{DeviceAuthorization, TokenInfo},
    };

    #[derive(Default)]
    struct MemoryTokenStorage {
        tokens: Mutex<HashMap<String, TokenInfo>>,
    }

    #[async_trait]
    impl TokenStorage for MemoryTokenStorage {
        async fn load(&self, name: &str) -> Result<Option<TokenInfo>, TokenStorageError> {
            Ok(self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(name)
                .cloned())
        }

        async fn save(&self, name: &str, token: &TokenInfo) -> Result<(), TokenStorageError> {
            self.tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(name.to_owned(), token.clone());
            Ok(())
        }

        async fn remove(&self, name: &str) -> Result<(), TokenStorageError> {
            self.tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(name);
            Ok(())
        }

        async fn list(&self) -> Result<Vec<String>, TokenStorageError> {
            Ok(self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .keys()
                .cloned()
                .collect())
        }
    }

    struct StaticRuntime;

    #[async_trait]
    impl OAuthManagerRuntime for StaticRuntime {
        fn now_seconds(&self) -> f64 {
            100.0
        }

        async fn sleep(&self, _duration: Duration) {}

        async fn refresh_token(
            &self,
            _config: &OAuthFlowConfig,
            _refresh_token: &str,
        ) -> Result<TokenInfo, OAuthError> {
            Err(OAuthError::new("unexpected refresh"))
        }

        async fn request_device(
            &self,
            _config: &OAuthFlowConfig,
        ) -> Result<DeviceAuthorization, OAuthError> {
            Err(OAuthError::new("unexpected device request"))
        }

        async fn poll_device(
            &self,
            _config: &OAuthFlowConfig,
            _device_code: &str,
        ) -> Result<DevicePollResult, OAuthError> {
            Err(OAuthError::new("unexpected device poll"))
        }
    }

    fn token(access_token: &str) -> TokenInfo {
        TokenInfo {
            access_token: access_token.to_owned(),
            refresh_token: format!("refresh-{access_token}"),
            expires_at: 10_000.0,
            scope: String::new(),
            token_type: "Bearer".to_owned(),
            expires_in: 3_600.0,
        }
    }

    fn toolkit_options(storage: Arc<dyn TokenStorage>) -> KimiOAuthToolkitOptions {
        KimiOAuthToolkitOptions {
            home_dir: Some(PathBuf::from("C:/tmp/kimi-oauth-toolkit-test")),
            storage: Some(storage),
            runtime: Some(Arc::new(StaticRuntime)),
            ..KimiOAuthToolkitOptions::default()
        }
    }

    #[tokio::test]
    async fn reports_status_cached_token_and_bearer_token_without_identity() {
        let storage = Arc::new(MemoryTokenStorage::default());
        storage
            .save("kimi-code", &token("access-1"))
            .await
            .expect("save token");
        let toolkit = KimiOAuthToolkit::new(toolkit_options(storage)).expect("toolkit");

        assert_eq!(
            toolkit.status(None, None).await.expect("status"),
            AuthStatus {
                providers: vec![AuthProviderStatus {
                    provider_name: "managed:kimi-code".to_owned(),
                    has_token: true,
                }]
            }
        );
        assert_eq!(
            toolkit
                .get_cached_access_token(None, None)
                .await
                .expect("cached token"),
            Some("access-1".to_owned())
        );
        assert_eq!(
            toolkit
                .token_provider(None, None)
                .expect("provider")
                .get_access_token(false)
                .await
                .expect("access token"),
            "access-1"
        );
    }

    #[tokio::test]
    async fn resolves_explicit_oauth_refs_to_their_storage_slot() {
        let storage = Arc::new(MemoryTokenStorage::default());
        storage
            .save("custom-slot", &token("custom-access"))
            .await
            .expect("save token");
        let toolkit = KimiOAuthToolkit::new(toolkit_options(storage)).expect("toolkit");
        let reference = KimiOAuthTokenRef {
            key: Some("oauth/custom-slot".to_owned()),
            oauth_host: Some("https://auth.dev.example".to_owned()),
        };

        assert_eq!(
            toolkit
                .get_cached_access_token(Some("custom"), Some(&reference))
                .await
                .expect("cached token"),
            Some("custom-access".to_owned())
        );
        assert_eq!(
            toolkit
                .ensure_fresh(Some("custom"), false, Some(&reference))
                .await
                .expect("fresh token"),
            "custom-access"
        );
    }

    #[test]
    fn caches_managers_by_storage_name_and_normalized_oauth_host() {
        let toolkit =
            KimiOAuthToolkit::new(toolkit_options(Arc::new(MemoryTokenStorage::default())))
                .expect("toolkit");
        let first = toolkit
            .manager_for(
                "managed:kimi-code",
                Some("oauth/kimi-code"),
                Some("https://auth.example/"),
            )
            .expect("first manager");
        let normalized = toolkit
            .manager_for(
                "other-name",
                Some("kimi-code"),
                Some(" https://auth.example "),
            )
            .expect("normalized manager");
        let other_host = toolkit
            .manager_for(
                "managed:kimi-code",
                Some("oauth/kimi-code"),
                Some("https://auth.other.example"),
            )
            .expect("other manager");

        assert!(Arc::ptr_eq(&first, &normalized));
        assert!(!Arc::ptr_eq(&first, &other_host));
    }

    #[test]
    fn manager_resolution_propagates_invalid_storage_keys() {
        let toolkit =
            KimiOAuthToolkit::new(toolkit_options(Arc::new(MemoryTokenStorage::default())))
                .expect("toolkit");
        let error = toolkit
            .manager_for("custom", Some("../unsafe"), None)
            .err()
            .expect("invalid key");
        assert!(matches!(error, KimiOAuthToolkitError::InvalidTokenKey(_)));
    }

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
