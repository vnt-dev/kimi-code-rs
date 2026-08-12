use std::{
    error::Error,
    fmt,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::FutureExt;
use tokio::{
    sync::{Mutex as AsyncMutex, Notify, OnceCell, oneshot},
    task::JoinHandle,
};

use super::{
    errors::OAuthError,
    flow::{
        DeviceHeaders, DevicePollResult, RefreshOptions, poll_device_token, refresh_access_token,
        request_device_authorization,
    },
    storage::{TokenStorage, TokenStorageError},
    token_state::{TokenState, classify_token, revoked_tombstone},
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

#[derive(Debug, Clone)]
pub enum OAuthManagerError {
    OAuth(Arc<OAuthError>),
    Storage(Arc<TokenStorageError>),
}

impl OAuthManagerError {
    pub fn oauth(&self) -> Option<&OAuthError> {
        match self {
            Self::OAuth(error) => Some(error.as_ref()),
            Self::Storage(_) => None,
        }
    }
}

impl fmt::Display for OAuthManagerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OAuth(error) => error.as_ref().fmt(formatter),
            Self::Storage(error) => error.as_ref().fmt(formatter),
        }
    }
}

impl Error for OAuthManagerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::OAuth(error) => Some(error.as_ref()),
            Self::Storage(error) => Some(error.as_ref()),
        }
    }
}

impl From<OAuthError> for OAuthManagerError {
    fn from(error: OAuthError) -> Self {
        Self::OAuth(Arc::new(error))
    }
}

impl From<TokenStorageError> for OAuthManagerError {
    fn from(error: TokenStorageError) -> Self {
        Self::Storage(Arc::new(error))
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

#[derive(Clone)]
enum DeviceHeaderSource {
    Factory(Arc<DeviceHeaderFactory>),
    Identity { home_dir: PathBuf, version: String },
}

#[derive(Clone, Default)]
pub struct SystemOAuthManagerRuntime {
    device_header_source: Option<DeviceHeaderSource>,
    device_headers: Arc<OnceCell<Option<DeviceHeaders>>>,
}

impl SystemOAuthManagerRuntime {
    pub fn new(device_headers: Option<Arc<DeviceHeaderFactory>>) -> Self {
        Self {
            device_header_source: device_headers.map(DeviceHeaderSource::Factory),
            device_headers: Arc::new(OnceCell::new()),
        }
    }

    pub fn with_device_identity(home_dir: PathBuf, version: String) -> Self {
        Self {
            device_header_source: Some(DeviceHeaderSource::Identity { home_dir, version }),
            device_headers: Arc::new(OnceCell::new()),
        }
    }

    async fn device_headers(&self) -> Option<DeviceHeaders> {
        self.device_headers
            .get_or_init(|| async {
                match self.device_header_source.clone() {
                    Some(DeviceHeaderSource::Factory(factory)) => {
                        tokio::task::spawn_blocking(move || factory())
                            .await
                            .unwrap_or(None)
                    }
                    Some(DeviceHeaderSource::Identity { home_dir, version }) => {
                        super::identity::create_kimi_device_headers_async(&home_dir, &version)
                            .await
                            .ok()
                    }
                    None => None,
                }
            })
            .await
            .clone()
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
        let headers = self.device_headers().await;
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
        let headers = self.device_headers().await;
        request_device_authorization(config, headers.as_ref()).await
    }

    async fn poll_device(
        &self,
        config: &OAuthFlowConfig,
        device_code: &str,
    ) -> Result<DevicePollResult, OAuthError> {
        let headers = self.device_headers().await;
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
    refresh_threshold: Arc<dyn Fn(f64) -> f64 + Send + Sync>,
    on_refresh: Option<Arc<dyn Fn(OAuthRefreshOutcome) + Send + Sync>>,
    config_dir: Option<PathBuf>,
    in_flight_refresh: AsyncMutex<Option<Arc<RefreshFlight>>>,
}

struct RefreshFlight {
    force: bool,
    result: AsyncMutex<Option<Result<String, OAuthManagerError>>>,
    notify: Notify,
}

impl RefreshFlight {
    fn new(force: bool) -> Self {
        Self {
            force,
            result: AsyncMutex::new(None),
            notify: Notify::new(),
        }
    }

    async fn complete(&self, result: Result<String, OAuthManagerError>) {
        *self.result.lock().await = Some(result);
        self.notify.notify_waiters();
    }

    async fn wait(&self) -> Result<String, OAuthManagerError> {
        loop {
            let notified = self.notify.notified();
            if let Some(result) = self.result.lock().await.clone() {
                return result;
            }
            notified.await;
        }
    }
}

struct RefreshLockGuard {
    directory: Option<PathBuf>,
    heartbeat_stop: Option<oneshot::Sender<()>>,
    heartbeat_task: Option<JoinHandle<()>>,
}

impl RefreshLockGuard {
    async fn release(mut self) {
        if let Some(stop) = self.heartbeat_stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.heartbeat_task.take() {
            let _ = task.await;
        }
        if let Some(directory) = self.directory.take() {
            let _ = tokio::fs::remove_file(directory.join("heartbeat")).await;
            let _ = tokio::fs::remove_dir(directory).await;
        }
    }
}

impl Drop for RefreshLockGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.heartbeat_stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.heartbeat_task.take() {
            task.abort();
        }
        if let Some(directory) = self.directory.take() {
            let _ = std::fs::remove_file(directory.join("heartbeat"));
            let _ = std::fs::remove_dir(directory);
        }
    }
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
            refresh_threshold: Arc::new(default_refresh_threshold),
            on_refresh: None,
            config_dir: test_config_dir_fallback(),
            in_flight_refresh: AsyncMutex::new(None),
        }
    }

    pub fn with_device_code_timeout(mut self, timeout: Duration) -> Self {
        self.device_code_timeout = timeout;
        self
    }

    pub fn with_refresh_threshold(
        mut self,
        threshold: Arc<dyn Fn(f64) -> f64 + Send + Sync>,
    ) -> Self {
        self.refresh_threshold = threshold;
        self
    }

    pub fn with_refresh_observer(
        mut self,
        observer: Arc<dyn Fn(OAuthRefreshOutcome) + Send + Sync>,
    ) -> Self {
        self.on_refresh = Some(observer);
        self
    }

    pub fn with_config_dir(mut self, config_dir: impl Into<PathBuf>) -> Self {
        self.config_dir = Some(config_dir.into());
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

    // Original: OAuthManager.ensureFresh()
    pub async fn ensure_fresh(self: &Arc<Self>, force: bool) -> Result<String, OAuthManagerError> {
        loop {
            let mut slot = self.in_flight_refresh.lock().await;
            if let Some(current) = slot.as_ref().cloned() {
                drop(slot);
                if !force || current.force {
                    return current.wait().await;
                }
                let _ = current.wait().await;
                continue;
            }

            let flight = Arc::new(RefreshFlight::new(force));
            *slot = Some(Arc::clone(&flight));
            drop(slot);

            let manager = Arc::clone(self);
            let task_flight = Arc::clone(&flight);
            tokio::spawn(async move {
                let result = AssertUnwindSafe(manager.do_ensure_fresh(force))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(OAuthError::new("OAuth refresh task failed unexpectedly").into())
                    });
                let mut slot = manager.in_flight_refresh.lock().await;
                if slot
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &task_flight))
                {
                    *slot = None;
                }
                drop(slot);
                task_flight.complete(result).await;
            });
            return flight.wait().await;
        }
    }

    // Original: OAuthManager.doEnsureFresh()
    async fn do_ensure_fresh(&self, force: bool) -> Result<String, OAuthManagerError> {
        let initial = self.load_state().await?;
        let token = match initial {
            TokenState::Missing => {
                return Err(OAuthError::unauthorized(format!(
                    "No token for \"{}\". Run /login to authenticate.",
                    self.config.name
                ))
                .into());
            }
            TokenState::Revoked { .. } => {
                return Err(OAuthError::unauthorized(format!(
                    "Stored token for \"{}\" was rejected; re-login required.",
                    self.config.name
                ))
                .into());
            }
            TokenState::Valid(token) => token,
        };
        if !self.should_refresh_token(&token, force) {
            return Ok(token.access_token);
        }

        let refresh_lock = self.acquire_refresh_lock().await?;
        let result = self.refresh_under_lock(token, force).await;
        refresh_lock.release().await;
        result
    }

    async fn refresh_under_lock(
        &self,
        original_token: TokenInfo,
        force: bool,
    ) -> Result<String, OAuthManagerError> {
        let active_token = match self.load_state().await? {
            TokenState::Revoked { .. } => {
                return Err(OAuthError::unauthorized(format!(
                    "Stored token for \"{}\" was rejected; re-login required.",
                    self.config.name
                ))
                .into());
            }
            TokenState::Missing => original_token,
            TokenState::Valid(after) => {
                if !self.should_refresh_token(&after, force) {
                    return Ok(after.access_token);
                }
                if force && token_changed(&after, &original_token) {
                    return Ok(after.access_token);
                }
                after
            }
        };

        if active_token.refresh_token.is_empty() {
            return Err(OAuthError::unauthorized(format!(
                "Token for \"{}\" has no refresh_token; re-login required.",
                self.config.name
            ))
            .into());
        }

        match self
            .runtime
            .refresh_token(&self.config, &active_token.refresh_token)
            .await
        {
            Ok(refreshed) => {
                self.storage.save(&self.config.name, &refreshed).await?;
                self.notify_refresh(OAuthRefreshOutcome::Success);
                Ok(refreshed.access_token)
            }
            Err(error) if error.kind() == super::errors::OAuthErrorKind::Unauthorized => {
                self.runtime.sleep(Duration::from_millis(100)).await;
                if let TokenState::Valid(recovery) = self.load_state().await?
                    && recovery.refresh_token != active_token.refresh_token
                {
                    self.notify_refresh(OAuthRefreshOutcome::Success);
                    return Ok(recovery.access_token);
                }
                self.storage
                    .save(&self.config.name, &revoked_tombstone(&active_token))
                    .await?;
                self.notify_refresh(OAuthRefreshOutcome::Unauthorized);
                Err(error.into())
            }
            Err(error) => {
                self.notify_refresh(OAuthRefreshOutcome::NetworkOrOther);
                Err(error.into())
            }
        }
    }

    fn should_refresh_token(&self, token: &TokenInfo, force: bool) -> bool {
        if force {
            return true;
        }
        if token.expires_at == 0.0 {
            return false;
        }
        token.expires_at - self.runtime.now_seconds() < (self.refresh_threshold)(token.expires_in)
    }

    fn notify_refresh(&self, outcome: OAuthRefreshOutcome) {
        if let Some(observer) = &self.on_refresh {
            let _ = std::panic::catch_unwind(AssertUnwindSafe(|| observer(outcome)));
        }
    }

    fn resolve_lock_paths(&self) -> Option<(PathBuf, PathBuf)> {
        if cfg!(windows) || std::env::var("KIMI_DISABLE_OAUTH_LOCK").ok().as_deref() == Some("1") {
            return None;
        }
        let oauth_directory = self.config_dir.as_ref()?.join("oauth");
        let sentinel = oauth_directory.join(&self.config.name);
        let lock_directory = oauth_directory.join(format!("{}.lock", self.config.name));
        Some((sentinel, lock_directory))
    }

    async fn acquire_refresh_lock(&self) -> Result<RefreshLockGuard, OAuthManagerError> {
        let Some((sentinel, lock_directory)) = self.resolve_lock_paths() else {
            return Ok(RefreshLockGuard {
                directory: None,
                heartbeat_stop: None,
                heartbeat_task: None,
            });
        };
        let Some(parent) = sentinel.parent() else {
            return Err(OAuthError::new(format!(
                "Unable to prepare OAuth refresh lock for \"{}\": invalid lock path",
                self.config.name
            ))
            .into());
        };
        if let Err(error) = prepare_lock_sentinel(parent, &sentinel).await {
            return Err(OAuthError::new(format!(
                "Unable to prepare OAuth refresh lock for \"{}\": {error}",
                self.config.name
            ))
            .into());
        }

        for attempt in 0..=120 {
            match tokio::fs::create_dir(&lock_directory).await {
                Ok(()) => {
                    return start_lock_heartbeat(lock_directory).await.map_err(|error| {
                        OAuthError::new(format!(
                            "Unable to acquire OAuth refresh lock for \"{}\": {error}",
                            self.config.name
                        ))
                        .into()
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&lock_directory).await {
                        remove_lock_directory(&lock_directory).await;
                    }
                    if attempt < 120 {
                        self.runtime.sleep(Duration::from_millis(500)).await;
                        continue;
                    }
                    return Err(OAuthError::new(format!(
                        "Unable to acquire OAuth refresh lock for \"{}\": {error}",
                        self.config.name
                    ))
                    .into());
                }
                Err(error) => {
                    return Err(OAuthError::new(format!(
                        "Unable to acquire OAuth refresh lock for \"{}\": {error}",
                        self.config.name
                    ))
                    .into());
                }
            }
        }
        Err(OAuthError::new(format!(
            "Unable to acquire OAuth refresh lock for \"{}\"",
            self.config.name
        ))
        .into())
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

fn token_changed(left: &TokenInfo, right: &TokenInfo) -> bool {
    left.refresh_token != right.refresh_token
        || left.access_token != right.access_token
        || left.expires_at != right.expires_at
        || left.expires_in != right.expires_in
}

async fn prepare_lock_sentinel(parent: &Path, sentinel: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(parent).await?;
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sentinel)
        .await?;
    Ok(())
}

async fn lock_is_stale(directory: &Path) -> bool {
    let heartbeat = directory.join("heartbeat");
    let metadata = match tokio::fs::metadata(&heartbeat).await {
        Ok(metadata) => metadata,
        Err(_) => match tokio::fs::metadata(directory).await {
            Ok(metadata) => metadata,
            Err(_) => return false,
        },
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    modified
        .elapsed()
        .is_ok_and(|age| age > Duration::from_secs(5))
}

async fn start_lock_heartbeat(directory: PathBuf) -> std::io::Result<RefreshLockGuard> {
    let heartbeat = directory.join("heartbeat");
    if let Err(error) = tokio::fs::write(&heartbeat, b"").await {
        let _ = tokio::fs::remove_dir(&directory).await;
        return Err(error);
    }
    let (stop_sender, mut stop_receiver) = oneshot::channel();
    let heartbeat_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    let _ = tokio::fs::write(&heartbeat, b"").await;
                }
                _ = &mut stop_receiver => break,
            }
        }
    });
    Ok(RefreshLockGuard {
        directory: Some(directory),
        heartbeat_stop: Some(stop_sender),
        heartbeat_task: Some(heartbeat_task),
    })
}

async fn remove_lock_directory(directory: &Path) {
    let _ = tokio::fs::remove_file(directory.join("heartbeat")).await;
    let _ = tokio::fs::remove_dir(directory).await;
}

fn test_config_dir_fallback() -> Option<PathBuf> {
    (std::env::var("NODE_ENV").ok().as_deref() == Some("test"))
        .then(|| std::env::var_os("KIMI_CODE_HOME").map(PathBuf::from))
        .flatten()
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
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;

    #[derive(Default)]
    struct MemoryStorage {
        tokens: Mutex<HashMap<String, TokenInfo>>,
    }

    #[tokio::test(flavor = "current_thread")]
    async fn system_runtime_initializes_device_headers_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let factory_calls = Arc::clone(&calls);
        let runtime = SystemOAuthManagerRuntime::new(Some(Arc::new(move || {
            factory_calls.fetch_add(1, Ordering::SeqCst);
            Some(DeviceHeaders::new())
        })));

        futures_util::future::join_all((0..16).map(|_| runtime.device_headers())).await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
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

    struct EnsureRuntime {
        now: f64,
        refreshes: AtomicUsize,
        refresh_results: Mutex<VecDeque<Result<TokenInfo, OAuthError>>>,
        gates: Mutex<VecDeque<Option<Arc<Notify>>>>,
        started: Notify,
        rotation: Option<(Arc<MemoryStorage>, TokenInfo)>,
        sleeps: Mutex<Vec<Duration>>,
    }

    struct ReplaceOnSecondLoadStorage {
        inner: Arc<MemoryStorage>,
        replacement: TokenInfo,
        loads: AtomicUsize,
    }

    #[async_trait]
    impl TokenStorage for ReplaceOnSecondLoadStorage {
        async fn load(&self, name: &str) -> Result<Option<TokenInfo>, TokenStorageError> {
            if self.loads.fetch_add(1, Ordering::SeqCst) == 1 {
                self.inner.save(name, &self.replacement).await?;
            }
            self.inner.load(name).await
        }

        async fn save(&self, name: &str, token: &TokenInfo) -> Result<(), TokenStorageError> {
            self.inner.save(name, token).await
        }

        async fn remove(&self, name: &str) -> Result<(), TokenStorageError> {
            self.inner.remove(name).await
        }

        async fn list(&self) -> Result<Vec<String>, TokenStorageError> {
            self.inner.list().await
        }
    }

    impl EnsureRuntime {
        fn new(results: Vec<Result<TokenInfo, OAuthError>>) -> Self {
            Self {
                now: 1_000_000_000.0,
                refreshes: AtomicUsize::new(0),
                refresh_results: Mutex::new(results.into()),
                gates: Mutex::new(VecDeque::new()),
                started: Notify::new(),
                rotation: None,
                sleeps: Mutex::new(Vec::new()),
            }
        }

        fn with_gates(mut self, gates: Vec<Option<Arc<Notify>>>) -> Self {
            self.gates = Mutex::new(gates.into());
            self
        }

        fn with_rotation(mut self, storage: Arc<MemoryStorage>, token: TokenInfo) -> Self {
            self.rotation = Some((storage, token));
            self
        }
    }

    #[async_trait]
    impl OAuthManagerRuntime for EnsureRuntime {
        fn now_seconds(&self) -> f64 {
            self.now
        }

        async fn sleep(&self, duration: Duration) {
            self.sleeps.lock().expect("sleeps lock").push(duration);
        }

        async fn refresh_token(
            &self,
            _config: &OAuthFlowConfig,
            _refresh_token: &str,
        ) -> Result<TokenInfo, OAuthError> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            let gate = self.gates.lock().expect("gates lock").pop_front();
            if let Some(Some(gate)) = gate {
                gate.notified().await;
            }
            if let Some((storage, token)) = &self.rotation {
                storage
                    .save("kimi-code", token)
                    .await
                    .map_err(|error| OAuthError::new(error.to_string()))?;
            }
            self.refresh_results
                .lock()
                .expect("refresh results lock")
                .pop_front()
                .unwrap_or_else(|| Err(OAuthError::new("no fake refresh result")))
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

    fn expiring_token(access_token: &str, refresh_token: &str) -> TokenInfo {
        TokenInfo {
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.to_owned(),
            expires_at: 1_000_000_100.0,
            scope: "scope".to_owned(),
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

    #[tokio::test]
    async fn ensure_fresh_short_circuits_or_refreshes_and_persists_by_threshold() {
        let storage = Arc::new(MemoryStorage::default());
        storage
            .save("kimi-code", &token("cached"))
            .await
            .expect("store cached token");
        let runtime = Arc::new(EnsureRuntime::new(vec![Ok(token("refreshed"))]));
        let manager = Arc::new(OAuthManager::new(
            config(),
            storage.clone(),
            runtime.clone(),
        ));
        assert_eq!(
            manager.ensure_fresh(false).await.expect("cached token"),
            "cached"
        );
        assert_eq!(runtime.refreshes.load(Ordering::SeqCst), 0);

        storage
            .save("kimi-code", &expiring_token("old", "old-refresh"))
            .await
            .expect("store expiring token");
        assert_eq!(
            manager.ensure_fresh(false).await.expect("refreshed token"),
            "refreshed"
        );
        assert_eq!(runtime.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(
            storage
                .load("kimi-code")
                .await
                .expect("load refreshed")
                .expect("refreshed exists")
                .access_token,
            "refreshed"
        );
    }

    #[tokio::test]
    async fn concurrent_normal_and_forced_refreshes_follow_original_coalescing_rules() {
        let storage = Arc::new(MemoryStorage::default());
        storage
            .save("kimi-code", &expiring_token("old", "old-refresh"))
            .await
            .expect("store expiring token");
        let gate = Arc::new(Notify::new());
        let runtime = Arc::new(
            EnsureRuntime::new(vec![Ok(token("one"))]).with_gates(vec![Some(Arc::clone(&gate))]),
        );
        let manager = Arc::new(OAuthManager::new(
            config(),
            storage.clone(),
            runtime.clone(),
        ));

        let first = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.ensure_fresh(false).await }
        });
        runtime.started.notified().await;
        let second = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.ensure_fresh(false).await }
        });
        gate.notify_one();
        assert_eq!(
            first.await.expect("first task").expect("first result"),
            "one"
        );
        assert_eq!(
            second.await.expect("second task").expect("second result"),
            "one"
        );
        assert_eq!(runtime.refreshes.load(Ordering::SeqCst), 1);

        storage
            .save("kimi-code", &expiring_token("old-2", "old-refresh-2"))
            .await
            .expect("store second expiring token");
        let gate = Arc::new(Notify::new());
        let runtime = Arc::new(
            EnsureRuntime::new(vec![Ok(token("normal")), Ok(token("forced"))])
                .with_gates(vec![Some(Arc::clone(&gate)), None]),
        );
        let manager = Arc::new(OAuthManager::new(config(), storage, runtime.clone()));
        let normal = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.ensure_fresh(false).await }
        });
        runtime.started.notified().await;
        let forced = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.ensure_fresh(true).await }
        });
        gate.notify_one();
        assert_eq!(
            normal.await.expect("normal task").expect("normal result"),
            "normal"
        );
        assert_eq!(
            forced.await.expect("forced task").expect("forced result"),
            "forced"
        );
        assert_eq!(runtime.refreshes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_forced_callers_share_one_forced_rotation() {
        let storage = Arc::new(MemoryStorage::default());
        storage
            .save("kimi-code", &token("old"))
            .await
            .expect("store token");
        let gate = Arc::new(Notify::new());
        let runtime = Arc::new(
            EnsureRuntime::new(vec![Ok(token("forced"))]).with_gates(vec![Some(Arc::clone(&gate))]),
        );
        let manager = Arc::new(OAuthManager::new(config(), storage, runtime.clone()));
        let first = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.ensure_fresh(true).await }
        });
        runtime.started.notified().await;
        let second = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.ensure_fresh(true).await }
        });
        gate.notify_one();
        assert_eq!(
            first.await.expect("first task").expect("first result"),
            "forced"
        );
        assert_eq!(
            second.await.expect("second task").expect("second result"),
            "forced"
        );
        assert_eq!(runtime.refreshes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unauthorized_refresh_tombstones_unrotated_tokens_and_notifies() {
        let storage = Arc::new(MemoryStorage::default());
        storage
            .save("kimi-code", &expiring_token("old", "rejected"))
            .await
            .expect("store rejected token");
        let runtime = Arc::new(EnsureRuntime::new(vec![Err(OAuthError::unauthorized(
            "refresh rejected",
        ))]));
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let observer_outcomes = Arc::clone(&outcomes);
        let manager = Arc::new(
            OAuthManager::new(config(), storage.clone(), runtime.clone()).with_refresh_observer(
                Arc::new(move |outcome| {
                    observer_outcomes
                        .lock()
                        .expect("outcomes lock")
                        .push(outcome);
                }),
            ),
        );
        let error = manager
            .ensure_fresh(false)
            .await
            .expect_err("unauthorized refresh");
        assert_eq!(
            error.oauth().map(OAuthError::kind),
            Some(super::super::errors::OAuthErrorKind::Unauthorized)
        );
        let tombstone = storage
            .load("kimi-code")
            .await
            .expect("load tombstone")
            .expect("tombstone exists");
        assert_eq!(tombstone.access_token, "");
        assert_eq!(tombstone.refresh_token, "");
        assert_eq!(tombstone.scope, "scope");
        assert_eq!(
            *outcomes.lock().expect("outcomes lock"),
            [OAuthRefreshOutcome::Unauthorized]
        );
        assert_eq!(
            *runtime.sleeps.lock().expect("sleeps lock"),
            [Duration::from_millis(100)]
        );
    }

    #[tokio::test]
    async fn peer_rotation_recovers_from_stale_unauthorized_refresh() {
        let storage = Arc::new(MemoryStorage::default());
        storage
            .save("kimi-code", &expiring_token("old", "stale-refresh"))
            .await
            .expect("store stale token");
        let peer = token("peer-access");
        let runtime = Arc::new(
            EnsureRuntime::new(vec![Err(OAuthError::unauthorized("stale"))])
                .with_rotation(storage.clone(), peer.clone()),
        );
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let observer_outcomes = Arc::clone(&outcomes);
        let manager = Arc::new(
            OAuthManager::new(config(), storage.clone(), runtime).with_refresh_observer(Arc::new(
                move |outcome| {
                    observer_outcomes
                        .lock()
                        .expect("outcomes lock")
                        .push(outcome);
                },
            )),
        );
        assert_eq!(
            manager.ensure_fresh(false).await.expect("peer recovery"),
            "peer-access"
        );
        assert_eq!(
            storage
                .load("kimi-code")
                .await
                .expect("load peer")
                .expect("peer exists"),
            peer
        );
        assert_eq!(
            *outcomes.lock().expect("outcomes lock"),
            [OAuthRefreshOutcome::Success]
        );
    }

    #[tokio::test]
    async fn missing_revoked_and_refreshless_tokens_require_login() {
        for stored in [None, Some(token("")), Some(expiring_token("access", ""))] {
            let storage = Arc::new(MemoryStorage::default());
            if let Some(token) = stored {
                storage
                    .save("kimi-code", &token)
                    .await
                    .expect("store invalid token");
            }
            let manager = Arc::new(OAuthManager::new(
                config(),
                storage,
                Arc::new(EnsureRuntime::new(Vec::new())),
            ));
            let error = manager
                .ensure_fresh(false)
                .await
                .expect_err("login required");
            assert_eq!(
                error.oauth().map(OAuthError::kind),
                Some(super::super::errors::OAuthErrorKind::Unauthorized)
            );
        }
    }

    #[tokio::test]
    async fn force_reuses_a_token_changed_during_cross_process_coordination() {
        let inner = Arc::new(MemoryStorage::default());
        inner
            .save("kimi-code", &expiring_token("old", "old-refresh"))
            .await
            .expect("store old token");
        let storage = Arc::new(ReplaceOnSecondLoadStorage {
            inner,
            replacement: token("peer"),
            loads: AtomicUsize::new(0),
        });
        let runtime = Arc::new(EnsureRuntime::new(Vec::new()));
        let manager = Arc::new(OAuthManager::new(config(), storage, runtime.clone()));

        assert_eq!(
            manager.ensure_fresh(true).await.expect("peer token"),
            "peer"
        );
        assert_eq!(runtime.refreshes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn network_errors_retain_the_token_and_observer_panics_are_isolated() {
        let storage = Arc::new(MemoryStorage::default());
        let original = expiring_token("old", "still-valid-refresh");
        storage
            .save("kimi-code", &original)
            .await
            .expect("store original");
        let runtime = Arc::new(EnsureRuntime::new(vec![Err(OAuthError::new(
            "network down",
        ))]));
        let manager = Arc::new(
            OAuthManager::new(config(), storage.clone(), runtime)
                .with_refresh_observer(Arc::new(|_| panic!("observer failure"))),
        );
        let error = manager
            .ensure_fresh(false)
            .await
            .expect_err("network refresh failure");
        assert_eq!(error.to_string(), "network down");
        assert_eq!(
            storage
                .load("kimi-code")
                .await
                .expect("load retained token")
                .expect("retained token exists"),
            original
        );
    }

    #[tokio::test]
    async fn unknown_zero_expiry_never_refreshes_without_force() {
        let storage = Arc::new(MemoryStorage::default());
        let mut unknown_expiry = token("unknown-expiry");
        unknown_expiry.expires_at = 0.0;
        storage
            .save("kimi-code", &unknown_expiry)
            .await
            .expect("store unknown expiry");
        let runtime = Arc::new(EnsureRuntime::new(Vec::new()));
        let manager = Arc::new(OAuthManager::new(config(), storage, runtime.clone()));
        assert_eq!(
            manager.ensure_fresh(false).await.expect("cached token"),
            "unknown-expiry"
        );
        assert_eq!(runtime.refreshes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn refresh_lock_heartbeat_is_cleaned_up_on_release() {
        let root = std::env::temp_dir().join(format!(
            "kimi-code-rs-oauth-lock-test-{}",
            uuid::Uuid::new_v4()
        ));
        let directory = root.join("provider.lock");
        tokio::fs::create_dir_all(&directory)
            .await
            .expect("create lock directory");
        let guard = start_lock_heartbeat(directory.clone())
            .await
            .expect("start heartbeat");
        assert!(directory.join("heartbeat").is_file());
        guard.release().await;
        assert!(!directory.exists());
        tokio::fs::remove_dir(root).await.expect("remove test root");
    }
}
