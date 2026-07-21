use std::{error::Error, fmt};

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use crate::cli::update::{
    preflight::{
        UpdatePlatform, can_auto_install, install_command_for, render_install_success_message,
        render_manual_update_message,
    },
    prompt::{InstallPromptChoiceValue, InstallPromptOptions},
    select::select_update_target,
    types::{InstallSource, NPM_PACKAGE_NAME, UpdateCache},
};

#[derive(Debug)]
pub struct UpgradeError(Box<dyn Error + Send + Sync>);

impl UpgradeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for UpgradeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for UpgradeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait UpgradeRuntime: Send + Sync {
    async fn refresh_update_cache(&self) -> Result<UpdateCache, UpgradeError>;
    async fn detect_install_source(&self) -> Result<InstallSource, UpgradeError>;
    async fn prompt_for_install_choice(
        &self,
        options: InstallPromptOptions,
    ) -> Result<InstallPromptChoiceValue, UpgradeError>;
    async fn install_update(
        &self,
        source: InstallSource,
        version: &str,
        platform: UpdatePlatform,
    ) -> Result<(), UpgradeError>;
    fn platform(&self) -> UpdatePlatform;
    fn is_interactive(&self) -> bool;
    fn track(&self, event: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError>;
    fn log_info(&self, message: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError>;
    fn log_warn(&self, message: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError>;
    fn write_stdout(&self, text: &str);
    fn write_stderr(&self, text: &str);
}

// Original:
//   apps/kimi-code/src/cli/sub/upgrade.ts
//   handleUpgrade()
pub async fn handle_upgrade(
    runtime: &dyn UpgradeRuntime,
    current_version: &str,
) -> Result<i32, UpgradeError> {
    let cache = match runtime.refresh_update_cache().await {
        Ok(cache) => cache,
        Err(error) => {
            let reason = error.to_string();
            track(
                runtime,
                "upgrade_command_failed",
                object([
                    ("current_version", json!(current_version)),
                    ("stage", json!("refresh")),
                    ("reason", json!(reason)),
                ]),
            );
            log_warn(
                runtime,
                "manual upgrade check failed",
                object([
                    ("currentVersion", json!(current_version)),
                    ("error", json!(error.to_string())),
                ]),
            );
            runtime.write_stderr(&format!("error: failed to check for updates: {error}\n"));
            return Ok(1);
        }
    };

    let Some(target) = select_update_target(current_version, cache.latest.as_deref()) else {
        track(
            runtime,
            "upgrade_command_no_update",
            object([("current_version", json!(current_version))]),
        );
        log_info(
            runtime,
            "manual upgrade no update",
            object([("currentVersion", json!(current_version))]),
        );
        runtime.write_stdout(&format!(
            "Kimi Code is already up to date ({}).\n",
            format_display_version(current_version)
        ));
        return Ok(0);
    };

    let source = runtime
        .detect_install_source()
        .await
        .unwrap_or(InstallSource::Unsupported);
    let platform = runtime.platform();
    let install_command = install_command_for(source, &target.version, platform);
    let telemetry_properties = || {
        object([
            ("current_version", json!(current_version)),
            ("target_version", json!(target.version)),
            ("source", json!(source.as_str())),
        ])
    };
    let log_properties = || {
        object([
            ("currentVersion", json!(current_version)),
            ("targetVersion", json!(target.version)),
            ("source", json!(source.as_str())),
        ])
    };
    if !can_auto_install(source, platform) || !runtime.is_interactive() {
        track(
            runtime,
            "upgrade_command_manual_command",
            telemetry_properties(),
        );
        log_info(runtime, "manual upgrade command shown", log_properties());
        runtime.write_stdout(&render_manual_update_message(
            current_version,
            &target,
            source,
            &install_command,
        ));
        return Ok(0);
    }

    track(runtime, "upgrade_command_prompted", telemetry_properties());
    log_info(runtime, "manual upgrade prompted", log_properties());
    let choice = runtime
        .prompt_for_install_choice(InstallPromptOptions {
            current_version: current_version.to_owned(),
            target: target.clone(),
            install_command,
            install_source: source,
        })
        .await?;
    if choice == InstallPromptChoiceValue::Skip {
        track(runtime, "upgrade_command_skipped", telemetry_properties());
        log_info(runtime, "manual upgrade skipped", log_properties());
        return Ok(0);
    }

    track(
        runtime,
        "upgrade_command_install_selected",
        telemetry_properties(),
    );
    match runtime
        .install_update(source, &target.version, platform)
        .await
    {
        Ok(()) => {
            track(runtime, "upgrade_command_succeeded", telemetry_properties());
            log_info(
                runtime,
                "manual upgrade install succeeded",
                log_properties(),
            );
            runtime.write_stdout(&render_install_success_message(&target));
            Ok(0)
        }
        Err(error) => {
            let mut properties = telemetry_properties();
            properties.insert("stage".to_owned(), json!("install"));
            properties.insert("reason".to_owned(), json!(error.to_string()));
            track(runtime, "upgrade_command_failed", properties);
            let mut properties = log_properties();
            properties.insert("error".to_owned(), json!(error.to_string()));
            log_warn(runtime, "manual upgrade install failed", properties);
            runtime.write_stderr(&format!(
                "warning: failed to install {NPM_PACKAGE_NAME}@{}: {error}\n",
                target.version
            ));
            Ok(1)
        }
    }
}

fn format_display_version(version: &str) -> String {
    if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    }
}

fn object<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn track(runtime: &dyn UpgradeRuntime, event: &str, properties: Map<String, Value>) {
    let _ = runtime.track(event, &properties);
}

fn log_info(runtime: &dyn UpgradeRuntime, message: &str, properties: Map<String, Value>) {
    let _ = runtime.log_info(message, &properties);
}

fn log_warn(runtime: &dyn UpgradeRuntime, message: &str, properties: Map<String, Value>) {
    let _ = runtime.log_warn(message, &properties);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::cli::update::types::{UpdateCacheSource, empty_update_cache};

    use super::*;

    struct RuntimeMock {
        cache_error: bool,
        latest: Option<String>,
        source: Result<InstallSource, &'static str>,
        interactive: bool,
        choice: InstallPromptChoiceValue,
        prompt_error: bool,
        install_error: bool,
        prompts: Mutex<Vec<InstallPromptOptions>>,
        installs: Mutex<Vec<(InstallSource, String, UpdatePlatform)>>,
        events: Mutex<Vec<String>>,
        info_logs: Mutex<Vec<Map<String, Value>>>,
        warn_logs: Mutex<Vec<Map<String, Value>>>,
        stdout: Mutex<String>,
        stderr: Mutex<String>,
    }

    impl RuntimeMock {
        fn new() -> Self {
            Self {
                cache_error: false,
                latest: Some("0.5.0".to_owned()),
                source: Ok(InstallSource::NpmGlobal),
                interactive: true,
                choice: InstallPromptChoiceValue::Install,
                prompt_error: false,
                install_error: false,
                prompts: Mutex::new(Vec::new()),
                installs: Mutex::new(Vec::new()),
                events: Mutex::new(Vec::new()),
                info_logs: Mutex::new(Vec::new()),
                warn_logs: Mutex::new(Vec::new()),
                stdout: Mutex::new(String::new()),
                stderr: Mutex::new(String::new()),
            }
        }
    }

    #[async_trait]
    impl UpgradeRuntime for RuntimeMock {
        async fn refresh_update_cache(&self) -> Result<UpdateCache, UpgradeError> {
            if self.cache_error {
                return Err(UpgradeError::new(std::io::Error::other("cdn unavailable")));
            }
            let mut cache = empty_update_cache();
            cache.source = UpdateCacheSource::Cdn;
            cache.latest = self.latest.clone();
            Ok(cache)
        }
        async fn detect_install_source(&self) -> Result<InstallSource, UpgradeError> {
            self.source
                .map_err(|message| UpgradeError::new(std::io::Error::other(message)))
        }
        async fn prompt_for_install_choice(
            &self,
            options: InstallPromptOptions,
        ) -> Result<InstallPromptChoiceValue, UpgradeError> {
            self.prompts.lock().expect("prompts").push(options);
            if self.prompt_error {
                Err(UpgradeError::new(std::io::Error::other("prompt failed")))
            } else {
                Ok(self.choice)
            }
        }
        async fn install_update(
            &self,
            source: InstallSource,
            version: &str,
            platform: UpdatePlatform,
        ) -> Result<(), UpgradeError> {
            self.installs
                .lock()
                .expect("installs")
                .push((source, version.to_owned(), platform));
            if self.install_error {
                Err(UpgradeError::new(std::io::Error::other(
                    "npm exited with code 1",
                )))
            } else {
                Ok(())
            }
        }
        fn platform(&self) -> UpdatePlatform {
            UpdatePlatform::Other
        }
        fn is_interactive(&self) -> bool {
            self.interactive
        }
        fn track(&self, event: &str, _: &Map<String, Value>) -> Result<(), UpgradeError> {
            self.events.lock().expect("events").push(event.to_owned());
            Ok(())
        }
        fn log_info(&self, _: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError> {
            self.info_logs
                .lock()
                .expect("info logs")
                .push(properties.clone());
            Ok(())
        }
        fn log_warn(&self, _: &str, properties: &Map<String, Value>) -> Result<(), UpgradeError> {
            self.warn_logs
                .lock()
                .expect("warn logs")
                .push(properties.clone());
            Ok(())
        }
        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }
        fn write_stderr(&self, text: &str) {
            self.stderr.lock().expect("stderr").push_str(text);
        }
    }

    #[tokio::test]
    async fn prompts_installs_tracks_and_reports_success() {
        let runtime = RuntimeMock::new();
        assert_eq!(handle_upgrade(&runtime, "0.4.0").await.expect("upgrade"), 0);
        assert_eq!(
            runtime.prompts.lock().expect("prompts")[0].target.version,
            "0.5.0"
        );
        assert_eq!(runtime.installs.lock().expect("installs").len(), 1);
        let events = runtime.events.lock().expect("events");
        assert!(events.contains(&"upgrade_command_prompted".to_owned()));
        assert!(events.contains(&"upgrade_command_succeeded".to_owned()));
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("Updated @moonshot-ai")
        );
        let info_logs = runtime.info_logs.lock().expect("info logs");
        assert_eq!(info_logs[0]["targetVersion"], "0.5.0");
        assert!(!info_logs[0].contains_key("target_version"));
    }

    #[tokio::test]
    async fn declined_prompt_skips_install() {
        let mut runtime = RuntimeMock::new();
        runtime.choice = InstallPromptChoiceValue::Skip;
        assert_eq!(handle_upgrade(&runtime, "0.4.0").await.expect("upgrade"), 0);
        assert!(runtime.installs.lock().expect("installs").is_empty());
        assert!(
            runtime
                .events
                .lock()
                .expect("events")
                .contains(&"upgrade_command_skipped".to_owned())
        );
    }

    #[tokio::test]
    async fn current_version_reports_up_to_date_before_source_detection() {
        let mut runtime = RuntimeMock::new();
        runtime.latest = Some("0.4.0".to_owned());
        runtime.source = Err("must not matter");
        assert_eq!(handle_upgrade(&runtime, "0.4.0").await.expect("upgrade"), 0);
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("up to date (v0.4.0)")
        );
    }

    #[tokio::test]
    async fn unsupported_or_noninteractive_prints_manual_command() {
        for (source, interactive) in [
            (InstallSource::Unsupported, true),
            (InstallSource::NpmGlobal, false),
        ] {
            let mut runtime = RuntimeMock::new();
            runtime.source = Ok(source);
            runtime.interactive = interactive;
            assert_eq!(handle_upgrade(&runtime, "0.4.0").await.expect("upgrade"), 0);
            assert!(runtime.prompts.lock().expect("prompts").is_empty());
            assert!(
                runtime
                    .stdout
                    .lock()
                    .expect("stdout")
                    .contains("To update manually")
            );
        }
    }

    #[tokio::test]
    async fn refresh_and_install_failures_return_one_with_expected_messages() {
        let mut refresh = RuntimeMock::new();
        refresh.cache_error = true;
        assert_eq!(
            handle_upgrade(&refresh, "0.4.0")
                .await
                .expect("handled refresh failure"),
            1
        );
        assert!(
            refresh
                .stderr
                .lock()
                .expect("stderr")
                .contains("failed to check for updates: cdn unavailable")
        );
        let mut install = RuntimeMock::new();
        install.install_error = true;
        assert_eq!(
            handle_upgrade(&install, "0.4.0")
                .await
                .expect("handled install failure"),
            1
        );
        assert!(
            install
                .stderr
                .lock()
                .expect("stderr")
                .contains("npm exited with code 1")
        );
        let warn_logs = install.warn_logs.lock().expect("warn logs");
        assert_eq!(warn_logs[0]["targetVersion"], "0.5.0");
        assert_eq!(warn_logs[0]["error"], "npm exited with code 1");
    }

    #[tokio::test]
    async fn prompt_failure_propagates_without_becoming_an_exit_code() {
        let mut runtime = RuntimeMock::new();
        runtime.prompt_error = true;

        let error = handle_upgrade(&runtime, "0.4.0")
            .await
            .expect_err("prompt failure must propagate");

        assert_eq!(error.to_string(), "prompt failed");
        assert!(runtime.stderr.lock().expect("stderr").is_empty());
    }
}
