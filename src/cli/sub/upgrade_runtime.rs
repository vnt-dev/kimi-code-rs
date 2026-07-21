use std::{
    io::{IsTerminal, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::upgrade::{UpgradeError, UpgradeRuntime};
use crate::cli::update::{
    preflight::{UpdatePlatform, install_update},
    prompt::{InstallPromptChoiceValue, InstallPromptOptions, prompt_for_install_choice},
    refresh::refresh_update_cache,
    runtime::{
        SystemForegroundInstallerRuntime, SystemInstallPromptRuntime, SystemInstallSourceRuntime,
        SystemUpdateRefreshRuntime,
    },
    source::detect_install_source,
    types::{InstallSource, UpdateCache},
};

pub trait UpgradeObserver: Send + Sync {
    fn track(&self, event: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError>;
    fn log_info(&self, message: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError>;
    fn log_warn(&self, message: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError>;
}

pub struct SystemUpgradeRuntime {
    refresh: SystemUpdateRefreshRuntime,
    source: SystemInstallSourceRuntime,
    prompt: SystemInstallPromptRuntime,
    installer: SystemForegroundInstallerRuntime,
    observer: Arc<dyn UpgradeObserver>,
    platform: UpdatePlatform,
    interactive: bool,
    stdout: Mutex<Box<dyn Write + Send>>,
    stderr: Mutex<Box<dyn Write + Send>>,
}

impl SystemUpgradeRuntime {
    pub fn new(
        package_root: impl Into<PathBuf>,
        native_install: bool,
        observer: Arc<dyn UpgradeObserver>,
    ) -> Self {
        Self::with_io(
            package_root,
            native_install,
            observer,
            std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            Box::new(std::io::stdout()),
            Box::new(std::io::stderr()),
        )
    }

    pub fn with_io(
        package_root: impl Into<PathBuf>,
        native_install: bool,
        observer: Arc<dyn UpgradeObserver>,
        interactive: bool,
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
    ) -> Self {
        Self {
            refresh: SystemUpdateRefreshRuntime::default(),
            source: SystemInstallSourceRuntime::new(package_root, native_install),
            prompt: SystemInstallPromptRuntime,
            installer: SystemForegroundInstallerRuntime,
            observer,
            platform: UpdatePlatform::current(),
            interactive,
            stdout: Mutex::new(stdout),
            stderr: Mutex::new(stderr),
        }
    }
}

#[async_trait]
impl UpgradeRuntime for SystemUpgradeRuntime {
    // Original:
    //   apps/kimi-code/src/cli/sub/upgrade.ts
    //   createDefaultUpgradeDeps()
    async fn refresh_update_cache(&self) -> Result<UpdateCache, UpgradeError> {
        refresh_update_cache(&self.refresh)
            .await
            .map_err(UpgradeError::new)
    }

    async fn detect_install_source(&self) -> Result<InstallSource, UpgradeError> {
        Ok(detect_install_source(&self.source).await)
    }

    async fn prompt_for_install_choice(
        &self,
        options: InstallPromptOptions,
    ) -> Result<InstallPromptChoiceValue, UpgradeError> {
        Ok(prompt_for_install_choice(&self.prompt, &options).await)
    }

    async fn install_update(
        &self,
        source: InstallSource,
        version: &str,
        platform: UpdatePlatform,
    ) -> Result<(), UpgradeError> {
        install_update(&self.installer, source, version, platform)
            .await
            .map_err(UpgradeError::new)
    }

    fn platform(&self) -> UpdatePlatform {
        self.platform
    }

    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn track(&self, event: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError> {
        self.observer.track(event, properties)
    }

    fn log_info(&self, message: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError> {
        self.observer.log_info(message, properties)
    }

    fn log_warn(&self, message: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError> {
        self.observer.log_warn(message, properties)
    }

    fn write_stdout(&self, text: &str) {
        if let Ok(mut stdout) = self.stdout.lock() {
            let _ = stdout.write_all(text.as_bytes());
        }
    }

    fn write_stderr(&self, text: &str) {
        if let Ok(mut stderr) = self.stderr.lock() {
            let _ = stderr.write_all(text.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::cli::update::source::InstallPlatform;

    #[derive(Default)]
    struct ObserverMock {
        events: Mutex<Vec<(String, Map<String, Value>)>>,
        info: Mutex<Vec<String>>,
        warnings: Mutex<Vec<String>>,
    }

    impl UpgradeObserver for ObserverMock {
        fn track(&self, event: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError> {
            self.events
                .lock()
                .expect("events")
                .push((event.to_owned(), properties.clone()));
            Ok(())
        }

        fn log_info(&self, message: &str, _: &Map<String, Value>) -> Result<(), UpgradeError> {
            self.info.lock().expect("info").push(message.to_owned());
            Ok(())
        }

        fn log_warn(&self, message: &str, _: &Map<String, Value>) -> Result<(), UpgradeError> {
            self.warnings
                .lock()
                .expect("warnings")
                .push(message.to_owned());
            Ok(())
        }
    }

    #[tokio::test]
    async fn composed_runtime_uses_native_detection_and_real_install_validation() {
        let observer = Arc::new(ObserverMock::default());
        let runtime = SystemUpgradeRuntime::with_io(
            "/not/an/npm/package",
            true,
            observer,
            false,
            Box::new(Vec::<u8>::new()),
            Box::new(Vec::<u8>::new()),
        );

        assert_eq!(
            runtime.detect_install_source().await.expect("source"),
            InstallSource::Native
        );
        assert!(!runtime.is_interactive());
        assert_eq!(runtime.platform(), UpdatePlatform::current());
        let error = runtime
            .install_update(InstallSource::Unsupported, "1.2.3", runtime.platform())
            .await
            .expect_err("unsupported source");
        assert!(error.to_string().contains("unsupported install source"));
    }

    #[test]
    fn composed_runtime_forwards_observer_calls_without_changing_payloads() {
        let observer = Arc::new(ObserverMock::default());
        let runtime = SystemUpgradeRuntime::with_io(
            "/package",
            false,
            observer.clone(),
            true,
            Box::new(Vec::<u8>::new()),
            Box::new(Vec::<u8>::new()),
        );
        let properties =
            Map::from_iter([("source".to_owned(), Value::String("native".to_owned()))]);

        runtime.track("event", &properties).expect("track");
        runtime.log_info("info", &properties).expect("info");
        runtime.log_warn("warn", &properties).expect("warn");

        assert_eq!(
            observer.events.lock().expect("events").as_slice(),
            [("event".to_owned(), properties)]
        );
        assert_eq!(observer.info.lock().expect("info").as_slice(), ["info"]);
        assert_eq!(
            observer.warnings.lock().expect("warnings").as_slice(),
            ["warn"]
        );
    }

    #[test]
    fn install_source_runtime_platform_type_remains_available_to_callers() {
        let runtime =
            SystemInstallSourceRuntime::with_platform("/package", false, InstallPlatform::Windows);
        assert_eq!(
            crate::cli::update::source::DetectInstallSourceDeps::platform(&runtime),
            InstallPlatform::Windows
        );
    }
}
