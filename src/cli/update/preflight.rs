use std::{collections::HashMap, error::Error, fmt};

use async_trait::async_trait;
use chrono::DateTime;
use serde_json::{Map, Value, json};

use super::{
    prompt::CHANGELOG_URL,
    types::{
        InstallSource, NPM_PACKAGE_NAME, UpdateDecision, UpdateInstallState, UpdateInstallSuccess,
        UpdateTarget,
    },
};

pub const KIMI_CODE_CDN_BASE: &str = "https://code.kimi.com/kimi-code";
pub const NATIVE_INSTALL_COMMAND_UNIX: &str =
    "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash";
pub const NATIVE_INSTALL_COMMAND_WIN: &str =
    "irm https://code.kimi.com/kimi-code/install.ps1 | iex";
pub const AUTO_INSTALL_FAILURE_PROMPT_THRESHOLD: u64 = 2;
pub const AUTO_INSTALL_ACTIVE_TTL_MS: i64 = 6 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePlatform {
    Windows,
    Other,
}

impl UpdatePlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCommand {
    pub command: String,
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedInstallSource;

impl fmt::Display for UnsupportedInstallSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unsupported install source cannot be auto-installed")
    }
}

impl Error for UnsupportedInstallSource {}

#[derive(Debug)]
pub struct UpdateNoticeError(Box<dyn Error + Send + Sync>);

impl UpdateNoticeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Display for UpdateNoticeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for UpdateNoticeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[async_trait]
pub trait UpdateNoticeRuntime: Send + Sync {
    fn now_iso(&self) -> String;
    fn write_stdout(&self, text: &str);
    fn track(&self, event: &str, properties: &Map<String, Value>) -> Result<(), UpdateNoticeError>;
    fn log_info(
        &self,
        message: &str,
        properties: &Map<String, Value>,
    ) -> Result<(), UpdateNoticeError>;
    async fn write_install_state(
        &self,
        state: &UpdateInstallState,
    ) -> Result<(), UpdateNoticeError>;
}

// Original:
//   apps/kimi-code/src/cli/update/preflight.ts
//   installCommandFor()
pub fn install_command_for(
    source: InstallSource,
    version: &str,
    platform: UpdatePlatform,
) -> String {
    match source {
        InstallSource::NpmGlobal => {
            format!("npm install -g {NPM_PACKAGE_NAME}@{version}")
        }
        InstallSource::PnpmGlobal => {
            format!("pnpm add -g {NPM_PACKAGE_NAME}@{version}")
        }
        InstallSource::YarnGlobal => {
            format!("yarn global add {NPM_PACKAGE_NAME}@{version}")
        }
        InstallSource::BunGlobal => format!("bun add -g {NPM_PACKAGE_NAME}@{version}"),
        InstallSource::Homebrew => "brew upgrade kimi-code".to_owned(),
        InstallSource::Native if platform == UpdatePlatform::Windows => {
            NATIVE_INSTALL_COMMAND_WIN.to_owned()
        }
        InstallSource::Native => NATIVE_INSTALL_COMMAND_UNIX.to_owned(),
        InstallSource::Unsupported => {
            format!("npm install -g {NPM_PACKAGE_NAME}@{version}")
        }
    }
}

// Original: canAutoInstall()
pub fn can_auto_install(source: InstallSource, platform: UpdatePlatform) -> bool {
    match source {
        InstallSource::NpmGlobal
        | InstallSource::PnpmGlobal
        | InstallSource::YarnGlobal
        | InstallSource::BunGlobal => true,
        InstallSource::Homebrew | InstallSource::Unsupported => false,
        InstallSource::Native => platform != UpdatePlatform::Windows,
    }
}

// Original: spawnForSource()
pub fn spawn_for_source(
    source: InstallSource,
    version: &str,
    platform: UpdatePlatform,
) -> Result<SpawnCommand, UnsupportedInstallSource> {
    let package = format!("{NPM_PACKAGE_NAME}@{version}");
    let command = match source {
        InstallSource::NpmGlobal => SpawnCommand {
            command: with_cmd_suffix("npm", platform),
            arguments: vec!["install".to_owned(), "-g".to_owned(), package],
        },
        InstallSource::PnpmGlobal => SpawnCommand {
            command: with_cmd_suffix("pnpm", platform),
            arguments: vec!["add".to_owned(), "-g".to_owned(), package],
        },
        InstallSource::YarnGlobal => SpawnCommand {
            command: with_cmd_suffix("yarn", platform),
            arguments: vec!["global".to_owned(), "add".to_owned(), package],
        },
        InstallSource::BunGlobal => SpawnCommand {
            command: if platform == UpdatePlatform::Windows {
                "bun.exe".to_owned()
            } else {
                "bun".to_owned()
            },
            arguments: vec!["add".to_owned(), "-g".to_owned(), package],
        },
        InstallSource::Homebrew => SpawnCommand {
            command: "brew".to_owned(),
            arguments: vec!["upgrade".to_owned(), "kimi-code".to_owned()],
        },
        InstallSource::Native => SpawnCommand {
            command: "bash".to_owned(),
            arguments: vec![
                "-c".to_owned(),
                format!("set -o pipefail; {NATIVE_INSTALL_COMMAND_UNIX}"),
            ],
        },
        InstallSource::Unsupported => return Err(UnsupportedInstallSource),
    };
    Ok(command)
}

fn with_cmd_suffix(base: &str, platform: UpdatePlatform) -> String {
    if platform == UpdatePlatform::Windows {
        format!("{base}.cmd")
    } else {
        base.to_owned()
    }
}

// Original: renderManualUpdateMessage()
pub fn render_manual_update_message(
    current_version: &str,
    target: &UpdateTarget,
    source: InstallSource,
    install_command: &str,
) -> String {
    let source_description = match source {
        InstallSource::NpmGlobal
        | InstallSource::PnpmGlobal
        | InstallSource::YarnGlobal
        | InstallSource::BunGlobal => source.as_str(),
        InstallSource::Homebrew => "homebrew",
        InstallSource::Native => "native (windows). Auto-update is not supported on this platform.",
        InstallSource::Unsupported => "unsupported package manager or layout.",
    };
    format!(
        "A newer version of {NPM_PACKAGE_NAME} is available ({current_version} -> {}).\n\
Detected install source: {source_description}\n\
To update manually, run: {install_command}\n",
        target.version
    )
}

// Original: renderInstallSuccessMessage()
pub fn render_install_success_message(target: &UpdateTarget) -> String {
    format!(
        "Updated {NPM_PACKAGE_NAME} to {}. Restart the CLI to use the new version.\n",
        target.version
    )
}

pub fn render_background_install_success_notice(version: &str) -> String {
    let display_version = if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    };
    format!("Kimi Code updated to {display_version}\nChangelog: {CHANGELOG_URL}\n")
}

// Original:
//   apps/kimi-code/src/cli/update/preflight.ts
//   showPendingBackgroundInstallNotice()
pub async fn show_pending_background_install_notice(
    runtime: &dyn UpdateNoticeRuntime,
    state: &UpdateInstallState,
    current_version: &str,
) -> UpdateInstallState {
    if let Some(success) = &state.last_success
        && success.notified_at.is_none()
        && success.version == current_version
    {
        runtime.write_stdout(&render_background_install_success_notice(&success.version));
        record_success_notice(runtime, &success.version, false);
        let next_state = UpdateInstallState {
            active: None,
            last_failure: None,
            last_success: Some(UpdateInstallSuccess {
                version: success.version.clone(),
                installed_at: success.installed_at.clone(),
                notified_at: Some(runtime.now_iso()),
            }),
        };
        let _ = runtime.write_install_state(&next_state).await;
        return next_state;
    }

    let Some(active) = &state.active else {
        return state.clone();
    };
    if active.version != current_version
        || state.last_success.as_ref().is_some_and(|success| {
            success.version == current_version && success.notified_at.is_some()
        })
    {
        return state.clone();
    }

    let notified_at = runtime.now_iso();
    runtime.write_stdout(&render_background_install_success_notice(&active.version));
    record_success_notice(runtime, &active.version, true);
    let next_state = UpdateInstallState {
        active: None,
        last_failure: None,
        last_success: Some(UpdateInstallSuccess {
            version: active.version.clone(),
            installed_at: notified_at.clone(),
            notified_at: Some(notified_at),
        }),
    };
    let _ = runtime.write_install_state(&next_state).await;
    next_state
}

fn record_success_notice(runtime: &dyn UpdateNoticeRuntime, version: &str, inferred: bool) {
    let telemetry = object([
        ("version", json!(version)),
        ("inferred_from_active", json!(inferred)),
    ]);
    let log = object([
        ("version", json!(version)),
        ("inferredFromActive", json!(inferred)),
    ]);
    let _ = runtime.track("update_success_notice_shown", &telemetry);
    let _ = runtime.log_info("background update success notice shown", &log);
}

fn object<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

// Original:
//   apps/kimi-code/src/cli/update/preflight.ts
//   failureAttemptsFor()
pub fn failure_attempts_for(state: &UpdateInstallState, target: &UpdateTarget) -> u64 {
    state.last_failure.as_ref().map_or(0, |failure| {
        if failure.version == target.version {
            failure.attempts
        } else {
            0
        }
    })
}

// Original: hasFreshActiveInstall()
pub fn has_fresh_active_install_at(
    state: &UpdateInstallState,
    target: &UpdateTarget,
    now_millis: i64,
) -> bool {
    let Some(active) = &state.active else {
        return false;
    };
    if active.version != target.version {
        return false;
    }
    let Ok(started_at) = DateTime::parse_from_rfc3339(&active.started_at) else {
        return false;
    };
    now_millis - started_at.timestamp_millis() < AUTO_INSTALL_ACTIVE_TTL_MS
}

// Original: isAutoUpdateDisabledByEnv()
pub fn is_auto_update_disabled_by_env(environment: &HashMap<String, String>) -> bool {
    ["KIMI_CODE_NO_AUTO_UPDATE", "KIMI_CLI_NO_AUTO_UPDATE"]
        .into_iter()
        .filter_map(|name| environment.get(name))
        .any(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

// Original: decideUpdateAction()
pub fn decide_update_action(
    target: Option<&UpdateTarget>,
    is_interactive: bool,
    source: InstallSource,
    platform: UpdatePlatform,
) -> UpdateDecision {
    if target.is_none() || !is_interactive {
        UpdateDecision::None
    } else if can_auto_install(source, platform) {
        UpdateDecision::PromptInstall
    } else {
        UpdateDecision::ManualCommand
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::{TimeZone, Utc};

    use super::*;

    const VERSION: &str = "0.5.0";

    struct NoticeRuntimeMock {
        fail_write: bool,
        stdout: Mutex<String>,
        tracks: Mutex<Vec<Map<String, Value>>>,
        writes: Mutex<Vec<UpdateInstallState>>,
    }

    impl NoticeRuntimeMock {
        fn new() -> Self {
            Self {
                fail_write: false,
                stdout: Mutex::new(String::new()),
                tracks: Mutex::new(Vec::new()),
                writes: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl UpdateNoticeRuntime for NoticeRuntimeMock {
        fn now_iso(&self) -> String {
            "2026-07-21T12:00:00.000Z".to_owned()
        }
        fn write_stdout(&self, text: &str) {
            self.stdout.lock().expect("stdout").push_str(text);
        }
        fn track(&self, _: &str, properties: &Map<String, Value>) -> Result<(), UpdateNoticeError> {
            self.tracks.lock().expect("tracks").push(properties.clone());
            Ok(())
        }
        fn log_info(&self, _: &str, _: &Map<String, Value>) -> Result<(), UpdateNoticeError> {
            Ok(())
        }
        async fn write_install_state(
            &self,
            state: &UpdateInstallState,
        ) -> Result<(), UpdateNoticeError> {
            self.writes.lock().expect("writes").push(state.clone());
            if self.fail_write {
                Err(UpdateNoticeError::new(std::io::Error::other(
                    "write failed",
                )))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn builds_manual_commands_for_every_install_source() {
        assert_eq!(
            install_command_for(InstallSource::NpmGlobal, VERSION, UpdatePlatform::Other),
            "npm install -g @moonshot-ai/kimi-code@0.5.0"
        );
        assert_eq!(
            install_command_for(InstallSource::PnpmGlobal, VERSION, UpdatePlatform::Other),
            "pnpm add -g @moonshot-ai/kimi-code@0.5.0"
        );
        assert_eq!(
            install_command_for(InstallSource::YarnGlobal, VERSION, UpdatePlatform::Other),
            "yarn global add @moonshot-ai/kimi-code@0.5.0"
        );
        assert_eq!(
            install_command_for(InstallSource::BunGlobal, VERSION, UpdatePlatform::Other),
            "bun add -g @moonshot-ai/kimi-code@0.5.0"
        );
        assert_eq!(
            install_command_for(InstallSource::Homebrew, VERSION, UpdatePlatform::Other),
            "brew upgrade kimi-code"
        );
        assert_eq!(
            install_command_for(InstallSource::Native, VERSION, UpdatePlatform::Windows),
            NATIVE_INSTALL_COMMAND_WIN
        );
        assert_eq!(
            install_command_for(InstallSource::Native, VERSION, UpdatePlatform::Other),
            NATIVE_INSTALL_COMMAND_UNIX
        );
        assert_eq!(
            install_command_for(InstallSource::Unsupported, VERSION, UpdatePlatform::Other),
            "npm install -g @moonshot-ai/kimi-code@0.5.0"
        );
    }

    #[test]
    fn auto_install_capability_matches_package_manager_and_platform() {
        for source in [
            InstallSource::NpmGlobal,
            InstallSource::PnpmGlobal,
            InstallSource::YarnGlobal,
            InstallSource::BunGlobal,
        ] {
            assert!(can_auto_install(source, UpdatePlatform::Windows));
            assert!(can_auto_install(source, UpdatePlatform::Other));
        }
        assert!(!can_auto_install(
            InstallSource::Homebrew,
            UpdatePlatform::Other
        ));
        assert!(!can_auto_install(
            InstallSource::Unsupported,
            UpdatePlatform::Other
        ));
        assert!(!can_auto_install(
            InstallSource::Native,
            UpdatePlatform::Windows
        ));
        assert!(can_auto_install(
            InstallSource::Native,
            UpdatePlatform::Other
        ));
    }

    #[test]
    fn builds_spawn_commands_and_windows_executable_suffixes() {
        assert_eq!(
            spawn_for_source(InstallSource::NpmGlobal, VERSION, UpdatePlatform::Windows)
                .expect("npm"),
            SpawnCommand {
                command: "npm.cmd".to_owned(),
                arguments: vec![
                    "install".to_owned(),
                    "-g".to_owned(),
                    "@moonshot-ai/kimi-code@0.5.0".to_owned()
                ],
            }
        );
        assert_eq!(
            spawn_for_source(InstallSource::BunGlobal, VERSION, UpdatePlatform::Windows)
                .expect("bun"),
            SpawnCommand {
                command: "bun.exe".to_owned(),
                arguments: vec![
                    "add".to_owned(),
                    "-g".to_owned(),
                    "@moonshot-ai/kimi-code@0.5.0".to_owned()
                ],
            }
        );
        assert_eq!(
            spawn_for_source(InstallSource::Homebrew, VERSION, UpdatePlatform::Other)
                .expect("brew"),
            SpawnCommand {
                command: "brew".to_owned(),
                arguments: vec!["upgrade".to_owned(), "kimi-code".to_owned()],
            }
        );
        assert_eq!(
            spawn_for_source(InstallSource::Native, VERSION, UpdatePlatform::Other)
                .expect("native")
                .arguments,
            [
                "-c".to_owned(),
                format!("set -o pipefail; {NATIVE_INSTALL_COMMAND_UNIX}")
            ]
        );
        assert!(
            spawn_for_source(InstallSource::Unsupported, VERSION, UpdatePlatform::Other).is_err()
        );
    }

    #[test]
    fn renders_manual_success_and_background_messages() {
        let target = UpdateTarget {
            version: VERSION.to_owned(),
        };
        assert_eq!(
            render_manual_update_message(
                "0.4.0",
                &target,
                InstallSource::Native,
                NATIVE_INSTALL_COMMAND_WIN
            ),
            format!(
                "A newer version of @moonshot-ai/kimi-code is available (0.4.0 -> 0.5.0).\nDetected install source: native (windows). Auto-update is not supported on this platform.\nTo update manually, run: {NATIVE_INSTALL_COMMAND_WIN}\n"
            )
        );
        assert_eq!(
            render_install_success_message(&target),
            "Updated @moonshot-ai/kimi-code to 0.5.0. Restart the CLI to use the new version.\n"
        );
        assert_eq!(
            render_background_install_success_notice("0.5.0"),
            format!("Kimi Code updated to v0.5.0\nChangelog: {CHANGELOG_URL}\n")
        );
        assert!(render_background_install_success_notice("v0.5.0").contains("to v0.5.0"));
    }

    #[test]
    fn failure_attempts_apply_only_to_the_same_target_version() {
        let mut state = super::super::types::empty_update_install_state();
        state.last_failure = Some(super::super::types::UpdateInstallFailure {
            version: "0.5.0".to_owned(),
            failed_at: "2026-07-21T00:00:00.000Z".to_owned(),
            attempts: 2,
        });
        assert_eq!(
            failure_attempts_for(
                &state,
                &UpdateTarget {
                    version: "0.5.0".to_owned()
                }
            ),
            2
        );
        assert_eq!(
            failure_attempts_for(
                &state,
                &UpdateTarget {
                    version: "0.6.0".to_owned()
                }
            ),
            0
        );
    }

    #[test]
    fn active_install_freshness_checks_version_timestamp_and_strict_ttl() {
        let started = Utc
            .with_ymd_and_hms(2026, 7, 21, 0, 0, 0)
            .single()
            .expect("date");
        let target = UpdateTarget {
            version: "0.5.0".to_owned(),
        };
        let mut state = super::super::types::empty_update_install_state();
        state.active = Some(super::super::types::UpdateInstallActive {
            version: target.version.clone(),
            source: InstallSource::NpmGlobal,
            started_at: started.to_rfc3339(),
        });
        assert!(has_fresh_active_install_at(
            &state,
            &target,
            started.timestamp_millis() + AUTO_INSTALL_ACTIVE_TTL_MS - 1
        ));
        assert!(!has_fresh_active_install_at(
            &state,
            &target,
            started.timestamp_millis() + AUTO_INSTALL_ACTIVE_TTL_MS
        ));
        state.active.as_mut().expect("active").started_at = "not a date".to_owned();
        assert!(!has_fresh_active_install_at(
            &state,
            &target,
            started.timestamp_millis()
        ));
        state.active.as_mut().expect("active").started_at = started.to_rfc3339();
        state.active.as_mut().expect("active").version = "0.6.0".to_owned();
        assert!(!has_fresh_active_install_at(
            &state,
            &target,
            started.timestamp_millis()
        ));
    }

    #[test]
    fn recognizes_both_disable_environment_variables_and_truthy_values() {
        for name in ["KIMI_CODE_NO_AUTO_UPDATE", "KIMI_CLI_NO_AUTO_UPDATE"] {
            for value in ["1", " true ", "YES", "On"] {
                assert!(is_auto_update_disabled_by_env(&HashMap::from([(
                    name.to_owned(),
                    value.to_owned()
                )])));
            }
        }
        for value in ["", "0", "false", "disabled"] {
            assert!(!is_auto_update_disabled_by_env(&HashMap::from([(
                "KIMI_CODE_NO_AUTO_UPDATE".to_owned(),
                value.to_owned()
            )])));
        }
    }

    #[test]
    fn decides_none_prompt_or_manual_from_visibility_and_capability() {
        let target = UpdateTarget {
            version: "0.5.0".to_owned(),
        };
        assert_eq!(
            decide_update_action(None, true, InstallSource::NpmGlobal, UpdatePlatform::Other),
            UpdateDecision::None
        );
        assert_eq!(
            decide_update_action(
                Some(&target),
                false,
                InstallSource::NpmGlobal,
                UpdatePlatform::Other
            ),
            UpdateDecision::None
        );
        assert_eq!(
            decide_update_action(
                Some(&target),
                true,
                InstallSource::NpmGlobal,
                UpdatePlatform::Other
            ),
            UpdateDecision::PromptInstall
        );
        assert_eq!(
            decide_update_action(
                Some(&target),
                true,
                InstallSource::Homebrew,
                UpdatePlatform::Other
            ),
            UpdateDecision::ManualCommand
        );
        assert_eq!(
            decide_update_action(
                Some(&target),
                true,
                InstallSource::Native,
                UpdatePlatform::Windows
            ),
            UpdateDecision::ManualCommand
        );
    }

    #[tokio::test]
    async fn unnotified_success_is_shown_once_and_marked_notified() {
        let runtime = NoticeRuntimeMock::new();
        let state = UpdateInstallState {
            active: None,
            last_failure: None,
            last_success: Some(UpdateInstallSuccess {
                version: VERSION.to_owned(),
                installed_at: "2026-07-21T10:00:00.000Z".to_owned(),
                notified_at: None,
            }),
        };
        let next = show_pending_background_install_notice(&runtime, &state, VERSION).await;
        assert_eq!(
            next.last_success,
            Some(UpdateInstallSuccess {
                version: VERSION.to_owned(),
                installed_at: "2026-07-21T10:00:00.000Z".to_owned(),
                notified_at: Some("2026-07-21T12:00:00.000Z".to_owned()),
            })
        );
        assert!(
            runtime
                .stdout
                .lock()
                .expect("stdout")
                .contains("updated to v0.5.0")
        );
        assert_eq!(
            runtime.tracks.lock().expect("tracks")[0]["inferred_from_active"],
            false
        );
    }

    #[tokio::test]
    async fn running_active_version_is_inferred_as_success() {
        let runtime = NoticeRuntimeMock::new();
        let state = UpdateInstallState {
            active: Some(super::super::types::UpdateInstallActive {
                version: VERSION.to_owned(),
                source: InstallSource::Native,
                started_at: "2026-07-21T10:00:00.000Z".to_owned(),
            }),
            last_failure: None,
            last_success: None,
        };
        let next = show_pending_background_install_notice(&runtime, &state, VERSION).await;
        assert_eq!(next.active, None);
        assert_eq!(next.last_failure, None);
        assert_eq!(
            runtime.tracks.lock().expect("tracks")[0]["inferred_from_active"],
            true
        );
        let success = next.last_success.expect("success");
        assert_eq!(success.installed_at, success.notified_at.expect("notified"));
    }

    #[tokio::test]
    async fn already_notified_or_other_version_leaves_state_unchanged() {
        let runtime = NoticeRuntimeMock::new();
        let state = UpdateInstallState {
            active: None,
            last_failure: None,
            last_success: Some(UpdateInstallSuccess {
                version: VERSION.to_owned(),
                installed_at: "2026-07-21T10:00:00.000Z".to_owned(),
                notified_at: Some("2026-07-21T11:00:00.000Z".to_owned()),
            }),
        };
        assert_eq!(
            show_pending_background_install_notice(&runtime, &state, VERSION).await,
            state
        );
        assert!(runtime.stdout.lock().expect("stdout").is_empty());
        assert!(runtime.writes.lock().expect("writes").is_empty());
    }

    #[tokio::test]
    async fn notice_state_write_is_best_effort() {
        let mut runtime = NoticeRuntimeMock::new();
        runtime.fail_write = true;
        let state = UpdateInstallState {
            active: None,
            last_failure: None,
            last_success: Some(UpdateInstallSuccess {
                version: VERSION.to_owned(),
                installed_at: "2026-07-21T10:00:00.000Z".to_owned(),
                notified_at: None,
            }),
        };
        let next = show_pending_background_install_notice(&runtime, &state, VERSION).await;
        assert!(next.last_success.expect("success").notified_at.is_some());
        assert!(runtime.stdout.lock().expect("stdout").contains("updated"));
    }
}
