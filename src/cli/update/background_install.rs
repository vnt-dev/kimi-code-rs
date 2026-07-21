use std::{error::Error, fmt, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;

use super::{
    preflight::{
        AUTO_INSTALL_FAILURE_PROMPT_THRESHOLD, RolloutTelemetry, SpawnUpdateRequest,
        UpdatePlatform, can_auto_install, failure_attempts_for, has_fresh_active_install_at,
        spawn_for_source,
    },
    types::{
        InstallSource, UpdateInstallActive, UpdateInstallFailure, UpdateInstallState,
        UpdateInstallSuccess, UpdateTarget,
    },
};

#[derive(Debug)]
pub struct BackgroundInstallError(Box<dyn Error + Send + Sync>);

impl BackgroundInstallError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }

    fn message(message: impl Into<String>) -> Self {
        Self(Box::new(BackgroundInstallMessage(message.into())))
    }
}

impl fmt::Display for BackgroundInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for BackgroundInstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Debug)]
struct BackgroundInstallMessage(String);

impl fmt::Display for BackgroundInstallMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for BackgroundInstallMessage {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundInstallLock {
    pub file_path: PathBuf,
}

#[async_trait]
pub trait BackgroundInstallerRuntime: Send + Sync + 'static {
    async fn try_acquire_lock(
        &self,
        version: &str,
    ) -> Result<Option<BackgroundInstallLock>, BackgroundInstallError>;
    async fn release_lock(&self, lock: BackgroundInstallLock)
    -> Result<(), BackgroundInstallError>;
    async fn read_install_state(&self) -> Result<UpdateInstallState, BackgroundInstallError>;
    async fn write_install_state(
        &self,
        state: &UpdateInstallState,
    ) -> Result<(), BackgroundInstallError>;
    async fn should_auto_install(&self) -> Result<bool, BackgroundInstallError>;
    async fn spawn_background(
        &self,
        request: SpawnUpdateRequest,
    ) -> Result<oneshot::Receiver<bool>, BackgroundInstallError>;
    fn now_iso(&self) -> String;
    fn now_millis(&self) -> i64;
    fn track(
        &self,
        event: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), BackgroundInstallError>;
    fn log_info(
        &self,
        message: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), BackgroundInstallError>;
    fn log_warn(
        &self,
        message: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), BackgroundInstallError>;
}

// Original:
//   apps/kimi-code/src/cli/update/preflight.ts
//   tryStartAutomaticBackgroundInstall()
pub async fn try_start_automatic_background_install(
    runtime: Arc<dyn BackgroundInstallerRuntime>,
    install_state: &UpdateInstallState,
    current_version: &str,
    target: &UpdateTarget,
    source: InstallSource,
    platform: UpdatePlatform,
    rollout_telemetry: &RolloutTelemetry,
) -> bool {
    let source_can_auto_install = can_auto_install(source, platform);
    let auto_install_updates = if source_can_auto_install {
        runtime.should_auto_install().await.unwrap_or(true)
    } else {
        false
    };
    if !auto_install_updates || !source_can_auto_install {
        return false;
    }
    if failure_attempts_for(install_state, target) >= AUTO_INSTALL_FAILURE_PROMPT_THRESHOLD {
        return false;
    }
    if !has_fresh_active_install_at(install_state, target, runtime.now_millis()) {
        let _ = start_background_install(
            runtime,
            install_state,
            current_version,
            target,
            source,
            platform,
            rollout_telemetry,
        )
        .await;
    }
    true
}

// Original:
//   apps/kimi-code/src/cli/update/preflight.ts
//   startBackgroundInstall()
pub async fn start_background_install(
    runtime: Arc<dyn BackgroundInstallerRuntime>,
    state: &UpdateInstallState,
    current_version: &str,
    target: &UpdateTarget,
    source: InstallSource,
    platform: UpdatePlatform,
    rollout_telemetry: &RolloutTelemetry,
) -> Result<(), BackgroundInstallError> {
    let Some(lock) = runtime.try_acquire_lock(&target.version).await? else {
        return Ok(());
    };
    let result = start_background_install_while_locked(
        Arc::clone(&runtime),
        state,
        current_version,
        target,
        source,
        platform,
        rollout_telemetry,
    )
    .await;
    let _ = runtime.release_lock(lock).await;
    result
}

async fn start_background_install_while_locked(
    runtime: Arc<dyn BackgroundInstallerRuntime>,
    state: &UpdateInstallState,
    current_version: &str,
    target: &UpdateTarget,
    source: InstallSource,
    platform: UpdatePlatform,
    rollout_telemetry: &RolloutTelemetry,
) -> Result<(), BackgroundInstallError> {
    let fresh_state = runtime
        .read_install_state()
        .await
        .unwrap_or_else(|_| state.clone());
    if has_fresh_active_install_at(&fresh_state, target, runtime.now_millis())
        || failure_attempts_for(&fresh_state, target) >= AUTO_INSTALL_FAILURE_PROMPT_THRESHOLD
    {
        return Ok(());
    }
    let started_state = UpdateInstallState {
        active: Some(UpdateInstallActive {
            version: target.version.clone(),
            source,
            started_at: runtime.now_iso(),
        }),
        ..fresh_state
    };
    runtime.write_install_state(&started_state).await?;
    record_started(
        runtime.as_ref(),
        current_version,
        target,
        source,
        rollout_telemetry,
    );

    let spawn = spawn_for_source(source, &target.version, platform)
        .map_err(|error| BackgroundInstallError::message(error.to_string()))?;
    let request = SpawnUpdateRequest {
        command: spawn.command,
        arguments: spawn.arguments,
        inherit_stdio: false,
        shell: platform == UpdatePlatform::Windows,
    };
    match runtime.spawn_background(request).await {
        Ok(completion) => {
            let runtime = Arc::clone(&runtime);
            let target = target.clone();
            tokio::spawn(async move {
                let succeeded = completion.await.unwrap_or(false);
                finish_background_install(
                    runtime.as_ref(),
                    &started_state,
                    &target,
                    source,
                    succeeded,
                )
                .await;
            });
        }
        Err(_) => {
            finish_background_install(runtime.as_ref(), &started_state, target, source, false)
                .await;
        }
    }
    Ok(())
}

async fn finish_background_install(
    runtime: &dyn BackgroundInstallerRuntime,
    started_state: &UpdateInstallState,
    target: &UpdateTarget,
    source: InstallSource,
    succeeded: bool,
) {
    let attempts = failure_attempts_for(started_state, target) + 1;
    let next_state = if succeeded {
        UpdateInstallState {
            active: None,
            last_failure: None,
            last_success: Some(UpdateInstallSuccess {
                version: target.version.clone(),
                installed_at: runtime.now_iso(),
                notified_at: None,
            }),
        }
    } else {
        UpdateInstallState {
            active: None,
            last_failure: Some(UpdateInstallFailure {
                version: target.version.clone(),
                failed_at: runtime.now_iso(),
                attempts,
            }),
            last_success: started_state.last_success.clone(),
        }
    };
    let _ = runtime.write_install_state(&next_state).await;
    if succeeded {
        record_event(
            runtime,
            "update_background_install_succeeded",
            "background update install succeeded",
            object([
                ("target_version", json!(target.version)),
                ("source", json!(source.as_str())),
            ]),
            object([
                ("targetVersion", json!(target.version)),
                ("source", json!(source.as_str())),
            ]),
            false,
        );
    } else {
        record_event(
            runtime,
            "update_background_install_failed",
            "background update install failed",
            object([
                ("target_version", json!(target.version)),
                ("source", json!(source.as_str())),
                ("attempts", json!(attempts)),
            ]),
            object([
                ("targetVersion", json!(target.version)),
                ("source", json!(source.as_str())),
                ("attempts", json!(attempts)),
            ]),
            true,
        );
    }
}

fn record_started(
    runtime: &dyn BackgroundInstallerRuntime,
    current_version: &str,
    target: &UpdateTarget,
    source: InstallSource,
    rollout: &RolloutTelemetry,
) {
    let telemetry = object([
        ("current_version", json!(current_version)),
        ("target_version", json!(target.version)),
        ("source", json!(source.as_str())),
        ("rollout_bucket", json!(rollout.rollout_bucket)),
        (
            "rollout_delay_seconds",
            json!(rollout.rollout_delay_seconds),
        ),
        (
            "rollout_from_manifest",
            json!(rollout.rollout_from_manifest),
        ),
        ("rollout_bypassed", json!(rollout.rollout_bypassed)),
    ]);
    let log = object([
        ("currentVersion", json!(current_version)),
        ("targetVersion", json!(target.version)),
        ("source", json!(source.as_str())),
    ]);
    record_event(
        runtime,
        "update_background_install_started",
        "background update install started",
        telemetry,
        log,
        false,
    );
}

fn record_event(
    runtime: &dyn BackgroundInstallerRuntime,
    event: &str,
    message: &str,
    telemetry: Map<String, Value>,
    log: Map<String, Value>,
    warning: bool,
) {
    let _ = runtime.track(event, &telemetry);
    if warning {
        let _ = runtime.log_warn(message, &log);
    } else {
        let _ = runtime.log_info(message, &log);
    }
}

fn object<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::cli::update::types::empty_update_install_state;

    struct RuntimeMock {
        lock_available: bool,
        auto_install: bool,
        fresh_state: Mutex<Result<UpdateInstallState, &'static str>>,
        completion_senders: Mutex<VecDeque<oneshot::Sender<bool>>>,
        requests: Mutex<Vec<SpawnUpdateRequest>>,
        writes: Mutex<Vec<UpdateInstallState>>,
        events: Mutex<Vec<(String, Map<String, Value>)>>,
        released: Mutex<usize>,
    }

    impl RuntimeMock {
        fn new() -> Self {
            Self {
                lock_available: true,
                auto_install: true,
                fresh_state: Mutex::new(Ok(empty_update_install_state())),
                completion_senders: Mutex::new(VecDeque::new()),
                requests: Mutex::new(Vec::new()),
                writes: Mutex::new(Vec::new()),
                events: Mutex::new(Vec::new()),
                released: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl BackgroundInstallerRuntime for RuntimeMock {
        async fn try_acquire_lock(
            &self,
            _: &str,
        ) -> Result<Option<BackgroundInstallLock>, BackgroundInstallError> {
            Ok(self.lock_available.then(|| BackgroundInstallLock {
                file_path: PathBuf::from("lock"),
            }))
        }

        async fn release_lock(
            &self,
            _: BackgroundInstallLock,
        ) -> Result<(), BackgroundInstallError> {
            *self.released.lock().expect("released") += 1;
            Ok(())
        }

        async fn read_install_state(&self) -> Result<UpdateInstallState, BackgroundInstallError> {
            self.fresh_state
                .lock()
                .expect("fresh state")
                .clone()
                .map_err(|message| BackgroundInstallError::new(std::io::Error::other(message)))
        }

        async fn write_install_state(
            &self,
            state: &UpdateInstallState,
        ) -> Result<(), BackgroundInstallError> {
            self.writes.lock().expect("writes").push(state.clone());
            Ok(())
        }

        async fn should_auto_install(&self) -> Result<bool, BackgroundInstallError> {
            Ok(self.auto_install)
        }

        async fn spawn_background(
            &self,
            request: SpawnUpdateRequest,
        ) -> Result<oneshot::Receiver<bool>, BackgroundInstallError> {
            self.requests.lock().expect("requests").push(request);
            let (sender, receiver) = oneshot::channel();
            self.completion_senders
                .lock()
                .expect("senders")
                .push_back(sender);
            Ok(receiver)
        }

        fn now_iso(&self) -> String {
            "2026-07-21T12:00:00.000Z".to_owned()
        }

        fn now_millis(&self) -> i64 {
            Utc.with_ymd_and_hms(2026, 7, 21, 12, 0, 0)
                .single()
                .expect("time")
                .timestamp_millis()
        }

        fn track(
            &self,
            event: &str,
            properties: &Map<String, Value>,
        ) -> Result<(), BackgroundInstallError> {
            self.events
                .lock()
                .expect("events")
                .push((event.to_owned(), properties.clone()));
            Ok(())
        }

        fn log_info(&self, _: &str, _: &Map<String, Value>) -> Result<(), BackgroundInstallError> {
            Ok(())
        }

        fn log_warn(&self, _: &str, _: &Map<String, Value>) -> Result<(), BackgroundInstallError> {
            Ok(())
        }
    }

    fn target() -> UpdateTarget {
        UpdateTarget {
            version: "0.5.0".to_owned(),
        }
    }

    fn rollout() -> RolloutTelemetry {
        RolloutTelemetry {
            rollout_bucket: 42,
            rollout_delay_seconds: 3_600,
            rollout_from_manifest: true,
            rollout_bypassed: false,
        }
    }

    #[tokio::test]
    async fn starts_detached_install_releases_lock_and_records_success() {
        let runtime = Arc::new(RuntimeMock::new());
        assert!(
            try_start_automatic_background_install(
                runtime.clone(),
                &empty_update_install_state(),
                "0.4.0",
                &target(),
                InstallSource::NpmGlobal,
                UpdatePlatform::Other,
                &rollout(),
            )
            .await
        );
        assert_eq!(*runtime.released.lock().expect("released"), 1);
        assert_eq!(runtime.requests.lock().expect("requests").len(), 1);
        assert!(runtime.writes.lock().expect("writes")[0].active.is_some());
        assert_eq!(
            runtime.events.lock().expect("events")[0].0,
            "update_background_install_started"
        );

        runtime
            .completion_senders
            .lock()
            .expect("senders")
            .pop_front()
            .expect("sender")
            .send(true)
            .expect("completion");
        tokio::task::yield_now().await;

        let writes = runtime.writes.lock().expect("writes");
        assert_eq!(writes.len(), 2);
        assert_eq!(
            writes[1].last_success.as_ref().expect("success").version,
            "0.5.0"
        );
        assert_eq!(
            runtime.events.lock().expect("events")[1].0,
            "update_background_install_succeeded"
        );
    }

    #[tokio::test]
    async fn failed_completion_increments_same_version_attempts() {
        let runtime = Arc::new(RuntimeMock::new());
        let state = UpdateInstallState {
            active: None,
            last_failure: Some(UpdateInstallFailure {
                version: "0.5.0".to_owned(),
                failed_at: "earlier".to_owned(),
                attempts: 1,
            }),
            last_success: None,
        };
        *runtime.fresh_state.lock().expect("fresh state") = Ok(state.clone());

        start_background_install(
            runtime.clone(),
            &state,
            "0.4.0",
            &target(),
            InstallSource::NpmGlobal,
            UpdatePlatform::Other,
            &rollout(),
        )
        .await
        .expect("start");
        runtime
            .completion_senders
            .lock()
            .expect("senders")
            .pop_front()
            .expect("sender")
            .send(false)
            .expect("completion");
        tokio::task::yield_now().await;

        let writes = runtime.writes.lock().expect("writes");
        assert_eq!(
            writes[1].last_failure.as_ref().expect("failure").attempts,
            2
        );
        assert_eq!(
            runtime.events.lock().expect("events")[1].0,
            "update_background_install_failed"
        );
    }

    #[tokio::test]
    async fn disabled_unsupported_threshold_and_fresh_active_do_not_spawn() {
        let mut disabled = RuntimeMock::new();
        disabled.auto_install = false;
        let disabled = Arc::new(disabled);
        assert!(
            !try_start_automatic_background_install(
                disabled.clone(),
                &empty_update_install_state(),
                "0.4.0",
                &target(),
                InstallSource::NpmGlobal,
                UpdatePlatform::Other,
                &rollout(),
            )
            .await
        );

        let unsupported = Arc::new(RuntimeMock::new());
        assert!(
            !try_start_automatic_background_install(
                unsupported.clone(),
                &empty_update_install_state(),
                "0.4.0",
                &target(),
                InstallSource::Homebrew,
                UpdatePlatform::Other,
                &rollout(),
            )
            .await
        );

        let threshold = UpdateInstallState {
            active: None,
            last_failure: Some(UpdateInstallFailure {
                version: "0.5.0".to_owned(),
                failed_at: "earlier".to_owned(),
                attempts: AUTO_INSTALL_FAILURE_PROMPT_THRESHOLD,
            }),
            last_success: None,
        };
        let runtime = Arc::new(RuntimeMock::new());
        assert!(
            !try_start_automatic_background_install(
                runtime.clone(),
                &threshold,
                "0.4.0",
                &target(),
                InstallSource::NpmGlobal,
                UpdatePlatform::Other,
                &rollout(),
            )
            .await
        );
        assert!(runtime.requests.lock().expect("requests").is_empty());
    }

    #[tokio::test]
    async fn lock_contention_and_fresh_disk_state_release_without_spawn() {
        let mut contention = RuntimeMock::new();
        contention.lock_available = false;
        let contention = Arc::new(contention);
        start_background_install(
            contention.clone(),
            &empty_update_install_state(),
            "0.4.0",
            &target(),
            InstallSource::NpmGlobal,
            UpdatePlatform::Other,
            &rollout(),
        )
        .await
        .expect("contention");
        assert_eq!(*contention.released.lock().expect("released"), 0);

        let runtime = Arc::new(RuntimeMock::new());
        *runtime.fresh_state.lock().expect("fresh state") = Ok(UpdateInstallState {
            active: Some(UpdateInstallActive {
                version: "0.5.0".to_owned(),
                source: InstallSource::NpmGlobal,
                started_at: "2026-07-21T11:59:00.000Z".to_owned(),
            }),
            last_failure: None,
            last_success: None,
        });
        start_background_install(
            runtime.clone(),
            &empty_update_install_state(),
            "0.4.0",
            &target(),
            InstallSource::NpmGlobal,
            UpdatePlatform::Other,
            &rollout(),
        )
        .await
        .expect("fresh");
        assert_eq!(*runtime.released.lock().expect("released"), 1);
        assert!(runtime.requests.lock().expect("requests").is_empty());
    }
}
