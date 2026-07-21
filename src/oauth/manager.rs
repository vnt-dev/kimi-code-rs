use std::{
    error::Error,
    fmt,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;

use super::{
    errors::OAuthError,
    flow::{
        DeviceHeaders, DevicePollResult, RefreshOptions, poll_device_token, refresh_access_token,
        request_device_authorization,
    },
    storage::{TokenStorage, TokenStorageError},
    token_state::{TokenState, classify_token},
    types::{DeviceAuthorization, OAuthFlowConfig, TokenInfo},
};

const MIN_REFRESH_THRESHOLD_SECONDS: f64 = 300.0;
const REFRESH_THRESHOLD_RATIO: f64 = 0.5;
const DEFAULT_DEVICE_CODE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

// Original:
//   packages/oauth/src/oauth-manager.ts
//   defaultRefreshThreshold()
pub fn default_refresh_threshold(expires_in: f64) -> f64 {
    if expires_in > 0.0 {
        MIN_REFRESH_THRESHOLD_SECONDS.max(expires_in * REFRESH_THRESHOLD_RATIO)
    } else {
        MIN_REFRESH_THRESHOLD_SECONDS
    }
}

#[derive(Debug)]
pub enum OAuthManagerError {
    OAuth(OAuthError),
    Storage(TokenStorageError),
}

impl OAuthManagerError {
    pub fn oauth(&self) -> Option<&OAuthError> {
        match self {
            Self::OAuth(error) => Some(error),
            Self::Storage(_) => None,
        }
    }
}

impl fmt::Display for OAuthManagerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OAuth(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl Error for OAuthManagerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::OAuth(error) => Some(error),
            Self::Storage(error) => Some(error),
        }
    }
}

impl From<OAuthError> for OAuthManagerError {
    fn from(error: OAuthError) -> Self {
        Self::OAuth(error)
    }
}

impl From<TokenStorageError> for OAuthManagerError {
    fn from(error: TokenStorageError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthRefreshOutcome {
    Success,
    Unauthorized,
    NetworkOrOther,
}

#[async_trait]
pub trait OAuthManagerRuntime: Send + Sync {
    fn now_seconds(&self) -> f64;

    async fn sleep(&self, duration: Duration);

    async fn refresh_token(
        &self,
        config: &OAuthFlowConfig,
        refresh_token: &str,
    ) -> Result<TokenInfo, OAuthError>;

    async fn request_device(
        &self,
        config: &OAuthFlowConfig,
    ) -> Result<DeviceAuthorization, OAuthError>;

    async fn poll_device(
        &self,
        config: &OAuthFlowConfig,
        device_code: &str,
    ) -> Result<DevicePollResult, OAuthError>;
}

type DeviceHeaderFactory = dyn Fn() -> Option<DeviceHeaders> + Send + Sync;

#[derive(Clone, Default)]
pub struct SystemOAuthManagerRuntime {
    device_headers: Option<Arc<DeviceHeaderFactory>>,
}

impl SystemOAuthManagerRuntime {
    pub fn new(device_headers: Option<Arc<DeviceHeaderFactory>>) -> Self {
        Self { device_headers }
    }

    fn device_headers(&self) -> Option<DeviceHeaders> {
        self.device_headers.as_ref().and_then(|factory| factory())
    }
}

#[async_trait]
impl OAuthManagerRuntime for SystemOAuthManagerRuntime {
    fn now_seconds(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as f64
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    async fn refresh_token(
        &self,
        config: &OAuthFlowConfig,
        refresh_token: &str,
    ) -> Result<TokenInfo, OAuthError> {
        let headers = self.device_headers();
        refresh_access_token(
            config,
            refresh_token,
            RefreshOptions {
                device_headers: headers.as_ref(),
                ..RefreshOptions::default()
            },
        )
        .await
    }

    async fn request_device(
        &self,
        config: &OAuthFlowConfig,
    ) -> Result<DeviceAuthorization, OAuthError> {
        let headers = self.device_headers();
        request_device_authorization(config, headers.as_ref()).await
    }

    async fn poll_device(
        &self,
        config: &OAuthFlowConfig,
        device_code: &str,
    ) -> Result<DevicePollResult, OAuthError> {
        let headers = self.device_headers();
        poll_device_token(config, device_code, headers.as_ref()).await
    }
}

#[async_trait]
pub trait DeviceCodeObserver: Send + Sync {
    async fn on_device_code(
        &self,
        authorization: &DeviceAuthorization,
    ) -> Result<(), OAuthManagerError>;
}

pub trait LoginAbortSignal: Send + Sync {
    fn is_aborted(&self) -> bool;
}

impl LoginAbortSignal for AtomicBool {
    fn is_aborted(&self) -> bool {
        self.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[derive(Default)]
pub struct LoginOptions<'a> {
    pub on_device_code: Option<&'a dyn DeviceCodeObserver>,
    pub signal: Option<&'a dyn LoginAbortSignal>,
}

pub struct OAuthManager {
    config: OAuthFlowConfig,
    storage: Arc<dyn TokenStorage>,
    runtime: Arc<dyn OAuthManagerRuntime>,
    device_code_timeout: Duration,
}

impl OAuthManager {
    pub fn new(
        config: OAuthFlowConfig,
        storage: Arc<dyn TokenStorage>,
        runtime: Arc<dyn OAuthManagerRuntime>,
    ) -> Self {
        Self {
            config,
            storage,
            runtime,
            device_code_timeout: DEFAULT_DEVICE_CODE_TIMEOUT,
        }
    }

    pub fn with_device_code_timeout(mut self, timeout: Duration) -> Self {
        self.device_code_timeout = timeout;
        self
    }

    async fn load_state(&self) -> Result<TokenState, OAuthManagerError> {
        Ok(classify_token(self.storage.load(&self.config.name).await?))
    }

    // Original: OAuthManager.hasToken()
    pub async fn has_token(&self) -> Result<bool, OAuthManagerError> {
        Ok(matches!(self.load_state().await?, TokenState::Valid(_)))
    }

    // Original: OAuthManager.getCachedAccessToken()
    pub async fn get_cached_access_token(&self) -> Result<Option<String>, OAuthManagerError> {
        Ok(match self.load_state().await? {
            TokenState::Valid(token) => Some(token.access_token),
            TokenState::Revoked { .. } | TokenState::Missing => None,
        })
    }

    // Original: OAuthManager.logout()
    pub async fn logout(&self) -> Result<(), OAuthManagerError> {
        self.storage.remove(&self.config.name).await?;
        Ok(())
    }

    // Original: OAuthManager.login()
    pub async fn login(&self, options: LoginOptions<'_>) -> Result<TokenInfo, OAuthManagerError> {
        let timeout_seconds = self.device_code_timeout.as_secs_f64().ceil();
        let deadline_at = self.runtime.now_seconds() + timeout_seconds;

        loop {
            let authorization = self.runtime.request_device(&self.config).await?;
            if let Some(observer) = options.on_device_code {
                observer.on_device_code(&authorization).await?;
            }

            let mut current_interval = js_math_max(authorization.interval, 1.0);
            loop {
                self.throw_if_aborted(options.signal)?;
                if self.runtime.now_seconds() >= deadline_at {
                    return Err(OAuthError::device_code_timeout_with_message(format!(
                        "Device authorization timed out after {timeout_seconds:.0}s"
                    ))
                    .into());
                }

                match self
                    .runtime
                    .poll_device(&self.config, &authorization.device_code)
                    .await?
                {
                    DevicePollResult::Success(token) => {
                        self.storage.save(&self.config.name, &token).await?;
                        return Ok(token);
                    }
                    DevicePollResult::Denied { description } => {
                        let suffix = if description.is_empty() {
                            String::new()
                        } else {
                            format!(": {description}")
                        };
                        return Err(OAuthError::new(format!("Authorization denied{suffix}")).into());
                    }
                    DevicePollResult::Expired => break,
                    DevicePollResult::Pending { error_code, .. } => {
                        if error_code == "slow_down" {
                            current_interval += 5.0;
                        }
                        self.runtime
                            .sleep(js_timer_duration(current_interval * 1_000.0))
                            .await;
                    }
                }
            }

            if self.runtime.now_seconds() >= deadline_at {
                return Err(OAuthError::device_code_timeout_with_message(
                    "Device authorization timed out",
                )
                .into());
            }
        }
    }

    fn throw_if_aborted(
        &self,
        signal: Option<&dyn LoginAbortSignal>,
    ) -> Result<(), OAuthManagerError> {
        if signal.is_some_and(LoginAbortSignal::is_aborted) {
            return Err(OAuthError::new("Login aborted by caller").into());
        }
        Ok(())
    }
}

fn js_math_max(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else {
        left.max(right)
    }
}

fn js_timer_duration(milliseconds: f64) -> Duration {
    if !milliseconds.is_finite() || milliseconds <= 0.0 {
        Duration::ZERO
    } else {
        Duration::try_from_secs_f64(milliseconds / 1_000.0).unwrap_or(Duration::MAX)
    }
}

// Original: newInstanceId()
pub fn new_instance_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, VecDeque},
        sync::{Mutex, atomic::Ordering},
    };

    use super::*;

    #[derive(Default)]
    struct MemoryStorage {
        tokens: Mutex<HashMap<String, TokenInfo>>,
    }

    #[async_trait]
    impl TokenStorage for MemoryStorage {
        async fn load(&self, name: &str) -> Result<Option<TokenInfo>, TokenStorageError> {
            Ok(self.tokens.lock().expect("tokens lock").get(name).cloned())
        }

        async fn save(&self, name: &str, token: &TokenInfo) -> Result<(), TokenStorageError> {
            self.tokens
                .lock()
                .expect("tokens lock")
                .insert(name.to_owned(), token.clone());
            Ok(())
        }

        async fn remove(&self, name: &str) -> Result<(), TokenStorageError> {
            self.tokens.lock().expect("tokens lock").remove(name);
            Ok(())
        }

        async fn list(&self) -> Result<Vec<String>, TokenStorageError> {
            Ok(self
                .tokens
                .lock()
                .expect("tokens lock")
                .keys()
                .cloned()
                .collect())
        }
    }

    struct FakeRuntime {
        now: Mutex<f64>,
        authorizations: Mutex<VecDeque<DeviceAuthorization>>,
        polls: Mutex<VecDeque<DevicePollResult>>,
        sleeps: Mutex<Vec<Duration>>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl FakeRuntime {
        fn new(authorizations: Vec<DeviceAuthorization>, polls: Vec<DevicePollResult>) -> Self {
            Self {
                now: Mutex::new(0.0),
                authorizations: Mutex::new(authorizations.into()),
                polls: Mutex::new(polls.into()),
                sleeps: Mutex::new(Vec::new()),
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl OAuthManagerRuntime for FakeRuntime {
        fn now_seconds(&self) -> f64 {
            *self.now.lock().expect("now lock")
        }

        async fn sleep(&self, duration: Duration) {
            self.sleeps.lock().expect("sleeps lock").push(duration);
            *self.now.lock().expect("now lock") += duration.as_secs_f64();
        }

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
            self.events.lock().expect("events lock").push("request");
            self.authorizations
                .lock()
                .expect("authorization lock")
                .pop_front()
                .ok_or_else(|| OAuthError::new("no fake authorization"))
        }

        async fn poll_device(
            &self,
            _config: &OAuthFlowConfig,
            _device_code: &str,
        ) -> Result<DevicePollResult, OAuthError> {
            self.events.lock().expect("events lock").push("poll");
            self.polls
                .lock()
                .expect("polls lock")
                .pop_front()
                .ok_or_else(|| OAuthError::new("no fake poll result"))
        }
    }

    struct RecordingObserver(Arc<Mutex<Vec<&'static str>>>);

    #[async_trait]
    impl DeviceCodeObserver for RecordingObserver {
        async fn on_device_code(
            &self,
            _authorization: &DeviceAuthorization,
        ) -> Result<(), OAuthManagerError> {
            self.0.lock().expect("events lock").push("observe");
            Ok(())
        }
    }

    fn config() -> OAuthFlowConfig {
        OAuthFlowConfig {
            name: "kimi-code".to_owned(),
            oauth_host: "https://test".to_owned(),
            client_id: "test".to_owned(),
        }
    }

    fn authorization(device_code: &str, interval: f64) -> DeviceAuthorization {
        DeviceAuthorization {
            user_code: "USER".to_owned(),
            device_code: device_code.to_owned(),
            verification_uri: "https://test/verify".to_owned(),
            verification_uri_complete: "https://test/verify?code=USER".to_owned(),
            expires_in: Some(600.0),
            interval,
        }
    }

    fn token(access_token: &str) -> TokenInfo {
        TokenInfo {
            access_token: access_token.to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_at: 2_000_000_000.0,
            scope: String::new(),
            token_type: "Bearer".to_owned(),
            expires_in: 3_600.0,
        }
    }

    #[test]
    fn refresh_threshold_uses_half_the_lifetime_with_a_five_minute_floor() {
        for (expires_in, expected) in [
            (-1.0, 300.0),
            (0.0, 300.0),
            (1.0, 300.0),
            (600.0, 300.0),
            (3_600.0, 1_800.0),
        ] {
            assert_eq!(default_refresh_threshold(expires_in), expected);
        }
    }

    #[test]
    fn refresh_threshold_preserves_javascript_nan_and_infinity_edges() {
        assert_eq!(default_refresh_threshold(f64::NAN), 300.0);
        assert_eq!(default_refresh_threshold(f64::NEG_INFINITY), 300.0);
        assert_eq!(default_refresh_threshold(f64::INFINITY), f64::INFINITY);
    }

    #[tokio::test]
    async fn token_lifecycle_classifies_cached_revoked_and_removed_tokens() {
        let storage = Arc::new(MemoryStorage::default());
        let runtime = Arc::new(FakeRuntime::new(Vec::new(), Vec::new()));
        let manager = OAuthManager::new(config(), storage.clone(), runtime);

        assert!(!manager.has_token().await.expect("missing state"));
        assert_eq!(
            manager
                .get_cached_access_token()
                .await
                .expect("missing cache"),
            None
        );
        storage
            .save("kimi-code", &token("access"))
            .await
            .expect("store token");
        assert!(manager.has_token().await.expect("valid state"));
        assert_eq!(
            manager
                .get_cached_access_token()
                .await
                .expect("cached token")
                .as_deref(),
            Some("access")
        );
        storage
            .save("kimi-code", &token(""))
            .await
            .expect("store tombstone");
        assert!(!manager.has_token().await.expect("revoked state"));

        manager.logout().await.expect("logout");
        assert!(
            storage
                .load("kimi-code")
                .await
                .expect("load after logout")
                .is_none()
        );
    }

    #[tokio::test]
    async fn login_observes_code_polls_slow_down_and_persists_success() {
        let storage = Arc::new(MemoryStorage::default());
        let runtime = Arc::new(FakeRuntime::new(
            vec![authorization("device-1", 2.0)],
            vec![
                DevicePollResult::Pending {
                    error_code: "authorization_pending".to_owned(),
                    description: String::new(),
                },
                DevicePollResult::Pending {
                    error_code: "slow_down".to_owned(),
                    description: String::new(),
                },
                DevicePollResult::Success(token("logged-in")),
            ],
        ));
        let observer = RecordingObserver(Arc::clone(&runtime.events));
        let manager = OAuthManager::new(config(), storage.clone(), runtime.clone());

        let result = manager
            .login(LoginOptions {
                on_device_code: Some(&observer),
                signal: None,
            })
            .await
            .expect("login succeeds");
        assert_eq!(result.access_token, "logged-in");
        assert_eq!(
            *runtime.events.lock().expect("events lock"),
            ["request", "observe", "poll", "poll", "poll"]
        );
        assert_eq!(
            *runtime.sleeps.lock().expect("sleeps lock"),
            [Duration::from_secs(2), Duration::from_secs(7)]
        );
        assert_eq!(
            storage
                .load("kimi-code")
                .await
                .expect("stored login token")
                .expect("token exists")
                .access_token,
            "logged-in"
        );
    }

    #[tokio::test]
    async fn expired_device_code_restarts_the_outer_flow() {
        let storage = Arc::new(MemoryStorage::default());
        let runtime = Arc::new(FakeRuntime::new(
            vec![authorization("expired", 1.0), authorization("next", 1.0)],
            vec![
                DevicePollResult::Expired,
                DevicePollResult::Success(token("second-flow")),
            ],
        ));
        let manager = OAuthManager::new(config(), storage, runtime.clone());

        assert_eq!(
            manager
                .login(LoginOptions::default())
                .await
                .expect("second flow succeeds")
                .access_token,
            "second-flow"
        );
        assert_eq!(
            runtime
                .events
                .lock()
                .expect("events lock")
                .iter()
                .filter(|event| **event == "request")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn denial_timeout_and_abort_preserve_distinct_errors() {
        let denied_runtime = Arc::new(FakeRuntime::new(
            vec![authorization("d", 1.0)],
            vec![DevicePollResult::Denied {
                description: "user declined".to_owned(),
            }],
        ));
        let denied =
            OAuthManager::new(config(), Arc::new(MemoryStorage::default()), denied_runtime)
                .login(LoginOptions::default())
                .await
                .expect_err("denied login");
        assert_eq!(denied.to_string(), "Authorization denied: user declined");

        let timeout_runtime = Arc::new(FakeRuntime::new(
            vec![authorization("d", 1.0)],
            vec![DevicePollResult::Pending {
                error_code: "authorization_pending".to_owned(),
                description: String::new(),
            }],
        ));
        let timeout = OAuthManager::new(
            config(),
            Arc::new(MemoryStorage::default()),
            timeout_runtime,
        )
        .with_device_code_timeout(Duration::from_secs(1))
        .login(LoginOptions::default())
        .await
        .expect_err("timed out login");
        assert_eq!(
            timeout.oauth().map(OAuthError::kind),
            Some(super::super::errors::OAuthErrorKind::DeviceCodeTimeout)
        );
        assert_eq!(
            timeout.to_string(),
            "Device authorization timed out after 1s"
        );

        let abort_runtime = Arc::new(FakeRuntime::new(vec![authorization("d", 1.0)], Vec::new()));
        let signal = AtomicBool::new(true);
        signal.store(true, Ordering::SeqCst);
        let aborted =
            OAuthManager::new(config(), Arc::new(MemoryStorage::default()), abort_runtime)
                .login(LoginOptions {
                    on_device_code: None,
                    signal: Some(&signal),
                })
                .await
                .expect_err("aborted login");
        assert_eq!(aborted.to_string(), "Login aborted by caller");
    }

    #[test]
    fn instance_ids_are_unique_uuid_values() {
        let first = new_instance_id();
        let second = new_instance_id();
        assert_ne!(first, second);
        assert!(uuid::Uuid::parse_str(&first).is_ok());
    }
}
