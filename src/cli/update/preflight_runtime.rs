use std::{
    collections::HashMap,
    io::{IsTerminal, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};

use super::{
    background_install::{
        BackgroundInstallError, BackgroundInstallerRuntime, try_start_automatic_background_install,
    },
    cache::read_update_cache,
    install_state::{read_update_install_state, write_update_install_state},
    preflight::{
        RolloutTelemetry, UpdateNoticeError, UpdateNoticeRuntime, UpdatePlatform, install_update,
        show_pending_background_install_notice,
    },
    prompt::{InstallPromptChoiceValue, InstallPromptOptions, prompt_for_install_choice},
    refresh::refresh_update_cache,
    rollout::{append_rollout_decision_log, resolve_update_device_id},
    run_preflight::{UpdatePreflightError, UpdatePreflightRuntime},
    runtime::{
        BackgroundInstallObserver, SystemBackgroundInstallerRuntime,
        SystemForegroundInstallerRuntime, SystemInstallPromptRuntime, SystemInstallSourceRuntime,
        SystemUpdateRefreshRuntime,
    },
    source::detect_install_source,
    types::{InstallSource, UpdateCache, UpdateInstallState, UpdateTarget},
};

pub trait UpdatePreflightObserver: Send + Sync {
    fn track(
        &self,
        event: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), UpdatePreflightError>;
    fn log_info(
        &self,
        message: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), UpdatePreflightError>;
    fn log_warn(
        &self,
        message: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), UpdatePreflightError>;
}

struct BackgroundObserverAdapter {
    observer: Arc<dyn UpdatePreflightObserver>,
}

impl BackgroundInstallObserver for BackgroundObserverAdapter {
    fn track(
        &self,
        event: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), BackgroundInstallError> {
        self.observer
            .track(event, properties)
            .map_err(BackgroundInstallError::new)
    }

    fn log_info(
        &self,
        message: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), BackgroundInstallError> {
        self.observer
            .log_info(message, properties)
            .map_err(BackgroundInstallError::new)
    }

    fn log_warn(
        &self,
        message: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), BackgroundInstallError> {
        self.observer
            .log_warn(message, properties)
            .map_err(BackgroundInstallError::new)
    }
}

pub struct SystemUpdatePreflightRuntime {
    refresh: SystemUpdateRefreshRuntime,
    source: SystemInstallSourceRuntime,
    prompt: SystemInstallPromptRuntime,
    foreground_installer: SystemForegroundInstallerRuntime,
    background_installer: Arc<SystemBackgroundInstallerRuntime>,
    observer: Arc<dyn UpdatePreflightObserver>,
    platform: UpdatePlatform,
    interactive: bool,
    environment: HashMap<String, String>,
    stdout: Mutex<Box<dyn Write + Send>>,
    stderr: Mutex<Box<dyn Write + Send>>,
}

impl SystemUpdatePreflightRuntime {
    pub fn new(
        package_root: impl Into<PathBuf>,
        native_install: bool,
        observer: Arc<dyn UpdatePreflightObserver>,
        interactive_override: Option<bool>,
    ) -> Self {
        Self::with_io(
            package_root,
            native_install,
            observer,
            interactive_override.unwrap_or_else(|| {
                std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
            }),
            std::env::vars().collect(),
            Box::new(std::io::stdout()),
            Box::new(std::io::stderr()),
        )
    }

    pub fn with_io(
        package_root: impl Into<PathBuf>,
        native_install: bool,
        observer: Arc<dyn UpdatePreflightObserver>,
        interactive: bool,
        environment: HashMap<String, String>,
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
    ) -> Self {
        let background_observer = Arc::new(BackgroundObserverAdapter {
            observer: Arc::clone(&observer),
        });
        Self {
            refresh: SystemUpdateRefreshRuntime::default(),
            source: SystemInstallSourceRuntime::new(package_root, native_install),
            prompt: SystemInstallPromptRuntime,
            foreground_installer: SystemForegroundInstallerRuntime,
            background_installer: Arc::new(SystemBackgroundInstallerRuntime::new(
                background_observer,
            )),
            observer,
            platform: UpdatePlatform::current(),
            interactive,
            environment,
            stdout: Mutex::new(stdout),
            stderr: Mutex::new(stderr),
        }
    }

    fn write_stdout_text(&self, text: &str) {
        if let Ok(mut stdout) = self.stdout.lock() {
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
        }
    }

    fn write_stderr_text(&self, text: &str) {
        if let Ok(mut stderr) = self.stderr.lock() {
            let _ = stderr.write_all(text.as_bytes());
            let _ = stderr.flush();
        }
    }
}

#[async_trait]
impl UpdateNoticeRuntime for SystemUpdatePreflightRuntime {
    fn now_iso(&self) -> String {
        DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    fn write_stdout(&self, text: &str) {
        self.write_stdout_text(text);
    }

    fn track(&self, event: &str, properties: &Map<String, Value>) -> Result<(), UpdateNoticeError> {
        self.observer
            .track(event, properties)
            .map_err(UpdateNoticeError::new)
    }

    fn log_info(
        &self,
        message: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), UpdateNoticeError> {
        self.observer
            .log_info(message, properties)
            .map_err(UpdateNoticeError::new)
    }

    async fn write_install_state(
        &self,
        state: &UpdateInstallState,
    ) -> Result<(), UpdateNoticeError> {
        write_update_install_state(state)
            .await
            .map_err(UpdateNoticeError::new)
    }
}

#[async_trait]
impl UpdatePreflightRuntime for SystemUpdatePreflightRuntime {
    fn environment(&self) -> HashMap<String, String> {
        self.environment.clone()
    }

    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn device_id(&self) -> String {
        resolve_update_device_id()
    }

    fn platform(&self) -> UpdatePlatform {
        self.platform
    }

    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(SystemTime::now())
    }

    async fn read_install_state(&self) -> Result<UpdateInstallState, UpdatePreflightError> {
        Ok(read_update_install_state().await)
    }

    async fn show_pending_background_install_notice(
        &self,
        state: &UpdateInstallState,
        current_version: &str,
    ) -> Result<UpdateInstallState, UpdatePreflightError> {
        Ok(show_pending_background_install_notice(self, state, current_version).await)
    }

    async fn read_update_cache(&self) -> Result<UpdateCache, UpdatePreflightError> {
        Ok(read_update_cache().await)
    }

    async fn refresh_update_cache(&self) -> Result<UpdateCache, UpdatePreflightError> {
        refresh_update_cache(&self.refresh)
            .await
            .map_err(UpdatePreflightError::new)
    }

    async fn detect_install_source(&self) -> Result<InstallSource, UpdatePreflightError> {
        Ok(detect_install_source(&self.source).await)
    }

    async fn try_start_automatic_background_install(
        &self,
        install_state: &UpdateInstallState,
        current_version: &str,
        target: &UpdateTarget,
        source: InstallSource,
        rollout: &RolloutTelemetry,
    ) -> bool {
        let runtime: Arc<dyn BackgroundInstallerRuntime> = self.background_installer.clone();
        try_start_automatic_background_install(
            runtime,
            install_state,
            current_version,
            target,
            source,
            self.platform,
            rollout,
        )
        .await
    }

    async fn prompt_for_install_choice(
        &self,
        options: &InstallPromptOptions,
    ) -> Result<InstallPromptChoiceValue, UpdatePreflightError> {
        Ok(prompt_for_install_choice(&self.prompt, options).await)
    }

    async fn install_update(
        &self,
        source: InstallSource,
        version: &str,
    ) -> Result<(), UpdatePreflightError> {
        install_update(&self.foreground_installer, source, version, self.platform)
            .await
            .map_err(UpdatePreflightError::new)
    }

    fn append_rollout_decision_log(&self, entry: Map<String, Value>) {
        tokio::spawn(async move {
            append_rollout_decision_log(&entry).await;
        });
    }

    fn track(&self, event: &str, properties: &Map<String, Value>) {
        let _ = self.observer.track(event, properties);
    }

    fn write_stdout(&self, text: &str) {
        self.write_stdout_text(text);
    }

    fn write_stderr(&self, text: &str) {
        self.write_stderr_text(text);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::cli::update::{
        run_preflight::run_update_preflight,
        types::{UpdatePreflightResult, empty_update_install_state},
    };

    #[derive(Default)]
    struct ObserverMock {
        events: Mutex<Vec<String>>,
        info: Mutex<Vec<String>>,
        warnings: Mutex<Vec<String>>,
    }

    impl UpdatePreflightObserver for ObserverMock {
        fn track(&self, event: &str, _: &Map<String, Value>) -> Result<(), UpdatePreflightError> {
            self.events.lock().expect("events").push(event.to_owned());
            Ok(())
        }

        fn log_info(
            &self,
            message: &str,
            _: &Map<String, Value>,
        ) -> Result<(), UpdatePreflightError> {
            self.info.lock().expect("info").push(message.to_owned());
            Ok(())
        }

        fn log_warn(
            &self,
            message: &str,
            _: &Map<String, Value>,
        ) -> Result<(), UpdatePreflightError> {
            self.warnings
                .lock()
                .expect("warnings")
                .push(message.to_owned());
            Ok(())
        }
    }

    fn runtime(
        observer: Arc<ObserverMock>,
        environment: HashMap<String, String>,
    ) -> SystemUpdatePreflightRuntime {
        SystemUpdatePreflightRuntime::with_io(
            "/not/an/npm/package",
            true,
            observer,
            false,
            environment,
            Box::new(Vec::<u8>::new()),
            Box::new(Vec::<u8>::new()),
        )
    }

    #[tokio::test]
    async fn composed_runtime_short_circuits_disabled_preflight_without_side_effects() {
        let observer = Arc::new(ObserverMock::default());
        let runtime = Arc::new(runtime(
            observer.clone(),
            HashMap::from([("KIMI_CODE_NO_AUTO_UPDATE".to_owned(), "true".to_owned())]),
        ));

        assert_eq!(
            run_update_preflight(runtime, "0.4.0").await,
            UpdatePreflightResult::Continue
        );
        assert!(observer.events.lock().expect("events").is_empty());
        assert!(observer.info.lock().expect("info").is_empty());
        assert!(observer.warnings.lock().expect("warnings").is_empty());
    }

    #[tokio::test]
    async fn composed_runtime_reuses_notice_source_and_install_boundaries() {
        let observer = Arc::new(ObserverMock::default());
        let runtime = runtime(observer, HashMap::new());
        assert_eq!(
            runtime.detect_install_source().await.expect("source"),
            InstallSource::Native
        );
        assert_eq!(
            runtime
                .show_pending_background_install_notice(&empty_update_install_state(), "0.4.0",)
                .await
                .expect("unchanged notice"),
            empty_update_install_state()
        );
        assert!(
            runtime
                .install_update(InstallSource::Unsupported, "0.5.0")
                .await
                .expect_err("unsupported install")
                .to_string()
                .contains("unsupported install source")
        );
    }

    #[test]
    fn notice_and_background_adapters_forward_the_shared_observer() {
        let observer = Arc::new(ObserverMock::default());
        let runtime = runtime(observer.clone(), HashMap::new());
        let properties = Map::new();

        UpdateNoticeRuntime::track(&runtime, "notice", &properties).expect("notice event");
        runtime
            .background_installer
            .track("background", &properties)
            .expect("background event");
        runtime
            .background_installer
            .log_info("started", &properties)
            .expect("background info");
        runtime
            .background_installer
            .log_warn("failed", &properties)
            .expect("background warning");

        assert_eq!(
            observer.events.lock().expect("events").as_slice(),
            ["notice", "background"]
        );
        assert_eq!(observer.info.lock().expect("info").as_slice(), ["started"]);
        assert_eq!(
            observer.warnings.lock().expect("warnings").as_slice(),
            ["failed"]
        );
    }
}
