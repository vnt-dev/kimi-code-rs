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
        IdentityError, KimiHostIdentity, KimiIdentityOptions, assert_kimi_host_identity,
        create_kimi_default_headers_async,
    },
    managed_auth::{KIMI_CODE_OAUTH_KEY, KIMI_CODE_PROVIDER_NAME, resolve_kimi_code_oauth_key},
    managed_config::{ManagedKimiCodeApplyOptions, ManagedKimiCodeApplyResult},
    managed_feedback::{
        FetchSubmitFeedbackResult, SubmitFeedbackBody, fetch_submit_feedback,
        kimi_code_feedback_url,
    },
    managed_feedback_upload::{
        CompleteFeedbackUploadBody, CreateFeedbackUploadUrlBody, FetchCompleteFeedbackUploadResult,
        FetchCreateFeedbackUploadUrlResult, fetch_complete_feedback_upload,
        fetch_create_feedback_upload_url,
    },
    managed_provision::{
        ManagedKimiCodeProvisionResult, ManagedKimiConfigAdapter,
        ProvisionManagedKimiCodeConfigOptions, ProvisionManagedKimiCodeError,
        provision_managed_kimi_code_config,
    },
    managed_usage::{
        BoosterWalletInfo, FetchManagedUsageResult, UsageRow, fetch_managed_usage,
        kimi_code_usage_url,
    },
    managed_userinfo::{
        FetchManagedUserInfoResult, ManagedUserInfo, fetch_managed_user_info,
        kimi_code_user_info_url,
    },
    manager::{
        DeviceCodeObserver, LoginAbortSignal, LoginOptions, OAuthManager, OAuthManagerError,
        OAuthManagerRuntime, OAuthRefreshOutcome, SystemOAuthManagerRuntime,
    },
    storage::{FileTokenStorage, TokenStorage},
    types::OAuthFlowConfig,
};
use crate::home::{HomeDirectoryUnavailable, default_kimi_home};

type RefreshThreshold = dyn Fn(f64) -> f64 + Send + Sync;
type RefreshObserver = dyn Fn(OAuthRefreshOutcome) + Send + Sync;

#[derive(Clone)]
pub struct KimiOAuthToolkitOptions<A = NoManagedConfigAdapter> {
    pub identity: Option<KimiHostIdentity>,
    pub home_dir: Option<PathBuf>,
    pub credentials_dir: Option<PathBuf>,
    pub storage: Option<Arc<dyn TokenStorage>>,
    pub flow_config: Option<OAuthFlowConfig>,
    pub runtime: Option<Arc<dyn OAuthManagerRuntime>>,
    pub device_code_timeout: Option<Duration>,
    pub refresh_threshold: Option<Arc<RefreshThreshold>>,
    pub on_refresh: Option<Arc<RefreshObserver>>,
    pub config_adapter: Option<Arc<A>>,
}

impl<A> Default for KimiOAuthToolkitOptions<A> {
    fn default() -> Self {
        Self {
            identity: None,
            home_dir: None,
            credentials_dir: None,
            storage: None,
            flow_config: None,
            runtime: None,
            device_code_timeout: None,
            refresh_threshold: None,
            on_refresh: None,
            config_adapter: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoManagedConfigAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoManagedConfigAdapterError;

impl fmt::Display for NoManagedConfigAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("managed Kimi Code configuration adapter is not available")
    }
}

impl Error for NoManagedConfigAdapterError {}

#[async_trait::async_trait]
impl ManagedKimiConfigAdapter for NoManagedConfigAdapter {
    type Config = ();
    type Error = NoManagedConfigAdapterError;

    async fn read(&self) -> Result<Self::Config, Self::Error> {
        Ok(())
    }

    async fn write(&self, _config: Self::Config) -> Result<(), Self::Error> {
        Ok(())
    }

    fn apply(
        &self,
        _config: &mut Self::Config,
        _options: ManagedKimiCodeApplyOptions<'_>,
    ) -> Result<ManagedKimiCodeApplyResult, Self::Error> {
        Err(NoManagedConfigAdapterError)
    }
}

#[derive(Debug)]
pub enum KimiOAuthToolkitError {
    InvalidTokenKey(InvalidKimiOAuthTokenKey),
    Identity(IdentityError),
    Home(HomeDirectoryUnavailable),
    Manager(OAuthManagerError),
}

#[derive(Debug)]
pub enum KimiOAuthToolkitOperationError<E> {
    Toolkit(KimiOAuthToolkitError),
    Provision(ProvisionManagedKimiCodeError<E>),
    Adapter(E),
}

impl<E: fmt::Display> fmt::Display for KimiOAuthToolkitOperationError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toolkit(error) => error.fmt(formatter),
            Self::Provision(error) => error.fmt(formatter),
            Self::Adapter(error) => error.fmt(formatter),
        }
    }
}

impl<E: Error + 'static> Error for KimiOAuthToolkitOperationError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Toolkit(error) => Some(error),
            Self::Provision(error) => Some(error),
            Self::Adapter(error) => Some(error),
        }
    }
}

impl<E> From<KimiOAuthToolkitError> for KimiOAuthToolkitOperationError<E> {
    fn from(error: KimiOAuthToolkitError) -> Self {
        Self::Toolkit(error)
    }
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

#[derive(Default)]
pub struct KimiOAuthLoginOptions<'a> {
    pub on_device_code: Option<&'a dyn DeviceCodeObserver>,
    pub signal: Option<&'a dyn LoginAbortSignal>,
    pub provision_config: Option<bool>,
    pub base_url: Option<&'a str>,
    pub oauth_ref: Option<&'a KimiOAuthTokenRef>,
    pub oauth_host: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiOAuthLoginResult {
    pub provider_name: String,
    pub ok: bool,
    pub provision: Option<ManagedKimiCodeProvisionResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiOAuthLogoutResult {
    pub provider_name: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AuthenticatedServiceOptions<'a> {
    pub oauth_ref: Option<&'a KimiOAuthTokenRef>,
    pub base_url: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthManagedUsageResult {
    Ok {
        summary: Option<UsageRow>,
        limits: Vec<UsageRow>,
        extra_usage: Option<BoosterWalletInfo>,
    },
    Error {
        status: Option<u16>,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthManagedUserInfoResult {
    Ok {
        user_info: Box<ManagedUserInfo>,
    },
    Error {
        status: Option<u16>,
        message: String,
    },
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

pub struct KimiOAuthToolkit<A = NoManagedConfigAdapter> {
    home_dir: PathBuf,
    identity: Option<KimiHostIdentity>,
    storage: Arc<dyn TokenStorage>,
    flow_config: OAuthFlowConfig,
    runtime: Arc<dyn OAuthManagerRuntime>,
    device_code_timeout: Option<Duration>,
    refresh_threshold: Option<Arc<RefreshThreshold>>,
    on_refresh: Option<Arc<RefreshObserver>>,
    managers: Mutex<HashMap<String, Arc<OAuthManager>>>,
    identity_headers: tokio::sync::OnceCell<indexmap::IndexMap<String, String>>,
    config_adapter: Option<Arc<A>>,
}

impl<A> KimiOAuthToolkit<A> {
    // Original:
    //   packages/oauth/src/toolkit.ts
    //   KimiOAuthToolkit.constructor()
    pub fn new(options: KimiOAuthToolkitOptions<A>) -> Result<Self, KimiOAuthToolkitError> {
        if let Some(identity) = options.identity.as_ref() {
            assert_kimi_host_identity(Some(identity))?;
        }
        let home_dir = options.home_dir.map_or_else(default_kimi_home, Ok)?;
        let credentials_dir = options
            .credentials_dir
            .unwrap_or_else(|| home_dir.join("credentials"));
        let storage = options
            .storage
            .unwrap_or_else(|| Arc::new(FileTokenStorage::new(credentials_dir)));
        let flow_config = options.flow_config.unwrap_or_else(kimi_code_flow_config);
        let identity = options.identity;
        let runtime = options.runtime.unwrap_or_else(|| {
            identity.as_ref().map_or_else(
                || Arc::new(SystemOAuthManagerRuntime::default()),
                |identity| {
                    Arc::new(SystemOAuthManagerRuntime::with_device_identity(
                        home_dir.clone(),
                        identity.version.clone(),
                    ))
                },
            )
        });
        Ok(Self {
            home_dir,
            identity,
            storage,
            flow_config,
            runtime,
            device_code_timeout: options.device_code_timeout,
            refresh_threshold: options.refresh_threshold,
            on_refresh: options.on_refresh,
            managers: Mutex::new(HashMap::new()),
            identity_headers: tokio::sync::OnceCell::new(),
            config_adapter: options.config_adapter,
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

    // Original: KimiOAuthToolkit.getManagedUsage()
    pub async fn get_managed_usage(
        &self,
        provider_name: Option<&str>,
        options: AuthenticatedServiceOptions<'_>,
    ) -> AuthManagedUsageResult {
        let access_token = match self.service_access_token(provider_name, &options).await {
            Ok(token) => token,
            Err(message) => {
                return AuthManagedUsageResult::Error {
                    status: None,
                    message,
                };
            }
        };
        let url = managed_usage_url(options.base_url);
        match fetch_managed_usage(&url, &access_token, None).await {
            FetchManagedUsageResult::Ok { parsed } => AuthManagedUsageResult::Ok {
                summary: parsed.summary,
                limits: parsed.limits,
                extra_usage: parsed.extra_usage,
            },
            FetchManagedUsageResult::Error { status, message } => {
                AuthManagedUsageResult::Error { status, message }
            }
        }
    }

    // Original: KimiOAuthToolkit.getManagedUserInfo()
    pub async fn get_managed_user_info(
        &self,
        provider_name: Option<&str>,
        options: AuthenticatedServiceOptions<'_>,
    ) -> AuthManagedUserInfoResult {
        let access_token = match self.service_access_token(provider_name, &options).await {
            Ok(token) => token,
            Err(message) => {
                return AuthManagedUserInfoResult::Error {
                    status: None,
                    message,
                };
            }
        };
        let url = managed_user_info_url(options.base_url);
        match fetch_managed_user_info(&url, &access_token, None).await {
            FetchManagedUserInfoResult::Ok { user_info } => {
                AuthManagedUserInfoResult::Ok { user_info }
            }
            FetchManagedUserInfoResult::Error { status, message } => {
                AuthManagedUserInfoResult::Error { status, message }
            }
        }
    }

    // Original: KimiOAuthToolkit.submitFeedback()
    pub async fn submit_feedback(
        &self,
        body: &SubmitFeedbackBody,
        provider_name: Option<&str>,
        options: AuthenticatedServiceOptions<'_>,
    ) -> FetchSubmitFeedbackResult {
        let access_token = match self.service_access_token(provider_name, &options).await {
            Ok(token) => token,
            Err(message) => {
                return FetchSubmitFeedbackResult::Error {
                    status: None,
                    message,
                };
            }
        };
        fetch_submit_feedback(
            &kimi_code_feedback_url(options.base_url),
            &access_token,
            body,
            None,
        )
        .await
    }

    // Original: KimiOAuthToolkit.createFeedbackUploadUrl()
    pub async fn create_feedback_upload_url(
        &self,
        body: &CreateFeedbackUploadUrlBody,
        provider_name: Option<&str>,
        options: AuthenticatedServiceOptions<'_>,
    ) -> FetchCreateFeedbackUploadUrlResult {
        let access_token = match self.service_access_token(provider_name, &options).await {
            Ok(token) => token,
            Err(message) => {
                return FetchCreateFeedbackUploadUrlResult::Error {
                    status: None,
                    message,
                };
            }
        };
        fetch_create_feedback_upload_url(&access_token, body, None, options.base_url).await
    }

    // Original: KimiOAuthToolkit.completeFeedbackUpload()
    pub async fn complete_feedback_upload(
        &self,
        body: &CompleteFeedbackUploadBody,
        provider_name: Option<&str>,
        options: AuthenticatedServiceOptions<'_>,
    ) -> FetchCompleteFeedbackUploadResult {
        let access_token = match self.service_access_token(provider_name, &options).await {
            Ok(token) => token,
            Err(message) => {
                return FetchCompleteFeedbackUploadResult::Error {
                    status: None,
                    message,
                };
            }
        };
        fetch_complete_feedback_upload(&access_token, body, None, options.base_url).await
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

    fn default_oauth_ref(&self, base_url: Option<&str>) -> KimiOAuthTokenRef {
        KimiOAuthTokenRef {
            key: Some(self.default_oauth_key(base_url, &self.flow_config.oauth_host)),
            oauth_host: Some(self.flow_config.oauth_host.clone()),
        }
    }

    async fn service_access_token(
        &self,
        provider_name: Option<&str>,
        options: &AuthenticatedServiceOptions<'_>,
    ) -> Result<String, String> {
        let default_reference;
        let oauth_ref = match options.oauth_ref {
            Some(reference) => reference,
            None => {
                default_reference = self.default_oauth_ref(options.base_url);
                &default_reference
            }
        };
        self.ensure_fresh(provider_name, false, Some(oauth_ref))
            .await
            .map_err(|error| error.to_string())
    }

    async fn identity_headers(
        &self,
    ) -> Result<Option<indexmap::IndexMap<String, String>>, KimiOAuthToolkitError> {
        let Some(identity) = &self.identity else {
            return Ok(None);
        };
        let headers = self
            .identity_headers
            .get_or_try_init(|| async {
                create_kimi_default_headers_async(&KimiIdentityOptions {
                    home_dir: self.home_dir.clone(),
                    host: identity.clone(),
                })
                .await
                .map_err(KimiOAuthToolkitError::from)
            })
            .await?;
        Ok(Some(headers.clone()))
    }
}

impl<A> KimiOAuthToolkit<A>
where
    A: ManagedKimiConfigAdapter,
{
    // Original:
    //   packages/oauth/src/toolkit.ts
    //   KimiOAuthToolkit.login()
    pub async fn login(
        &self,
        provider_name: Option<&str>,
        options: KimiOAuthLoginOptions<'_>,
    ) -> Result<KimiOAuthLoginResult, KimiOAuthToolkitOperationError<A::Error>> {
        let name = provider_name.unwrap_or(KIMI_CODE_PROVIDER_NAME);
        let oauth_host = self.oauth_host_for(options.oauth_ref, options.oauth_host);
        let oauth_key = options
            .oauth_ref
            .and_then(|reference| reference.key.as_deref())
            .map_or_else(
                || self.default_oauth_key(options.base_url, &oauth_host),
                str::to_owned,
            );
        let manager = self.manager_for(name, Some(&oauth_key), Some(&oauth_host))?;
        let had_token = manager.has_token().await.map_err(manager_operation_error)?;
        let mut used_device_login = false;
        let access_token = if had_token {
            match manager.ensure_fresh(false).await {
                Ok(token) => token,
                Err(error) if is_unauthorized_manager_error(&error) => {
                    used_device_login = true;
                    login_with_device(&manager, &options)
                        .await
                        .map_err(manager_operation_error)?
                }
                Err(error) => return Err(manager_operation_error(error)),
            }
        } else {
            used_device_login = true;
            login_with_device(&manager, &options)
                .await
                .map_err(manager_operation_error)?
        };

        let should_provision = options
            .provision_config
            .unwrap_or(self.config_adapter.is_some());
        let provision = if should_provision {
            if let Some(adapter) = self.config_adapter.as_deref() {
                let headers = self.identity_headers().await?;
                match provision_with_token(
                    adapter,
                    &access_token,
                    &oauth_key,
                    &oauth_host,
                    had_token,
                    &options,
                    headers.as_ref(),
                )
                .await
                {
                    Ok(provision) => Some(provision),
                    Err(error)
                        if is_unauthorized_provision_error(&error)
                            && had_token
                            && !used_device_login =>
                    {
                        let retry_token = match manager.ensure_fresh(true).await {
                            Ok(token) => token,
                            Err(error) if is_unauthorized_manager_error(&error) => {
                                used_device_login = true;
                                login_with_device(&manager, &options)
                                    .await
                                    .map_err(manager_operation_error)?
                            }
                            Err(error) => return Err(manager_operation_error(error)),
                        };
                        match provision_with_token(
                            adapter,
                            &retry_token,
                            &oauth_key,
                            &oauth_host,
                            had_token,
                            &options,
                            headers.as_ref(),
                        )
                        .await
                        {
                            Ok(provision) => Some(provision),
                            Err(error)
                                if is_unauthorized_provision_error(&error)
                                    && !used_device_login =>
                            {
                                let device_token = login_with_device(&manager, &options)
                                    .await
                                    .map_err(manager_operation_error)?;
                                Some(
                                    provision_with_token(
                                        adapter,
                                        &device_token,
                                        &oauth_key,
                                        &oauth_host,
                                        had_token,
                                        &options,
                                        headers.as_ref(),
                                    )
                                    .await
                                    .map_err(KimiOAuthToolkitOperationError::Provision)?,
                                )
                            }
                            Err(error) => {
                                return Err(KimiOAuthToolkitOperationError::Provision(error));
                            }
                        }
                    }
                    Err(error) => {
                        return Err(KimiOAuthToolkitOperationError::Provision(error));
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(KimiOAuthLoginResult {
            provider_name: name.to_owned(),
            ok: true,
            provision,
        })
    }

    // Original: KimiOAuthToolkit.logout()
    pub async fn logout(
        &self,
        provider_name: Option<&str>,
        oauth_ref: Option<&KimiOAuthTokenRef>,
    ) -> Result<KimiOAuthLogoutResult, KimiOAuthToolkitOperationError<A::Error>> {
        let name = provider_name.unwrap_or(KIMI_CODE_PROVIDER_NAME);
        let oauth_host = self.oauth_host_for(oauth_ref, None);
        let oauth_key = oauth_ref
            .and_then(|reference| reference.key.as_deref())
            .map_or_else(|| self.default_oauth_key(None, &oauth_host), str::to_owned);
        self.manager_for(name, Some(&oauth_key), Some(&oauth_host))?
            .logout()
            .await
            .map_err(manager_operation_error)?;

        if name == KIMI_CODE_PROVIDER_NAME
            && let Some(adapter) = self
                .config_adapter
                .as_deref()
                .filter(|adapter| adapter.supports_remove())
        {
            let mut config = adapter
                .read()
                .await
                .map_err(KimiOAuthToolkitOperationError::Adapter)?;
            adapter
                .remove(&mut config)
                .map_err(KimiOAuthToolkitOperationError::Adapter)?;
            adapter
                .write(config)
                .await
                .map_err(KimiOAuthToolkitOperationError::Adapter)?;
        }
        Ok(KimiOAuthLogoutResult {
            provider_name: name.to_owned(),
            ok: true,
        })
    }
}

async fn provision_with_token<A: ManagedKimiConfigAdapter>(
    adapter: &A,
    access_token: &str,
    oauth_key: &str,
    oauth_host: &str,
    preserve_default_model: bool,
    login_options: &KimiOAuthLoginOptions<'_>,
    headers: Option<&indexmap::IndexMap<String, String>>,
) -> Result<ManagedKimiCodeProvisionResult, ProvisionManagedKimiCodeError<A::Error>> {
    provision_managed_kimi_code_config(ProvisionManagedKimiCodeConfigOptions {
        adapter,
        access_token,
        base_url: login_options.base_url,
        oauth_key: Some(oauth_key),
        oauth_host: Some(oauth_host),
        preserve_default_model,
        headers,
    })
    .await
}

async fn login_with_device(
    manager: &OAuthManager,
    options: &KimiOAuthLoginOptions<'_>,
) -> Result<String, OAuthManagerError> {
    Ok(manager
        .login(LoginOptions {
            on_device_code: options.on_device_code,
            signal: options.signal,
        })
        .await?
        .access_token)
}

fn manager_operation_error<E>(error: OAuthManagerError) -> KimiOAuthToolkitOperationError<E> {
    KimiOAuthToolkitOperationError::Toolkit(KimiOAuthToolkitError::Manager(error))
}

fn is_unauthorized_manager_error(error: &OAuthManagerError) -> bool {
    error
        .oauth()
        .is_some_and(|error| error.kind() == super::errors::OAuthErrorKind::Unauthorized)
}

fn is_unauthorized_provision_error<E>(error: &ProvisionManagedKimiCodeError<E>) -> bool {
    matches!(error, ProvisionManagedKimiCodeError::Models(error) if error.is_unauthorized())
}

fn normalize_oauth_host(oauth_host: &str) -> String {
    oauth_host.trim().trim_end_matches('/').to_owned()
}

fn managed_usage_url(base_url: Option<&str>) -> String {
    base_url.map_or_else(kimi_code_usage_url, |base_url| {
        format!("{}/usages", base_url.trim_end_matches('/'))
    })
}

fn managed_user_info_url(base_url: Option<&str>) -> String {
    base_url.map_or_else(kimi_code_user_info_url, |base_url| {
        format!("{}/me", base_url.trim_end_matches('/'))
    })
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
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };

    use async_trait::async_trait;
    use serde_json::{Map, Value};

    use super::*;
    use crate::managed_config::{
        ManagedConfigError, apply_managed_kimi_code_config, apply_managed_kimi_code_logout_config,
    };
    use crate::{
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

    struct DeviceRuntime {
        device_requests: AtomicUsize,
    }

    #[async_trait]
    impl OAuthManagerRuntime for DeviceRuntime {
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
            self.device_requests.fetch_add(1, Ordering::SeqCst);
            Ok(DeviceAuthorization {
                user_code: "CODE".to_owned(),
                device_code: "device".to_owned(),
                verification_uri: "https://auth.example/device".to_owned(),
                verification_uri_complete: "https://auth.example/device?code=CODE".to_owned(),
                expires_in: Some(600.0),
                interval: 0.0,
            })
        }

        async fn poll_device(
            &self,
            _config: &OAuthFlowConfig,
            _device_code: &str,
        ) -> Result<DevicePollResult, OAuthError> {
            Ok(DevicePollResult::Success(token("device-access")))
        }
    }

    struct RefreshRuntime {
        refreshes: AtomicUsize,
        device_requests: AtomicUsize,
    }

    #[async_trait]
    impl OAuthManagerRuntime for RefreshRuntime {
        fn now_seconds(&self) -> f64 {
            100.0
        }

        async fn sleep(&self, _duration: Duration) {}

        async fn refresh_token(
            &self,
            _config: &OAuthFlowConfig,
            _refresh_token: &str,
        ) -> Result<TokenInfo, OAuthError> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            Ok(token("refreshed-access"))
        }

        async fn request_device(
            &self,
            _config: &OAuthFlowConfig,
        ) -> Result<DeviceAuthorization, OAuthError> {
            self.device_requests.fetch_add(1, Ordering::SeqCst);
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

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ConfigAdapterError(String);

    impl fmt::Display for ConfigAdapterError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl Error for ConfigAdapterError {}

    struct ConfigAdapter {
        initial: Value,
        written: Mutex<Option<Value>>,
        events: Mutex<Vec<&'static str>>,
        cleanup: bool,
    }

    #[async_trait]
    impl ManagedKimiConfigAdapter for ConfigAdapter {
        type Config = Map<String, Value>;
        type Error = ConfigAdapterError;

        async fn read(&self) -> Result<Self::Config, Self::Error> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("read");
            self.initial
                .as_object()
                .cloned()
                .ok_or_else(|| ConfigAdapterError("config is not an object".to_owned()))
        }

        async fn write(&self, config: Self::Config) -> Result<(), Self::Error> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("write");
            *self
                .written
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Value::Object(config));
            Ok(())
        }

        fn apply(
            &self,
            config: &mut Self::Config,
            options: ManagedKimiCodeApplyOptions<'_>,
        ) -> Result<ManagedKimiCodeApplyResult, Self::Error> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("apply");
            apply_managed_kimi_code_config(config, options).map_err(map_config_error)
        }

        fn supports_remove(&self) -> bool {
            self.cleanup
        }

        fn remove(&self, config: &mut Self::Config) -> Result<(), Self::Error> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("remove");
            apply_managed_kimi_code_logout_config(config);
            Ok(())
        }
    }

    fn map_config_error(error: ManagedConfigError) -> ConfigAdapterError {
        ConfigAdapterError(error.to_string())
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

    fn sequence_models_server(
        responses: Vec<(u16, &'static str)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind models server");
        let address = listener.local_addr().expect("models server address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept models request");
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 4_096];
                loop {
                    let count = stream.read(&mut buffer).expect("read models request");
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some(header_end) = text.find("\r\n\r\n") {
                        let content_length = text[..header_end]
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(String::from_utf8_lossy(&bytes).into_owned());
                let response = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write models response");
            }
        });
        (format!("http://{address}/coding/v1"), requests, handle)
    }

    #[tokio::test]
    async fn login_without_existing_token_uses_device_flow_and_logout_removes_token() {
        let storage = Arc::new(MemoryTokenStorage::default());
        let runtime = Arc::new(DeviceRuntime {
            device_requests: AtomicUsize::new(0),
        });
        let toolkit = KimiOAuthToolkit::new(KimiOAuthToolkitOptions::<NoManagedConfigAdapter> {
            home_dir: Some(PathBuf::from("C:/tmp/kimi-login-test")),
            storage: Some(storage.clone()),
            runtime: Some(runtime.clone()),
            ..KimiOAuthToolkitOptions::default()
        })
        .expect("toolkit");

        let result = toolkit
            .login(None, KimiOAuthLoginOptions::default())
            .await
            .expect("login");
        assert!(result.ok);
        assert_eq!(result.provider_name, "managed:kimi-code");
        assert_eq!(result.provision, None);
        assert_eq!(runtime.device_requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            storage.load("kimi-code").await.expect("stored token"),
            Some(token("device-access"))
        );

        let logout = toolkit.logout(None, None).await.expect("logout");
        assert!(logout.ok);
        assert_eq!(logout.provider_name, "managed:kimi-code");
        assert_eq!(storage.load("kimi-code").await.expect("removed"), None);
    }

    #[tokio::test]
    async fn provisioning_unauthorized_for_existing_token_forces_refresh_then_retries() {
        let (base_url, requests, server) = sequence_models_server(vec![
            (401, r#"{"error":{"message":"expired"}}"#),
            (
                200,
                r#"{"data":[{"id":"kimi-for-coding","context_length":262144,"supports_reasoning":true}]}"#,
            ),
        ]);
        let storage = Arc::new(MemoryTokenStorage::default());
        storage
            .save("test-slot", &token("old-access"))
            .await
            .expect("save old token");
        let runtime = Arc::new(RefreshRuntime {
            refreshes: AtomicUsize::new(0),
            device_requests: AtomicUsize::new(0),
        });
        let adapter = Arc::new(ConfigAdapter {
            initial: serde_json::json!({ "providers": {} }),
            written: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            cleanup: false,
        });
        let toolkit = KimiOAuthToolkit::new(KimiOAuthToolkitOptions {
            identity: None,
            home_dir: Some(PathBuf::from("C:/tmp/kimi-provision-retry")),
            credentials_dir: None,
            storage: Some(storage),
            flow_config: None,
            runtime: Some(runtime.clone()),
            device_code_timeout: None,
            refresh_threshold: None,
            on_refresh: None,
            config_adapter: Some(adapter.clone()),
        })
        .expect("toolkit");
        let oauth_ref = KimiOAuthTokenRef {
            key: Some("oauth/test-slot".to_owned()),
            oauth_host: None,
        };

        let result = toolkit
            .login(
                None,
                KimiOAuthLoginOptions {
                    provision_config: Some(true),
                    base_url: Some(&base_url),
                    oauth_ref: Some(&oauth_ref),
                    ..KimiOAuthLoginOptions::default()
                },
            )
            .await
            .expect("login and provision");
        server.join().expect("models server thread");

        assert!(result.provision.is_some());
        assert_eq!(runtime.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.device_requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            adapter
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            ["read", "apply", "write"]
        );
        let requests = requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("authorization: bearer old-access")
        );
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("authorization: bearer refreshed-access")
        );
    }

    #[tokio::test]
    async fn logout_runs_optional_managed_config_cleanup_only_for_managed_provider() {
        let storage = Arc::new(MemoryTokenStorage::default());
        storage
            .save("kimi-code", &token("access"))
            .await
            .expect("save token");
        let adapter = Arc::new(ConfigAdapter {
            initial: serde_json::json!({
                "providers": {
                    "managed:kimi-code": { "type": "kimi" },
                    "custom": { "type": "openai" }
                },
                "models": {
                    "kimi-code/kimi": { "provider": "managed:kimi-code", "model": "kimi" },
                    "custom/model": { "provider": "custom", "model": "model" }
                }
            }),
            written: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            cleanup: true,
        });
        let toolkit = KimiOAuthToolkit::new(KimiOAuthToolkitOptions {
            identity: None,
            home_dir: Some(PathBuf::from("C:/tmp/kimi-logout-cleanup")),
            credentials_dir: None,
            storage: Some(storage),
            flow_config: None,
            runtime: Some(Arc::new(StaticRuntime)),
            device_code_timeout: None,
            refresh_threshold: None,
            on_refresh: None,
            config_adapter: Some(adapter.clone()),
        })
        .expect("toolkit");

        toolkit.logout(None, None).await.expect("logout");

        assert_eq!(
            adapter
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            ["read", "remove", "write"]
        );
        let written = adapter
            .written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let written = written.as_ref().expect("written config");
        assert!(written["providers"].get("managed:kimi-code").is_none());
        assert!(written["providers"].get("custom").is_some());
        assert!(written["models"].get("kimi-code/kimi").is_none());
        assert!(written["models"].get("custom/model").is_some());
    }

    #[tokio::test]
    async fn authenticated_service_methods_share_token_resolution_and_preserve_results() {
        let (base_url, requests, server) = sequence_models_server(vec![
            (200, r#"{"usage":{"used":40,"limit":1000}}"#),
            (200, r#"{"feedback_id":7}"#),
            (
                200,
                r#"{"upload":{"id":9,"parts":[{"part_number":1,"url":"https://upload.example/1","method":"PUT","size":12}]}}"#,
            ),
            (200, r#"{}"#),
        ]);
        let storage = Arc::new(MemoryTokenStorage::default());
        storage
            .save("service-slot", &token("service-access"))
            .await
            .expect("save service token");
        let toolkit = KimiOAuthToolkit::new(toolkit_options(storage)).expect("toolkit");
        let oauth_ref = KimiOAuthTokenRef {
            key: Some("oauth/service-slot".to_owned()),
            oauth_host: None,
        };

        let usage = toolkit
            .get_managed_usage(
                None,
                AuthenticatedServiceOptions {
                    oauth_ref: Some(&oauth_ref),
                    base_url: Some(&base_url),
                },
            )
            .await;
        let AuthManagedUsageResult::Ok { summary, .. } = usage else {
            panic!("expected usage success")
        };
        assert_eq!(summary.expect("usage summary").used, 40.0);

        let feedback = toolkit
            .submit_feedback(
                &SubmitFeedbackBody {
                    session_id: "session".to_owned(),
                    content: "feedback".to_owned(),
                    version: "1.0.0".to_owned(),
                    os: "Windows".to_owned(),
                    model: Some("kimi-code/kimi".to_owned()),
                    contact: None,
                    info: None,
                },
                None,
                AuthenticatedServiceOptions {
                    oauth_ref: Some(&oauth_ref),
                    base_url: Some(&base_url),
                },
            )
            .await;
        assert_eq!(feedback, FetchSubmitFeedbackResult::Ok { feedback_id: 7.0 });

        let created = toolkit
            .create_feedback_upload_url(
                &CreateFeedbackUploadUrlBody {
                    file_hash: "sha256".to_owned(),
                    file_name: "report.zip".to_owned(),
                    file_size: 12,
                    feedback_id: 7,
                },
                None,
                AuthenticatedServiceOptions {
                    oauth_ref: Some(&oauth_ref),
                    base_url: Some(&base_url),
                },
            )
            .await;
        assert!(matches!(
            created,
            FetchCreateFeedbackUploadUrlResult::Ok { upload_id: 9, .. }
        ));

        let completed = toolkit
            .complete_feedback_upload(
                &CompleteFeedbackUploadBody {
                    upload_id: 9,
                    parts: Vec::new(),
                },
                None,
                AuthenticatedServiceOptions {
                    oauth_ref: Some(&oauth_ref),
                    base_url: Some(&base_url),
                },
            )
            .await;
        assert_eq!(completed, FetchCompleteFeedbackUploadResult::Ok);
        server.join().expect("service server thread");

        let requests = requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests.len(), 4);
        for request in requests.iter() {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer service-access")
            );
        }
        assert!(requests[0].starts_with("GET /coding/v1/usages HTTP/1.1"));
        assert!(requests[1].starts_with("POST /coding/v1/feedback HTTP/1.1"));
        assert!(requests[2].starts_with("POST /coding/v1/feedback/upload_url HTTP/1.1"));
        assert!(requests[3].starts_with("POST /coding/v1/feedback/upload_complete HTTP/1.1"));
    }

    #[tokio::test]
    async fn authenticated_services_convert_token_failures_to_result_errors() {
        let toolkit =
            KimiOAuthToolkit::new(toolkit_options(Arc::new(MemoryTokenStorage::default())))
                .expect("toolkit");

        let result = toolkit
            .get_managed_usage(None, AuthenticatedServiceOptions::default())
            .await;
        let AuthManagedUsageResult::Error { status, message } = result else {
            panic!("expected token error")
        };
        assert_eq!(status, None);
        assert!(!message.is_empty());
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
