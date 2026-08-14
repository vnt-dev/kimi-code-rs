//! `kimi-webbridge` capability entry (macOS / Linux / Windows).
//!
//! Layers: daemon binary (`~/.kimi-webbridge/bin/`, local HTTP daemon on
//! 127.0.0.1:10086) + agent wiring (the official `kimi-webbridge` plugin —
//! skills only, installed through the plugin service) + browser extension
//! (soft gate, user installs from the webstore or the manual zip).
//!
//! A running daemon is left untouched (start-if-down only, Kimi Work
//! coexistence). Reinstall replaces the on-disk binary from the latest
//! channel, which takes effect the next time the daemon starts. Installs
//! are detect-first and idempotent: only unsatisfied layers are redone,
//! setup re-enables a previously disabled wiring plugin, the binary step
//! requires the executable bit on POSIX (an interrupted install reads as
//! missing and re-downloads). Legacy standalone skill copies are moved into
//! a Kimi Code backup after the managed plugin has been refreshed, so plugin
//! updates become authoritative without deleting user files.
//!
//! Original: `packages/agent-core-v2/src/app/capability/entries/kimiWebbridge.ts`.

use parking_lot::Mutex;
use std::sync::Arc;
use std::{
    collections::HashMap,
    error::Error,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    _base::errors::errors::ExpectedError,
    app::{
        capability::{
            host::{download_to_file, rm_force, run_command},
            types::{
                CapabilityDetectResult, CapabilityEntry, CapabilityEntryResult, CapabilityId,
                CapabilityInstallReporter, CapabilityStep, CapabilityStepState,
            },
        },
        plugin::{InstallPluginInput, SetPluginEnabledInput},
    },
};

use super::{
    PluginLayerConfig, context::CapabilityEntryContext, detect_plugin_layer, is_executable,
    mkdtemp_in, now_millis, path_exists,
};

const PLUGIN: PluginLayerConfig = PluginLayerConfig {
    id: "kimi-webbridge",
    zip_url: "https://code.kimi.com/kimi-code/plugins/official/kimi-webbridge.zip",
};
const BINARY_CDN_BASE: &str = "https://cdn.kimi.com/webbridge/latest/releases";
const DEFAULT_DAEMON_BASE_URL: &str = "http://127.0.0.1:10086";
const STATUS_TIMEOUT: Duration = Duration::from_millis(1_500);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const START_POLL_INTERVAL: Duration = Duration::from_millis(500);
const START_POLL_ATTEMPTS: usize = 20;

#[derive(Clone, Debug, Default, Deserialize)]
struct DaemonStatus {
    running: Option<bool>,
    version: Option<String>,
    extension_connected: Option<bool>,
}

#[derive(Clone, Debug)]
struct StandaloneSkillDir {
    label: &'static str,
    path: PathBuf,
}

pub fn binary_asset_name(platform: &str, arch: &str) -> Option<&'static str> {
    match (platform, arch) {
        ("darwin", "arm64") => Some("kimi-webbridge-darwin-arm64"),
        ("darwin", "x64") => Some("kimi-webbridge-darwin-amd64"),
        ("linux", "arm64") => Some("kimi-webbridge-linux-arm64"),
        ("linux", "x64") => Some("kimi-webbridge-linux-amd64"),
        ("win32", "x64") => Some("kimi-webbridge-windows-amd64.exe"),
        _ => None,
    }
}

pub struct KimiWebbridgeEntry {
    ctx: CapabilityEntryContext,
    base_url: String,
    bin_dir: PathBuf,
    bin_path: PathBuf,
    user_source_skill_dirs: Vec<StandaloneSkillDir>,
    standalone_skill_backup_dir: PathBuf,
    supported: bool,
    standalone_skill_backup_path: Mutex<Option<String>>,
    standalone_skill_migration_error: Mutex<Option<String>>,
}

impl KimiWebbridgeEntry {
    fn new(ctx: CapabilityEntryContext) -> Self {
        let base_url = ctx
            .webbridge_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_DAEMON_BASE_URL.to_owned());
        let bin_dir = ctx.user_home_dir.join(".kimi-webbridge").join("bin");
        let bin_name = if ctx.platform == "win32" {
            "kimi-webbridge.exe"
        } else {
            "kimi-webbridge"
        };
        let user_source_skill_dirs = vec![
            StandaloneSkillDir {
                label: "kimi-code",
                path: ctx.kimi_home_dir.join("skills").join("kimi-webbridge"),
            },
            StandaloneSkillDir {
                label: "agents",
                path: ctx
                    .user_home_dir
                    .join(".agents")
                    .join("skills")
                    .join("kimi-webbridge"),
            },
        ];
        Self {
            standalone_skill_backup_dir: ctx
                .kimi_home_dir
                .join("backups")
                .join("kimi-webbridge-skills"),
            supported: binary_asset_name(&ctx.platform, &ctx.arch).is_some(),
            bin_path: bin_dir.join(bin_name),
            bin_dir,
            user_source_skill_dirs,
            base_url,
            standalone_skill_backup_path: Mutex::new(None),
            standalone_skill_migration_error: Mutex::new(None),
            ctx,
        }
    }

    fn bin_path_string(&self) -> String {
        self.bin_path.to_string_lossy().into_owned()
    }

    // Original: fetchDaemonStatus() — any failure reads as "daemon down".
    async fn fetch_daemon_status(&self) -> Option<DaemonStatus> {
        let fetch = self.ctx.fetch_impl_or_default();
        let url = format!("{}/status", self.base_url);
        let fetch = async move {
            let response = fetch.fetch(&url).await.ok()?;
            if !response.ok {
                return None;
            }
            let mut body = response.body?;
            let mut bytes = Vec::new();
            while let Some(chunk) = futures_util::StreamExt::next(&mut body).await {
                bytes.extend_from_slice(&chunk.ok()?);
            }
            serde_json::from_slice::<DaemonStatus>(&bytes).ok()
        };
        tokio::time::timeout(STATUS_TIMEOUT, fetch)
            .await
            .ok()
            .flatten()
    }

    async fn standalone_skill_dirs(&self) -> Vec<StandaloneSkillDir> {
        let mut present = Vec::new();
        for dir in &self.user_source_skill_dirs {
            if path_exists(&dir.path).await {
                present.push(dir.clone());
            }
        }
        present
    }

    // Original: migrateStandaloneSkills() — move legacy copies into a fresh
    // backup root instead of deleting them.
    async fn migrate_standalone_skills(&self) -> io::Result<Option<PathBuf>> {
        let skills = self.standalone_skill_dirs().await;
        if skills.is_empty() {
            return Ok(None);
        }
        tokio::fs::create_dir_all(&self.standalone_skill_backup_dir).await?;
        let backup_root = mkdtemp_in(&self.standalone_skill_backup_dir, "migration-").await?;
        for skill in skills {
            tokio::fs::rename(&skill.path, backup_root.join(skill.label)).await?;
        }
        Ok(Some(backup_root))
    }

    async fn detect(&self) -> CapabilityEntryResult<CapabilityDetectResult> {
        let mut steps = Vec::new();

        let binary_present = path_exists(&self.bin_path).await;
        let binary_usable =
            binary_present && (self.ctx.platform == "win32" || is_executable(&self.bin_path).await);
        steps.push(CapabilityStep {
            id: "daemon-binary".to_owned(),
            state: if binary_usable {
                CapabilityStepState::Ok
            } else {
                CapabilityStepState::Missing
            },
            detail: if binary_present && !binary_usable {
                Some("not executable".to_owned())
            } else {
                None
            },
            optional: None,
        });

        let daemon = self.fetch_daemon_status().await;
        let daemon_running = daemon.as_ref().and_then(|daemon| daemon.running) == Some(true);
        steps.push(CapabilityStep {
            id: "daemon".to_owned(),
            state: if daemon_running {
                CapabilityStepState::Ok
            } else {
                CapabilityStepState::Missing
            },
            detail: if daemon_running {
                daemon.as_ref().and_then(|daemon| daemon.version.clone())
            } else {
                None
            },
            optional: None,
        });

        let plugin = detect_plugin_layer(&self.ctx, &PLUGIN, "skill").await?;
        steps.push(plugin.step);

        let standalone_skills = self.standalone_skill_dirs().await;
        if !standalone_skills.is_empty() {
            let migration_error = self.standalone_skill_migration_error.lock().clone();
            steps.push(CapabilityStep {
                id: "standalone-skill-migration".to_owned(),
                state: CapabilityStepState::Missing,
                detail: Some(migration_error.unwrap_or_else(|| {
                    standalone_skills
                        .iter()
                        .map(|dir| dir.path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                })),
                optional: Some(true),
            });
        } else if path_exists(&self.standalone_skill_backup_dir).await {
            let backup_path = self.standalone_skill_backup_path.lock().clone();
            steps.push(CapabilityStep {
                id: "standalone-skill-migration".to_owned(),
                state: CapabilityStepState::Ok,
                detail: Some(
                    backup_path
                        .unwrap_or_else(|| self.standalone_skill_backup_dir.display().to_string()),
                ),
                optional: Some(true),
            });
        }

        steps.push(CapabilityStep {
            id: "extension".to_owned(),
            state: if daemon
                .as_ref()
                .and_then(|daemon| daemon.extension_connected)
                == Some(true)
            {
                CapabilityStepState::Ok
            } else {
                CapabilityStepState::Missing
            },
            detail: None,
            optional: Some(true),
        });

        Ok(CapabilityDetectResult {
            steps,
            version: daemon.and_then(|daemon| daemon.version),
        })
    }

    async fn wait_for_daemon(&self) -> CapabilityEntryResult<()> {
        for _ in 0..START_POLL_ATTEMPTS {
            if self
                .fetch_daemon_status()
                .await
                .and_then(|status| status.running)
                == Some(true)
            {
                return Ok(());
            }
            tokio::time::sleep(START_POLL_INTERVAL).await;
        }
        Err(Box::new(ExpectedError::new(format!(
            "WebBridge daemon did not come up on {} — check ~/.kimi-webbridge/logs",
            self.base_url
        ))))
    }

    async fn install_binary(
        &self,
        report: &CapabilityInstallReporter,
        asset: &str,
    ) -> CapabilityEntryResult<()> {
        report("download", Some(0));
        let url = format!("{BINARY_CDN_BASE}/{asset}");
        let staging = std::env::temp_dir().join(format!(
            "kimi-webbridge-{}-{}{}",
            now_millis(),
            &uuid::Uuid::new_v4().simple().to_string()[..6],
            if self.ctx.platform == "win32" {
                ".exe"
            } else {
                ""
            }
        ));
        let result = async {
            let fetch = self.ctx.fetch_impl_or_default();
            download_to_file(
                &url,
                &staging,
                Some(&|percent| report("download", Some(percent))),
                &fetch,
                None,
            )
            .await?;
            tokio::fs::create_dir_all(&self.bin_dir).await?;
            let rename_result: Result<(), Box<dyn Error + Send + Sync>> =
                match tokio::fs::rename(&staging, &self.bin_path).await {
                    Ok(()) => Ok(()),
                    Err(error) if is_cross_device(&error) => {
                        rename_across_devices_fallback(&staging, &self.bin_path)
                            .await
                            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)
                    }
                    Err(error) => Err(Box::new(error) as Box<dyn Error + Send + Sync>),
                };
            rename_result?;
            if self.ctx.platform != "win32" {
                chmod_755(&self.bin_path).await?;
            }
            Ok(())
        }
        .await;
        let _ = tokio::fs::remove_file(&staging).await;
        result
    }

    async fn install(&self, report: CapabilityInstallReporter) -> CapabilityEntryResult<()> {
        let Some(asset) = binary_asset_name(&self.ctx.platform, &self.ctx.arch) else {
            return Err(Box::new(ExpectedError::new(format!(
                "kimi-webbridge is not supported on {}/{}",
                self.ctx.platform, self.ctx.arch
            ))));
        };

        let before = self.detect().await?;
        let step_states: HashMap<&str, CapabilityStepState> = before
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step.state))
            .collect();
        let ready_before = before
            .steps
            .iter()
            .filter(|step| step.optional != Some(true))
            .all(|step| step.state == CapabilityStepState::Ok);
        let standalone_skill_migration_pending =
            step_states.get("standalone-skill-migration") == Some(&CapabilityStepState::Missing);
        if step_states.get("daemon-binary") != Some(&CapabilityStepState::Ok) || ready_before {
            self.install_binary(&report, asset).await?;
        }

        let status = self.fetch_daemon_status().await;
        if status.and_then(|status| status.running) != Some(true) {
            report("daemon", None);
            let started = run_command(
                &self.ctx.host_process,
                &self.bin_path_string(),
                &["start".to_owned()],
                Some(START_TIMEOUT),
            )
            .await?;
            if started.code != 0 {
                return Err(Box::new(ExpectedError::new(format!(
                    "kimi-webbridge start failed: {}",
                    if started.stderr.is_empty() {
                        &started.stdout
                    } else {
                        &started.stderr
                    }
                ))));
            }
            self.wait_for_daemon().await?;
        }

        report("skill", None);
        let summary = self
            .ctx
            .plugins
            .install_plugin(InstallPluginInput {
                source: PLUGIN.zip_url.to_owned(),
            })
            .await?;
        if !summary.enabled {
            self.ctx
                .plugins
                .set_plugin_enabled(SetPluginEnabledInput {
                    id: PLUGIN.id.to_owned(),
                    enabled: true,
                })
                .await?;
        }

        if standalone_skill_migration_pending {
            report("standalone-skill-migration", None);
            match self.migrate_standalone_skills().await {
                Ok(backup_root) => {
                    *self.standalone_skill_backup_path.lock() =
                        backup_root.map(|path| path.display().to_string());
                    *self.standalone_skill_migration_error.lock() = None;
                }
                Err(error) => {
                    *self.standalone_skill_migration_error.lock() = Some(format!(
                        "Could not back up the standalone kimi-webbridge skill: {error}"
                    ));
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl CapabilityEntry for KimiWebbridgeEntry {
    fn id(&self) -> CapabilityId {
        CapabilityId::KimiWebbridge
    }

    fn plugin_id(&self) -> Option<&str> {
        Some(PLUGIN.id)
    }

    fn display_name(&self) -> &str {
        "Kimi WebBridge"
    }

    fn description(&self) -> &str {
        "Control your real browser (with your login sessions) — navigate, click, type, read pages, and screenshot any website."
    }

    fn supported(&self) -> bool {
        self.supported
    }

    async fn detect(&self) -> CapabilityEntryResult<CapabilityDetectResult> {
        self.detect().await
    }

    async fn install(&self, report: CapabilityInstallReporter) -> CapabilityEntryResult<()> {
        self.install(report).await
    }
}

/// Original: `createKimiWebbridgeEntry()`.
pub fn create_kimi_webbridge_entry(ctx: CapabilityEntryContext) -> Arc<dyn CapabilityEntry> {
    Arc::new(KimiWebbridgeEntry::new(ctx))
}

// Original: chmod(binPath, 0o755) on POSIX hosts.
#[cfg(unix)]
async fn chmod_755(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).await
}

#[cfg(not(unix))]
async fn chmod_755(_: &Path) -> io::Result<()> {
    Ok(())
}

// Original: node reports cross-device renames as EXDEV on every platform.
#[cfg(unix)]
fn is_cross_device(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::EXDEV)
}

#[cfg(windows)]
fn is_cross_device(error: &io::Error) -> bool {
    const ERROR_NOT_SAME_DEVICE: i32 = 17;
    error.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE)
}

#[cfg(not(any(unix, windows)))]
fn is_cross_device(_: &io::Error) -> bool {
    false
}

/// Original: renameAcrossDevicesFallback() — copy into a sibling temp file,
/// rename within the destination filesystem, then drop the staging copy.
pub async fn rename_across_devices_fallback(from: &Path, to: &Path) -> io::Result<()> {
    let sibling = PathBuf::from(format!(
        "{}.{}.{}.tmp",
        to.display(),
        std::process::id(),
        now_millis()
    ));
    let result = async {
        tokio::fs::copy(from, &sibling).await?;
        tokio::fs::rename(&sibling, to).await
    }
    .await;
    let _ = tokio::fs::remove_file(&sibling).await;
    result?;
    rm_force(from).await
}

#[cfg(test)]
mod tests {
    //! `kimi-webbridge` capability entry — platform asset mapping, layered
    //! detect, and the idempotent install flow (download → start-if-down →
    //! plugin wiring). All host effects are faked (temp dirs, scripted fetch,
    //! scripted host processes, fake plugins).
    //!
    //! Original: `packages/agent-core-v2/test/app/capability/kimiWebbridge.test.ts`.

    use parking_lot::Mutex;
    use std::path::PathBuf;

    use serde_json::json;

    use crate::{
        app::{capability::entries::test_fakes::*, plugin::PluginState},
        os::interface::host_process::HostProcessServiceHandle,
    };

    use super::*;

    async fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("capability-{tag}-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        root
    }

    #[cfg(unix)]
    async fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .await
            .unwrap();
    }

    #[cfg(not(unix))]
    async fn make_executable(_: &Path) {}

    #[cfg(unix)]
    async fn make_non_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
            .await
            .unwrap();
    }

    fn make_ctx(
        root: &Path,
        plugins: crate::app::plugin::PluginServiceHandle,
        host_process: HostProcessServiceHandle,
    ) -> CapabilityEntryContext {
        CapabilityEntryContext {
            platform: "darwin".to_owned(),
            arch: "arm64".to_owned(),
            kimi_home_dir: root.join("kimi-home"),
            user_home_dir: root.join("user-home"),
            plugins,
            host_process,
            fetch_impl: None,
            applications_dir: None,
            webbridge_base_url: None,
            detect_probe_timeout: None,
            command_timeout: None,
        }
    }

    fn webbridge_ctx(
        root: &Path,
        plugins: &Arc<FakePluginService>,
        fetch: WebbridgeFetch,
    ) -> CapabilityEntryContext {
        let (host, _) = scripted_host(vec![]);
        CapabilityEntryContext {
            fetch_impl: Some(Arc::new(fetch)),
            ..make_ctx(root, plugins.handle(), host)
        }
    }

    fn noop_reporter() -> CapabilityInstallReporter {
        Box::new(|_, _| {})
    }

    type RecordedReports = Arc<Mutex<Vec<(String, Option<u32>)>>>;

    fn recording_reports() -> (CapabilityInstallReporter, RecordedReports) {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&reports);
        (
            Box::new(move |step, percent| {
                recording.lock().push((step.to_owned(), percent));
            }),
            reports,
        )
    }

    async fn fake_binary(root: &Path) -> PathBuf {
        let bin_dir = root.join("user-home").join(".kimi-webbridge").join("bin");
        tokio::fs::create_dir_all(&bin_dir).await.unwrap();
        let bin_path = bin_dir.join("kimi-webbridge");
        tokio::fs::write(&bin_path, "bin").await.unwrap();
        make_executable(&bin_path).await;
        bin_path
    }

    #[test]
    fn maps_platforms_to_cdn_asset_names() {
        assert_eq!(
            binary_asset_name("darwin", "arm64"),
            Some("kimi-webbridge-darwin-arm64")
        );
        assert_eq!(
            binary_asset_name("darwin", "x64"),
            Some("kimi-webbridge-darwin-amd64")
        );
        assert_eq!(
            binary_asset_name("linux", "arm64"),
            Some("kimi-webbridge-linux-arm64")
        );
        assert_eq!(
            binary_asset_name("linux", "x64"),
            Some("kimi-webbridge-linux-amd64")
        );
        assert_eq!(
            binary_asset_name("win32", "x64"),
            Some("kimi-webbridge-windows-amd64.exe")
        );
        assert_eq!(binary_asset_name("win32", "arm64"), None);
        assert_eq!(binary_asset_name("freebsd", "x64"), None);
    }

    #[tokio::test]
    async fn exdev_fallback_replaces_the_destination_without_opening_it_for_write() {
        let root = temp_root("kimi-webbridge-entry").await;
        let from = root.join("staging").join("kimi-webbridge");
        let to = root.join("bin").join("kimi-webbridge");
        tokio::fs::create_dir_all(from.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::create_dir_all(to.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&from, "new").await.unwrap();
        tokio::fs::write(&to, "old-running").await.unwrap();

        // Stage-then-rename on the target filesystem: the live destination is
        // replaced atomically (never opened for write — ETXTBSY-safe), the
        // source is removed, and no sibling temp is left behind.
        rename_across_devices_fallback(&from, &to).await.unwrap();

        assert_eq!(tokio::fs::read_to_string(&to).await.unwrap(), "new");
        assert!(!path_exists(&from).await);
        let mut leftovers = tokio::fs::read_dir(to.parent().unwrap()).await.unwrap();
        while let Some(entry) = leftovers.next_entry().await.unwrap() {
            assert!(!entry.file_name().to_string_lossy().ends_with(".tmp"));
        }
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn is_unsupported_on_unknown_platforms() {
        let root = temp_root("kimi-webbridge-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![]);
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            platform: "freebsd".to_owned(),
            ..make_ctx(&root, plugins.handle(), host)
        });
        assert!(!entry.supported());
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn detects_a_fully_installed_daemon_with_extension_as_soft_gate() {
        let root = temp_root("kimi-webbridge-entry").await;
        fake_binary(&root).await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-webbridge", true, PluginState::Ok).version("1.11.3"),
        ]);
        let fetch = WebbridgeFetch::new(vec![Some(
            json!({ "running": true, "version": "v1.11.3", "extension_connected": false }),
        )]);
        let entry = create_kimi_webbridge_entry(webbridge_ctx(&root, &plugins, fetch));

        let detected = entry.detect().await.unwrap();
        assert_eq!(detected.version.as_deref(), Some("v1.11.3"));
        assert_eq!(
            detected.steps,
            vec![
                CapabilityStep::new("daemon-binary", CapabilityStepState::Ok),
                CapabilityStep {
                    id: "daemon".to_owned(),
                    state: CapabilityStepState::Ok,
                    detail: Some("v1.11.3".to_owned()),
                    optional: None,
                },
                CapabilityStep {
                    id: "skill".to_owned(),
                    state: CapabilityStepState::Ok,
                    detail: Some("1.11.3".to_owned()),
                    optional: None,
                },
                CapabilityStep {
                    id: "extension".to_owned(),
                    state: CapabilityStepState::Missing,
                    detail: None,
                    optional: Some(true),
                },
            ]
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn backs_up_standalone_skills_after_refreshing_the_managed_plugin() {
        let root = temp_root("kimi-webbridge-entry").await;
        let kimi_home = root.join("kimi-home");
        let user_home = root.join("user-home");
        let kimi_skill = kimi_home.join("skills").join("kimi-webbridge");
        let agents_skill = user_home
            .join(".agents")
            .join("skills")
            .join("kimi-webbridge");
        tokio::fs::create_dir_all(&kimi_skill).await.unwrap();
        tokio::fs::write(kimi_skill.join("SKILL.md"), "old")
            .await
            .unwrap();
        tokio::fs::create_dir_all(&agents_skill).await.unwrap();
        tokio::fs::write(agents_skill.join("SKILL.md"), "old")
            .await
            .unwrap();
        let plugins = FakePluginService::new(vec![FakePlugin::new(
            "kimi-webbridge",
            true,
            PluginState::Ok,
        )]);
        let fetch = WebbridgeFetch::new(vec![Some(
            json!({ "running": true, "version": "v1.11.3", "extension_connected": true }),
        )]);
        let entry = create_kimi_webbridge_entry(webbridge_ctx(&root, &plugins, fetch));

        let detected = entry.detect().await.unwrap();
        assert_eq!(
            detected
                .steps
                .iter()
                .find(|step| step.id == "standalone-skill-migration"),
            Some(&CapabilityStep {
                id: "standalone-skill-migration".to_owned(),
                state: CapabilityStepState::Missing,
                detail: Some(format!(
                    "{}, {}",
                    kimi_skill.display(),
                    agents_skill.display()
                )),
                optional: Some(true),
            })
        );
        let (report, reports) = recording_reports();
        entry.install(report).await.unwrap();

        assert_eq!(
            plugins.installs.lock().as_slice(),
            ["https://code.kimi.com/kimi-code/plugins/official/kimi-webbridge.zip"]
        );
        let reports = reports.lock().clone();
        assert!(
            reports
                .iter()
                .any(|(step, _)| step == "standalone-skill-migration")
        );
        assert!(!path_exists(&kimi_skill).await);
        assert!(!path_exists(&agents_skill).await);

        let backup_dir = kimi_home.join("backups").join("kimi-webbridge-skills");
        let mut backups = tokio::fs::read_dir(&backup_dir).await.unwrap();
        let backup = backups.next_entry().await.unwrap().unwrap();
        assert!(backups.next_entry().await.unwrap().is_none());
        assert_eq!(
            tokio::fs::read_to_string(backup.path().join("kimi-code").join("SKILL.md"))
                .await
                .unwrap(),
            "old"
        );
        assert_eq!(
            tokio::fs::read_to_string(backup.path().join("agents").join("SKILL.md"))
                .await
                .unwrap(),
            "old"
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn installs_end_to_end_download_start_if_down_and_plugin_wiring() {
        let root = temp_root("kimi-webbridge-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, calls) = scripted_host(vec![]);
        // First status poll (before start): down. Subsequent polls: up.
        let fetch = WebbridgeFetch::new(vec![
            Some(json!({ "running": false })),
            Some(json!({ "running": false })),
            Some(json!({ "running": true, "version": "v1.11.3", "extension_connected": true })),
        ]);
        let (report, reports) = recording_reports();
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            fetch_impl: Some(Arc::new(fetch)),
            ..make_ctx(&root, plugins.handle(), host)
        });

        entry.install(report).await.unwrap();

        // Binary downloaded into place and made executable.
        let bin_path = root
            .join("user-home")
            .join(".kimi-webbridge")
            .join("bin")
            .join("kimi-webbridge");
        assert!(path_exists(&bin_path).await);
        // Daemon started exactly once (start-if-down).
        assert_eq!(
            calls.lock().as_slice(),
            [format!("{} start", bin_path.display())]
        );
        // Plugin wiring installed from the official CDN zip.
        assert_eq!(
            plugins.installs.lock().as_slice(),
            ["https://code.kimi.com/kimi-code/plugins/official/kimi-webbridge.zip"]
        );
        // Progress reported download steps.
        let reports = reports.lock().clone();
        assert_eq!(reports[0], ("download".to_owned(), Some(0)));
        assert!(reports.iter().any(|(step, _)| step == "daemon"));
        assert!(reports.iter().any(|(step, _)| step == "skill"));
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn never_starts_the_daemon_when_one_is_already_running() {
        let root = temp_root("kimi-webbridge-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, calls) = scripted_host(vec![]);
        let fetch = WebbridgeFetch::new(vec![Some(
            json!({ "running": true, "version": "v1.11.3", "extension_connected": true }),
        )]);
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            fetch_impl: Some(Arc::new(fetch)),
            ..make_ctx(&root, plugins.handle(), host)
        });

        entry.install(noop_reporter()).await.unwrap();
        assert!(calls.lock().is_empty());
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn reinstalls_the_latest_binary_and_plugin_for_a_ready_capability() {
        let root = temp_root("kimi-webbridge-entry").await;
        let bin_path = fake_binary(&root).await;
        tokio::fs::write(&bin_path, "old-bin").await.unwrap();
        make_executable(&bin_path).await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-webbridge", true, PluginState::Ok).version("1.11.3"),
        ]);
        let (host, calls) = scripted_host(vec![]);
        let fetch = WebbridgeFetch::new(vec![Some(
            json!({ "running": true, "version": "v1.11.3", "extension_connected": true }),
        )])
        .binary(b"latest-bin");
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            fetch_impl: Some(Arc::new(fetch)),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let (report, reports) = recording_reports();

        entry.install(report).await.unwrap();

        let reports = reports.lock().clone();
        assert_eq!(reports[0].0, "download");
        assert!(reports.iter().any(|(step, _)| step == "skill"));
        assert!(calls.lock().is_empty());
        assert_eq!(
            plugins.installs.lock().as_slice(),
            ["https://code.kimi.com/kimi-code/plugins/official/kimi-webbridge.zip"]
        );
        assert_eq!(
            tokio::fs::read_to_string(&bin_path).await.unwrap(),
            "latest-bin"
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn resumes_partial_setup_without_repeating_completed_runtime_layers() {
        let root = temp_root("kimi-webbridge-entry").await;
        fake_binary(&root).await;
        let plugins = FakePluginService::new(vec![]);
        let (host, calls) = scripted_host(vec![]);
        let fetch = WebbridgeFetch::new(vec![Some(
            json!({ "running": true, "version": "v1.11.3", "extension_connected": true }),
        )]);
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            fetch_impl: Some(Arc::new(fetch)),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let (report, reports) = recording_reports();

        entry.install(report).await.unwrap();

        assert_eq!(
            reports
                .lock()
                .iter()
                .map(|(step, _)| step.clone())
                .collect::<Vec<_>>(),
            vec!["skill"]
        );
        assert!(calls.lock().is_empty());
        assert_eq!(plugins.installs.lock().len(), 1);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn refreshes_the_wiring_plugin_when_daemon_recovery_is_the_only_missing_layer() {
        let root = temp_root("kimi-webbridge-entry").await;
        let bin_path = fake_binary(&root).await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-webbridge", true, PluginState::Ok).version("1.11.3"),
        ]);
        let (host, calls) = scripted_host(vec![]);
        let fetch = WebbridgeFetch::new(vec![
            Some(json!({ "running": false })),
            Some(json!({ "running": false })),
            Some(json!({ "running": true, "version": "v1.11.3", "extension_connected": true })),
        ]);
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            fetch_impl: Some(Arc::new(fetch)),
            ..make_ctx(&root, plugins.handle(), host)
        });

        entry.install(noop_reporter()).await.unwrap();

        assert_eq!(
            plugins.installs.lock().as_slice(),
            ["https://code.kimi.com/kimi-code/plugins/official/kimi-webbridge.zip"]
        );
        assert_eq!(
            calls.lock().as_slice(),
            [format!("{} start", bin_path.display())]
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_install_on_unsupported_platforms_before_any_side_effect() {
        let root = temp_root("kimi-webbridge-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![]);
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            platform: "freebsd".to_owned(),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let error = entry.install(noop_reporter()).await.unwrap_err();
        assert!(error.to_string().contains("not supported"), "{error}");
        assert!(plugins.installs.lock().is_empty());
        rm_force(&root).await.unwrap();
    }

    // The executable bit is a POSIX concept; the TS suite only exercises it
    // meaningfully there as well.
    #[cfg(unix)]
    #[tokio::test]
    async fn treats_a_non_executable_leftover_binary_as_missing_and_re_downloads_it() {
        let root = temp_root("kimi-webbridge-entry").await;
        let bin_path = fake_binary(&root).await;
        // An install interrupted between rename and chmod leaves this behind.
        tokio::fs::write(&bin_path, "stale").await.unwrap();
        make_non_executable(&bin_path).await;
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![]);
        let fetch = WebbridgeFetch::new(vec![Some(
            json!({ "running": true, "version": "v1.11.3", "extension_connected": true }),
        )]);
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            fetch_impl: Some(Arc::new(fetch)),
            ..make_ctx(&root, plugins.handle(), host)
        });

        let detected = entry.detect().await.unwrap();
        assert_eq!(
            detected
                .steps
                .iter()
                .find(|step| step.id == "daemon-binary"),
            Some(&CapabilityStep {
                id: "daemon-binary".to_owned(),
                state: CapabilityStepState::Missing,
                detail: Some("not executable".to_owned()),
                optional: None,
            })
        );

        entry.install(noop_reporter()).await.unwrap();
        use std::os::unix::fs::MetadataExt;
        let mode = tokio::fs::metadata(&bin_path).await.unwrap().mode();
        assert_ne!(mode & 0o111, 0);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn reenables_a_previously_disabled_wiring_plugin_during_setup() {
        let root = temp_root("kimi-webbridge-entry").await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-webbridge", false, PluginState::Ok).version("1.11.3"),
        ]);
        let (host, _) = scripted_host(vec![]);
        let fetch = WebbridgeFetch::new(vec![Some(
            json!({ "running": true, "version": "v1.11.3", "extension_connected": true }),
        )]);
        let entry = create_kimi_webbridge_entry(CapabilityEntryContext {
            fetch_impl: Some(Arc::new(fetch)),
            ..make_ctx(&root, plugins.handle(), host)
        });

        // installPlugin preserves the disabled flag, but setup must not strand
        // the capability at partial by leaving the wiring off.
        entry.install(noop_reporter()).await.unwrap();
        assert_eq!(
            plugins.enabled_calls.lock().as_slice(),
            [("kimi-webbridge".to_owned(), true)]
        );
        rm_force(&root).await.unwrap();
    }
}
