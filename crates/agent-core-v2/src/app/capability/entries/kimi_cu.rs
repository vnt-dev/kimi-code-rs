//! `kimi-cu` capability entry (macOS and Windows).
//!
//! Both platforms share the same product capability and plugin wiring flow.
//! macOS adds KimiCU.app + launchd + TCC permissions; Windows uses the
//! official signed runtime installer and its built-in `doctor` command.
//!
//! The macOS path replicates the official `setup_macos.sh` step-for-step
//! (stop old processes → ditto into /Applications → register service →
//! request permissions) with structured progress and errors instead of a
//! shell pipe. Elevation when /Applications is not writable goes through
//! `osascript ... with administrator privileges` (native auth dialog).
//! Installs are detect-first and idempotent: setup always refreshes the wiring
//! plugin, only unsatisfied runtime layers are redone, and setup re-enables a
//! previously disabled wiring plugin (and its
//! MCP servers), the app step requires an executable binary with bundle
//! metadata, the archive is staged and unpacked before the old service is
//! stopped, and cleanup of old processes is best-effort — a wedged old
//! binary turns CLI probes into failed steps or is skipped past, never
//! blocking the replacement.
//! The Windows path downloads and runs the official `setup_windows.ps1`, so
//! its signature verification, rollback, and agent autostart stay upstream.
//!
//! Original: `packages/agent-core-v2/src/app/capability/entries/kimiCu.ts`.

use std::{
    collections::HashMap,
    error::Error,
    io,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Duration,
};

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;

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
        plugin::{
            GetPluginInfoInput, InstallPluginInput, SetPluginEnabledInput,
            SetPluginMcpServerEnabledInput,
        },
    },
};

use super::{
    PluginLayerConfig, context::CapabilityEntryContext, detect_plugin_layer, is_executable,
    mkdtemp_in, now_millis, path_exists,
};

const MAC_PLUGIN: PluginLayerConfig = PluginLayerConfig {
    id: "kimi-cu",
    zip_url: "https://cdn.kimi.com/kimi-computer-use/latest/kimi-cu-plugin.zip",
};
const WINDOWS_PLUGIN: PluginLayerConfig = PluginLayerConfig {
    id: "kimi-cu-win",
    zip_url: "https://cdn.kimi.com/kimi-computer-use-windows/latest/kimi-cu-win-plugin.zip",
};
const APP_ZIP_URL: &str = "https://cdn.kimi.com/kimi-computer-use/latest/KimiCU.app.zip";
const WINDOWS_SETUP_URL: &str =
    "https://cdn.kimi.com/kimi-computer-use-windows/latest/setup_windows.ps1";
const APP_BUNDLE: &str = "KimiCU.app";
const LAUNCHD_LABEL: &str = "ai.kimi.cu.service";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const PERMISSIONS_TIMEOUT: Duration = Duration::from_secs(15);
const DETECT_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const WINDOWS_INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_WINDOWS_SYSTEM_ROOT: &str = "C:\\Windows";
const WINDOWS_DOCTOR_SCRIPT: &str =
    "$candidates = @($env:KIMI_CU_WINDOWS_EXE); \
if ($env:KIMI_CU_WINDOWS_HOME) { $candidates += (Join-Path $env:KIMI_CU_WINDOWS_HOME 'kimi-cu.exe') }; \
if ($env:LOCALAPPDATA) { $candidates += (Join-Path $env:LOCALAPPDATA 'KimiCU\\kimi-cu.exe') }; \
if ($env:ProgramFiles) { $candidates += (Join-Path $env:ProgramFiles 'KimiCU\\kimi-cu.exe') }; \
$exe = $candidates | Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and (Test-Path -LiteralPath $_ -PathType Leaf) } | Select-Object -First 1; \
if (-not $exe) { exit 3 }; & $exe doctor; exit $LASTEXITCODE";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionStatus {
    pub accessibility: bool,
    pub screen_recording: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WindowsDoctorOutput {
    pub version: Option<String>,
}

#[derive(Debug)]
struct LegacyMcpFile {
    raw: String,
    value: Value,
    servers: Map<String, Value>,
}

pub fn parse_permission_status(output: &str) -> Option<PermissionStatus> {
    static PERMISSIONS_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?:permissions|permissionStatus):\s*accessibility=(true|false)\s+screenRecording=(true|false)",
        )
        .unwrap()
    });
    let captures = PERMISSIONS_RE.captures(output)?;
    Some(PermissionStatus {
        accessibility: captures.get(1)?.as_str() == "true",
        screen_recording: captures.get(2)?.as_str() == "true",
    })
}

pub fn parse_windows_doctor_output(output: &str) -> Option<WindowsDoctorOutput> {
    let mut fields = std::collections::HashMap::new();
    for line in output.split('\n') {
        let line = line.trim_end_matches('\r');
        let Some(separator) = line.find('=') else {
            continue;
        };
        if separator == 0 {
            continue;
        }
        fields.insert(line[..separator].trim(), line[separator + 1..].trim());
    }
    if fields.get("mcp") != Some(&"true") || fields.get("helper") != Some(&"embedded") {
        return None;
    }
    Some(WindowsDoctorOutput {
        version: fields.get("version").map(|version| (*version).to_owned()),
    })
}

/// Original: `windowsPowerShellPath()` — reads `SystemRoot` at call time.
pub fn windows_power_shell_path() -> String {
    windows_power_shell_path_for_root(std::env::var("SystemRoot").ok().as_deref())
}

/// Original: `windowsPowerShellPath(systemRoot)` with an explicit root.
pub fn windows_power_shell_path_for_root(system_root: Option<&str>) -> String {
    let system_root = system_root.unwrap_or(DEFAULT_WINDOWS_SYSTEM_ROOT);
    let root = if is_win32_absolute(system_root) {
        system_root
    } else {
        DEFAULT_WINDOWS_SYSTEM_ROOT
    };
    format!(
        "{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
        root.trim_end_matches(['\\', '/'])
    )
}

/// Original: node `path.win32.isAbsolute`.
fn is_win32_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        return true;
    }
    path.starts_with('\\') || path.starts_with('/')
}

pub async fn read_app_bundle_version(info_plist_path: &Path) -> Option<String> {
    static VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"<key>CFBundleShortVersionString</key>\s*<string>([^<]+)</string>").unwrap()
    });
    let xml = tokio::fs::read_to_string(info_plist_path).await.ok()?;
    Some(VERSION_RE.captures(&xml)?.get(1)?.as_str().to_owned())
}

fn apple_script_quote(script: &str) -> String {
    script.replace('\\', "\\\\").replace('"', "\\\"")
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn elevated_ditto_script(from: &str, to: &str) -> String {
    format!("/usr/bin/ditto {} {}", sh_quote(from), sh_quote(to))
}

// Original: parseLegacyMcpFile() — recognizes only the exact standalone
// `kimi-cu` MCP registration shape written by older installers.
fn parse_legacy_mcp_file(raw: &str, app_bin: &str) -> Option<LegacyMcpFile> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let servers = value.as_object()?.get("mcpServers")?.as_object()?.clone();
    let legacy = servers.get("kimi-cu")?.as_object()?;
    if legacy.get("command")?.as_str()? != app_bin {
        return None;
    }
    if legacy.get("enabled") == Some(&Value::Bool(false)) {
        return None;
    }
    let args = legacy.get("args")?.as_array()?;
    let args: Vec<&str> = args.iter().map(Value::as_str).collect::<Option<Vec<_>>>()?;
    let known_args = (args.len() == 1 && args[0] == "mcp")
        || (args.len() == 3 && args[0] == "mcp" && args[1] == "-s" && args[2] == "user");
    if !known_args {
        return None;
    }
    if legacy.keys().any(|key| key != "args" && key != "command") {
        return None;
    }
    Some(LegacyMcpFile {
        raw: raw.to_owned(),
        value,
        servers,
    })
}

// Original: `error instanceof Error ? error.message : String(error)`.
fn boxed_error(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(ExpectedError::new(message))
}

fn non_empty_or_code(stderr: &str, code: i32) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        code.to_string()
    } else {
        trimmed.to_owned()
    }
}

// Original: `stderr.trim() || stdout.trim() || `exit code ${code}``.
fn trim_or_fallback(stderr: &str, stdout: &str, code: i32) -> String {
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_owned();
    }
    let stdout = stdout.trim();
    if !stdout.is_empty() {
        return stdout.to_owned();
    }
    format!("exit code {code}")
}

// Original: `process.getuid()` — only called on macOS install paths.
#[cfg(unix)]
fn current_uid() -> String {
    // SAFETY: getuid has no failure modes.
    unsafe { libc::getuid() }.to_string()
}

#[cfg(not(unix))]
fn current_uid() -> String {
    "501".to_owned()
}

#[cfg(unix)]
async fn file_mode(path: &Path) -> io::Result<u32> {
    use std::os::unix::fs::MetadataExt;
    Ok(tokio::fs::metadata(path).await?.mode() & 0o777)
}

#[cfg(not(unix))]
async fn file_mode(_: &Path) -> io::Result<u32> {
    Ok(0o644)
}

// Original: installPluginLayer() — upsert the wiring plugin, re-enable it
// when a previous install left it disabled, then enable any MCP server that
// stayed off.
async fn install_plugin_layer(
    ctx: &CapabilityEntryContext,
    config: &PluginLayerConfig,
) -> CapabilityEntryResult<()> {
    let summary = ctx
        .plugins
        .install_plugin(InstallPluginInput {
            source: config.zip_url.to_owned(),
        })
        .await?;
    if !summary.enabled {
        ctx.plugins
            .set_plugin_enabled(SetPluginEnabledInput {
                id: config.id.to_owned(),
                enabled: true,
            })
            .await?;
    }
    if summary.enabled_mcp_server_count >= summary.mcp_server_count {
        return Ok(());
    }
    let info = ctx
        .plugins
        .get_plugin_info(GetPluginInfoInput {
            id: config.id.to_owned(),
        })
        .await?;
    for server in &info.mcp_servers {
        if !server.enabled {
            ctx.plugins
                .set_plugin_mcp_server_enabled(SetPluginMcpServerEnabledInput {
                    id: config.id.to_owned(),
                    server: server.name.clone(),
                    enabled: true,
                })
                .await?;
        }
    }
    Ok(())
}

struct MacKimiCuEntry {
    ctx: CapabilityEntryContext,
    applications_dir: PathBuf,
    app_path: PathBuf,
    app_bin: PathBuf,
    info_plist: PathBuf,
    probe_timeout: Duration,
    command_timeout: Duration,
    supported: bool,
    user_mcp_config_path: PathBuf,
}

impl MacKimiCuEntry {
    fn new(ctx: CapabilityEntryContext) -> Self {
        let applications_dir = ctx
            .applications_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("/Applications"));
        let app_path = applications_dir.join(APP_BUNDLE);
        let app_bin = app_path.join("Contents").join("MacOS").join("kimi-cu");
        let info_plist = app_path.join("Contents").join("Info.plist");
        Self {
            probe_timeout: ctx.detect_probe_timeout.unwrap_or(DETECT_PROBE_TIMEOUT),
            command_timeout: ctx.command_timeout.unwrap_or(COMMAND_TIMEOUT),
            supported: ctx.platform == "darwin",
            user_mcp_config_path: ctx.kimi_home_dir.join("mcp.json"),
            applications_dir,
            app_path,
            app_bin,
            info_plist,
            ctx,
        }
    }

    fn app_bin_string(&self) -> String {
        self.app_bin.to_string_lossy().into_owned()
    }

    async fn service_running(&self) -> CapabilityEntryResult<bool> {
        static STATUS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"status=1\b").unwrap());
        if !path_exists(&self.app_bin).await {
            return Ok(false);
        }
        let result = run_command(
            &self.ctx.host_process,
            &self.app_bin_string(),
            &["service-status".to_owned()],
            Some(self.probe_timeout),
        )
        .await?;
        Ok(STATUS_RE.is_match(&result.stdout))
    }

    async fn permission_status(&self) -> CapabilityEntryResult<Option<PermissionStatus>> {
        if !path_exists(&self.app_bin).await {
            return Ok(None);
        }
        let result = run_command(
            &self.ctx.host_process,
            &self.app_bin_string(),
            &["xpc-ping".to_owned()],
            Some(self.probe_timeout),
        )
        .await?;
        Ok(parse_permission_status(&result.stdout))
    }

    async fn legacy_mcp_file(&self) -> Option<LegacyMcpFile> {
        let raw = tokio::fs::read_to_string(&self.user_mcp_config_path)
            .await
            .ok()?;
        parse_legacy_mcp_file(&raw, &self.app_bin_string())
    }

    // Original: removeLegacyMcpRegistration() — compare-and-swap the user
    // config through a same-directory temp file so a concurrent edit is
    // detected instead of clobbered.
    async fn remove_legacy_mcp_registration(&self, legacy: Option<&LegacyMcpFile>) -> io::Result<bool> {
        let Some(legacy) = legacy else {
            return Ok(false);
        };
        let mut next_servers = legacy.servers.clone();
        next_servers.remove("kimi-cu");
        let mut next = legacy.value.clone();
        next.as_object_mut()
            .expect("legacy mcp root is an object")
            .insert("mcpServers".to_owned(), Value::Object(next_servers));
        let mode = file_mode(&self.user_mcp_config_path).await?;
        let temp_path = PathBuf::from(format!(
            "{}.kimi-cu-migration-{}-{}",
            self.user_mcp_config_path.display(),
            std::process::id(),
            now_millis()
        ));
        let migrated = async {
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }
            #[cfg(not(unix))]
            let _ = mode;
            let mut file = options.open(&temp_path).await?;
            file.write_all(format!("{}\n", serde_json::to_string_pretty(&next)?).as_bytes())
                .await?;
            file.flush().await?;
            drop(file);
            if tokio::fs::read_to_string(&self.user_mcp_config_path).await? != legacy.raw {
                return Ok(false);
            }
            tokio::fs::rename(&temp_path, &self.user_mcp_config_path).await?;
            Ok(true)
        }
        .await;
        let _ = tokio::fs::remove_file(&temp_path).await;
        migrated
    }

    async fn detect(&self) -> CapabilityEntryResult<CapabilityDetectResult> {
        let mut steps = Vec::new();

        let plugin = detect_plugin_layer(&self.ctx, &MAC_PLUGIN, "plugin").await?;
        let plugin_version = plugin.version.clone();
        steps.push(plugin.step);

        if self.legacy_mcp_file().await.is_some() {
            steps.push(CapabilityStep {
                id: "legacy-mcp".to_owned(),
                state: CapabilityStepState::Missing,
                detail: Some("duplicate standalone kimi-cu MCP registration".to_owned()),
                optional: Some(true),
            });
        }

        let version = read_app_bundle_version(&self.info_plist).await;
        let app_exists = path_exists(&self.app_bin).await;
        let app_usable = app_exists
            && is_executable(&self.app_bin).await
            && path_exists(&self.info_plist).await;
        steps.push(CapabilityStep {
            id: "app".to_owned(),
            state: if app_usable {
                CapabilityStepState::Ok
            } else {
                CapabilityStepState::Missing
            },
            detail: if app_exists && !app_usable {
                Some("not executable".to_owned())
            } else {
                version.clone()
            },
            optional: None,
        });

        match self.service_running().await {
            Ok(running) => steps.push(CapabilityStep::new(
                "service",
                if running {
                    CapabilityStepState::Ok
                } else {
                    CapabilityStepState::Missing
                },
            )),
            Err(error) => steps.push(CapabilityStep {
                id: "service".to_owned(),
                state: CapabilityStepState::Failed,
                detail: Some(error.to_string()),
                optional: None,
            }),
        }

        match self.permission_status().await {
            Err(error) => steps.push(CapabilityStep {
                id: "permissions".to_owned(),
                state: CapabilityStepState::Failed,
                detail: Some(error.to_string()),
                optional: None,
            }),
            Ok(permissions) => {
                let granted = permissions
                    .as_ref()
                    .is_some_and(|status| status.accessibility && status.screen_recording);
                let missing = permissions.map(|status| {
                    [
                        (!status.accessibility).then_some("accessibility"),
                        (!status.screen_recording).then_some("screenRecording"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(",")
                });
                steps.push(CapabilityStep {
                    id: "permissions".to_owned(),
                    state: if granted {
                        CapabilityStepState::Ok
                    } else {
                        CapabilityStepState::Missing
                    },
                    detail: match missing {
                        _ if granted => None,
                        Some(missing) if !missing.is_empty() => Some(missing),
                        _ => None,
                    },
                    optional: None,
                });
            }
        }

        Ok(CapabilityDetectResult {
            steps,
            version: version.or(plugin_version),
        })
    }

    async fn best_effort(&self, command: &str, args: &[String]) {
        let _ = run_command(
            &self.ctx.host_process,
            command,
            args,
            Some(self.command_timeout),
        )
        .await;
    }

    // Original: stopOldProcesses() — best-effort; a wedged old binary never
    // blocks the replacement.
    async fn stop_old_processes(&self) {
        let uid = current_uid();
        if path_exists(&self.app_bin).await {
            self.best_effort(&self.app_bin_string(), &["uninstall".to_owned()])
                .await;
        }
        self.best_effort(
            "launchctl",
            &["bootout".to_owned(), format!("gui/{uid}/{LAUNCHD_LABEL}")],
        )
        .await;
        // Keep connected MCP frontends alive while the app bundle is replaced.
        // Their work is delegated to the service below; killing them makes the
        // client report an installation-driven restart as an unexpected failure.
        for mode in ["service", "overlay"] {
            self.best_effort(
                "pkill",
                &[
                    "-f".to_owned(),
                    format!("{APP_BUNDLE}/Contents/MacOS/kimi-cu[[:space:]]+{mode}"),
                ],
            )
            .await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    async fn move_app_into_place(&self, unzipped_app: &Path) -> CapabilityEntryResult<()> {
        let _ = rm_force(&self.app_path).await;
        let from = unzipped_app.to_string_lossy().into_owned();
        let to = self.app_path.to_string_lossy().into_owned();
        let direct = run_command(
            &self.ctx.host_process,
            "ditto",
            &[from.clone(), to.clone()],
            Some(self.command_timeout),
        )
        .await?;
        if direct.code == 0 {
            return Ok(());
        }
        let script = apple_script_quote(&elevated_ditto_script(&from, &to));
        let elevated = run_command(
            &self.ctx.host_process,
            "osascript",
            &[
                "-e".to_owned(),
                format!("do shell script \"{script}\" with administrator privileges"),
            ],
            Some(Duration::from_secs(120)),
        )
        .await?;
        if elevated.code != 0 {
            return Err(boxed_error(format!(
                "Failed to install {APP_BUNDLE} into {} (direct: {}; elevated: {})",
                self.applications_dir.display(),
                non_empty_or_code(&direct.stderr, direct.code),
                non_empty_or_code(&elevated.stderr, elevated.code),
            )));
        }
        Ok(())
    }

    async fn install_app_bundle(
        &self,
        work_dir: &Path,
        report: &CapabilityInstallReporter,
    ) -> CapabilityEntryResult<()> {
        report("download", Some(0));
        let zip_path = work_dir.join("KimiCU.app.zip");
        let fetch = self.ctx.fetch_impl_or_default();
        download_to_file(
            APP_ZIP_URL,
            &zip_path,
            Some(&|percent| report("download", Some(percent))),
            &fetch,
            None,
        )
        .await?;

        report("app", None);
        let unzip_dir = work_dir.join("unzipped");
        let unzipped = run_command(
            &self.ctx.host_process,
            "ditto",
            &[
                "-x".to_owned(),
                "-k".to_owned(),
                zip_path.to_string_lossy().into_owned(),
                unzip_dir.to_string_lossy().into_owned(),
            ],
            Some(Duration::from_secs(120)),
        )
        .await?;
        if unzipped.code != 0 {
            return Err(boxed_error(format!(
                "Failed to unzip KimiCU.app: {}",
                if unzipped.stderr.is_empty() {
                    &unzipped.stdout
                } else {
                    &unzipped.stderr
                }
            )));
        }
        self.stop_old_processes().await;
        self.move_app_into_place(&unzip_dir.join(APP_BUNDLE)).await?;
        run_command(
            &self.ctx.host_process,
            "xattr",
            &[
                "-dr".to_owned(),
                "com.apple.quarantine".to_owned(),
                self.app_path.to_string_lossy().into_owned(),
            ],
            Some(self.command_timeout),
        )
        .await?;
        Ok(())
    }

    async fn install(&self, report: CapabilityInstallReporter) -> CapabilityEntryResult<()> {
        if !self.supported {
            return Err(boxed_error(format!(
                "kimi-cu is only supported on macOS (current: {})",
                self.ctx.platform
            )));
        }

        let before = self.detect().await?;
        let legacy_mcp_before = self.legacy_mcp_file().await;
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

        report("plugin", None);
        install_plugin_layer(&self.ctx, &MAC_PLUGIN).await?;

        // A read-only or concurrently edited user config must not block the app
        // installation. Detection keeps the duplicate as an optional warning so
        // clients can record it in logs and a later install can retry migration.
        if self
            .remove_legacy_mcp_registration(legacy_mcp_before.as_ref())
            .await
            .unwrap_or(false)
        {
            report("mcp-config", None);
        }

        let install_app = step_states.get("app") != Some(&CapabilityStepState::Ok) || ready_before;
        if install_app {
            let work_dir = mkdtemp_in(&std::env::temp_dir(), "kimi-cu-install-").await?;
            let result = self.install_app_bundle(&work_dir, &report).await;
            let _ = rm_force(&work_dir).await;
            result?;
        }

        if install_app || step_states.get("service") != Some(&CapabilityStepState::Ok) {
            report("service", None);
            let registered = run_command(
                &self.ctx.host_process,
                &self.app_bin_string(),
                &["install".to_owned()],
                Some(self.command_timeout),
            )
            .await?;
            if registered.code != 0 {
                return Err(boxed_error(format!(
                    "kimi-cu install failed: {}",
                    if registered.stderr.is_empty() {
                        &registered.stdout
                    } else {
                        &registered.stderr
                    }
                )));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            let running = self.service_running().await.unwrap_or(false);
            if !running {
                return Err(boxed_error(
                    "kimi-cu background service is not running after install",
                ));
            }
        }

        if step_states.get("permissions") != Some(&CapabilityStepState::Ok) {
            report("permissions", None);
            let _ = run_command(
                &self.ctx.host_process,
                &self.app_bin_string(),
                &[
                    "request-permissions".to_owned(),
                    "--ax".to_owned(),
                    "--screen".to_owned(),
                ],
                Some(PERMISSIONS_TIMEOUT),
            )
            .await;
        }
        Ok(())
    }
}

#[async_trait]
impl CapabilityEntry for MacKimiCuEntry {
    fn id(&self) -> CapabilityId {
        CapabilityId::KimiCu
    }

    fn plugin_id(&self) -> Option<&str> {
        Some(MAC_PLUGIN.id)
    }

    fn display_name(&self) -> &str {
        "Kimi Computer Use"
    }

    fn description(&self) -> &str {
        "macOS GUI automation in the background — read app UIs and click, type, scroll, and drag without taking over your mouse or foregrounding apps."
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

struct RuntimeDetection {
    step: CapabilityStep,
    version: Option<String>,
}

struct WindowsKimiCuEntry {
    ctx: CapabilityEntryContext,
    supported: bool,
    probe_timeout: Duration,
    install_timeout: Duration,
    powershell_path: String,
}

impl WindowsKimiCuEntry {
    fn new(ctx: CapabilityEntryContext) -> Self {
        Self {
            supported: ctx.platform == "win32" && ctx.arch == "x64",
            probe_timeout: ctx.detect_probe_timeout.unwrap_or(DETECT_PROBE_TIMEOUT),
            install_timeout: ctx.command_timeout.unwrap_or(WINDOWS_INSTALL_TIMEOUT),
            powershell_path: windows_power_shell_path(),
            ctx,
        }
    }

    // Original: runtimeStep() — probe the installed runtime through its
    // `doctor` command; exit code 3 means "not installed".
    async fn runtime_step(&self) -> RuntimeDetection {
        let result = run_command(
            &self.ctx.host_process,
            &self.powershell_path,
            &[
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-Command".to_owned(),
                WINDOWS_DOCTOR_SCRIPT.to_owned(),
            ],
            Some(self.probe_timeout),
        )
        .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                return RuntimeDetection {
                    step: CapabilityStep {
                        id: "runtime".to_owned(),
                        state: CapabilityStepState::Failed,
                        detail: Some(error.to_string()),
                        optional: None,
                    },
                    version: None,
                };
            }
        };
        if result.code == 3 {
            return RuntimeDetection {
                step: CapabilityStep::new("runtime", CapabilityStepState::Missing),
                version: None,
            };
        }
        if result.code != 0 {
            return RuntimeDetection {
                step: CapabilityStep {
                    id: "runtime".to_owned(),
                    state: CapabilityStepState::Failed,
                    detail: Some(trim_or_fallback(&result.stderr, &result.stdout, result.code)),
                    optional: None,
                },
                version: None,
            };
        }
        match parse_windows_doctor_output(&result.stdout) {
            None => RuntimeDetection {
                step: CapabilityStep {
                    id: "runtime".to_owned(),
                    state: CapabilityStepState::Failed,
                    detail: Some("doctor returned unexpected output".to_owned()),
                    optional: None,
                },
                version: None,
            },
            Some(doctor) => RuntimeDetection {
                step: CapabilityStep {
                    id: "runtime".to_owned(),
                    state: CapabilityStepState::Ok,
                    detail: doctor.version.clone(),
                    optional: None,
                },
                version: doctor.version,
            },
        }
    }

    async fn detect(&self) -> CapabilityEntryResult<CapabilityDetectResult> {
        let (plugin, runtime) = tokio::join!(
            detect_plugin_layer(&self.ctx, &WINDOWS_PLUGIN, "plugin"),
            self.runtime_step()
        );
        let plugin = plugin?;
        Ok(CapabilityDetectResult {
            steps: vec![plugin.step, runtime.step],
            version: runtime.version.or(plugin.version),
        })
    }

    async fn install_runtime(
        &self,
        work_dir: &Path,
        report: &CapabilityInstallReporter,
    ) -> CapabilityEntryResult<()> {
        let setup_path = work_dir.join("setup_windows.ps1");
        report("download", Some(0));
        let fetch = self.ctx.fetch_impl_or_default();
        download_to_file(
            WINDOWS_SETUP_URL,
            &setup_path,
            Some(&|percent| report("download", Some(percent))),
            &fetch,
            None,
        )
        .await?;

        report("runtime", None);
        let installed = run_command(
            &self.ctx.host_process,
            &self.powershell_path,
            &[
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-ExecutionPolicy".to_owned(),
                "Bypass".to_owned(),
                "-File".to_owned(),
                setup_path.to_string_lossy().into_owned(),
            ],
            Some(self.install_timeout),
        )
        .await?;
        if installed.code != 0 {
            return Err(boxed_error(format!(
                "kimi-cu Windows runtime install failed: {}",
                trim_or_fallback(&installed.stderr, &installed.stdout, installed.code)
            )));
        }
        Ok(())
    }

    async fn install(&self, report: CapabilityInstallReporter) -> CapabilityEntryResult<()> {
        if !self.supported {
            return Err(boxed_error(format!(
                "kimi-cu is only supported on macOS or Windows x64 (current: {}/{})",
                self.ctx.platform, self.ctx.arch
            )));
        }

        let before = self.detect().await?;
        let step_states: HashMap<&str, CapabilityStepState> = before
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step.state))
            .collect();
        let ready_before = before
            .steps
            .iter()
            .all(|step| step.state == CapabilityStepState::Ok);

        report("plugin", None);
        install_plugin_layer(&self.ctx, &WINDOWS_PLUGIN).await?;

        if step_states.get("runtime") != Some(&CapabilityStepState::Ok) || ready_before {
            let work_dir = mkdtemp_in(&std::env::temp_dir(), "kimi-cu-windows-install-").await?;
            let result = self.install_runtime(&work_dir, &report).await;
            let _ = rm_force(&work_dir).await;
            result?;

            let runtime = self.runtime_step().await;
            if runtime.step.state != CapabilityStepState::Ok {
                return Err(boxed_error(format!(
                    "kimi-cu Windows runtime is not ready after install: {}",
                    runtime
                        .step
                        .detail
                        .unwrap_or_else(|| runtime.step.state.as_str().to_owned())
                )));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl CapabilityEntry for WindowsKimiCuEntry {
    fn id(&self) -> CapabilityId {
        CapabilityId::KimiCu
    }

    fn plugin_id(&self) -> Option<&str> {
        Some(WINDOWS_PLUGIN.id)
    }

    fn display_name(&self) -> &str {
        "Kimi Computer Use"
    }

    fn description(&self) -> &str {
        "Windows GUI automation — read app UIs and click, type, scroll, and drag in desktop apps."
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

/// Original: `createKimiCuEntry()` — platform dispatch at construction time.
pub fn create_kimi_cu_entry(ctx: CapabilityEntryContext) -> Arc<dyn CapabilityEntry> {
    if ctx.platform == "win32" {
        Arc::new(WindowsKimiCuEntry::new(ctx))
    } else {
        Arc::new(MacKimiCuEntry::new(ctx))
    }
}

#[cfg(test)]
mod tests {
    //! `kimi-cu` capability entry — macOS and Windows platform selection,
    //! layered detection, and install orchestration. Host effects are faked
    //! (temp app bundle, scripted host processes, fake plugins).
    //!
    //! Original: `packages/agent-core-v2/test/app/capability/kimiCu.test.ts`.

    use std::{
        collections::VecDeque,
        path::PathBuf,
        sync::Mutex,
        time::Duration,
    };

    use serde_json::json;

    use crate::{
        app::{
            capability::{entries::test_fakes::*, host::FetchLike},
            plugin::PluginState,
        },
        os::interface::host_process::{
            HostProcess, HostProcessError, HostProcessOptions, HostProcessService,
            HostProcessServiceHandle,
        },
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

    async fn fake_app_bundle(root: &Path) -> PathBuf {
        let applications_dir = root.join("Applications");
        let macos_dir = applications_dir
            .join("KimiCU.app")
            .join("Contents")
            .join("MacOS");
        tokio::fs::create_dir_all(&macos_dir).await.unwrap();
        let app_bin = macos_dir.join("kimi-cu");
        tokio::fs::write(&app_bin, "#!/bin/sh\n").await.unwrap();
        // Real bundles are executable; anything less reads as a broken install.
        make_executable(&app_bin).await;
        tokio::fs::write(
            applications_dir
                .join("KimiCU.app")
                .join("Contents")
                .join("Info.plist"),
            "<key>CFBundleShortVersionString</key>\n<string>0.5.4</string>",
        )
        .await
        .unwrap();
        applications_dir
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

    type RecordedReports = Arc<Mutex<Vec<(String, Option<u32>)>>>;

    fn recording_reports() -> (CapabilityInstallReporter, RecordedReports) {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&reports);
        (
            Box::new(move |step, percent| {
                recording.lock().unwrap().push((step.to_owned(), percent));
            }),
            reports,
        )
    }

    fn noop_reporter() -> CapabilityInstallReporter {
        Box::new(|_, _| {})
    }

    fn step_names(reports: &[(String, Option<u32>)]) -> Vec<String> {
        reports.iter().map(|(step, _)| step.clone()).collect()
    }

    #[test]
    fn parses_the_machine_readable_request_permissions_output() {
        assert_eq!(
            parse_permission_status("permissions: accessibility=true screenRecording=true"),
            Some(PermissionStatus {
                accessibility: true,
                screen_recording: true,
            })
        );
        assert_eq!(
            parse_permission_status("permissions: accessibility=true screenRecording=false"),
            Some(PermissionStatus {
                accessibility: true,
                screen_recording: false,
            })
        );
        assert_eq!(
            parse_permission_status("permissionStatus: accessibility=false screenRecording=true"),
            Some(PermissionStatus {
                accessibility: false,
                screen_recording: true,
            })
        );
        assert_eq!(parse_permission_status("unknown command"), None);
        assert_eq!(parse_permission_status(""), None);
    }

    #[test]
    fn doctor_output_accepts_only_an_mcp_capable_embedded_runtime() {
        assert_eq!(
            parse_windows_doctor_output(
                "version=0.2.14\r\nmcp=true\r\nhelper=embedded\r\nagent=running\r\n",
            ),
            Some(WindowsDoctorOutput {
                version: Some("0.2.14".to_owned()),
            })
        );
        assert_eq!(parse_windows_doctor_output("mcp=false\nhelper=embedded"), None);
        assert_eq!(parse_windows_doctor_output("mcp=true\nhelper=external"), None);
    }

    #[test]
    fn powershell_path_always_resolves_the_system_executable_absolutely() {
        assert_eq!(
            windows_power_shell_path_for_root(Some("D:\\Windows")),
            "D:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
        assert!(is_win32_absolute(&windows_power_shell_path_for_root(Some(
            "relative"
        ))));
    }

    #[test]
    fn elevated_ditto_script_shell_quotes_both_paths_so_metacharacters_stay_literal() {
        // The elevated path runs the string through /bin/sh with administrator
        // privileges: every path must be exactly one literal argument.
        assert_eq!(
            elevated_ditto_script("/tmp/kimi cu/app", "/Applications/KimiCU.app"),
            "/usr/bin/ditto '/tmp/kimi cu/app' '/Applications/KimiCU.app'"
        );
        assert_eq!(
            elevated_ditto_script("$(touch /tmp/pwned); echo '", "/Applications/KimiCU.app"),
            "/usr/bin/ditto '$(touch /tmp/pwned); echo '\\''' '/Applications/KimiCU.app'"
        );
    }

    #[tokio::test]
    async fn reads_cf_bundle_short_version_string_from_info_plist() {
        let root = temp_root("kimi-cu-version").await;
        let plist = root.join("Info.plist");
        tokio::fs::write(
            &plist,
            "<?xml version=\"1.0\"?><plist><dict>\n<key>CFBundleShortVersionString</key>\n<string>0.4.18</string>\n</dict></plist>",
        )
        .await
        .unwrap();
        assert_eq!(
            read_app_bundle_version(&plist).await.as_deref(),
            Some("0.4.18")
        );
        assert_eq!(read_app_bundle_version(&root.join("nope.plist")).await, None);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn supports_macos_and_windows_x64_under_one_capability_id() {
        let root = temp_root("kimi-cu-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![]);
        assert!(create_kimi_cu_entry(make_ctx(&root, plugins.handle(), host.clone())).supported());
        let linux = create_kimi_cu_entry(CapabilityEntryContext {
            platform: "linux".to_owned(),
            ..make_ctx(&root, plugins.handle(), host.clone())
        });
        assert!(!linux.supported());
        let windows = create_kimi_cu_entry(CapabilityEntryContext {
            platform: "win32".to_owned(),
            arch: "x64".to_owned(),
            ..make_ctx(&root, plugins.handle(), host.clone())
        });
        assert_eq!(windows.id(), CapabilityId::KimiCu);
        assert_eq!(windows.plugin_id(), Some("kimi-cu-win"));
        assert!(windows.supported());
        let windows_arm = create_kimi_cu_entry(CapabilityEntryContext {
            platform: "win32".to_owned(),
            arch: "arm64".to_owned(),
            ..make_ctx(&root, plugins.handle(), host)
        });
        assert!(!windows_arm.supported());
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn detects_the_windows_plugin_and_signed_runtime_through_doctor() {
        let root = temp_root("kimi-cu-entry").await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-cu-win", true, PluginState::Ok).version("0.2.14"),
        ]);
        let (host, calls) = scripted_host(vec![SpawnScript::new("-Command", 0).stdout(
            "version=0.2.14\r\nmcp=true\r\nhelper=embedded\r\nagent=running\r\n",
        )]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            platform: "win32".to_owned(),
            arch: "x64".to_owned(),
            ..make_ctx(&root, plugins.handle(), host)
        });

        let detected = entry.detect().await.unwrap();
        assert_eq!(
            detected,
            CapabilityDetectResult {
                version: Some("0.2.14".to_owned()),
                steps: vec![
                    CapabilityStep {
                        id: "plugin".to_owned(),
                        state: CapabilityStepState::Ok,
                        detail: Some("0.2.14".to_owned()),
                        optional: None,
                    },
                    CapabilityStep {
                        id: "runtime".to_owned(),
                        state: CapabilityStepState::Ok,
                        detail: Some("0.2.14".to_owned()),
                        optional: None,
                    },
                ],
            }
        );
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].starts_with(&format!(
                "{} -NoProfile -NonInteractive -Command ",
                windows_power_shell_path()
            )),
            "{}",
            calls[0]
        );
        rm_force(&root).await.unwrap();
    }

    /// Original: the inline spawn fake sequencing `doctor` results — first
    /// probe "not installed", post-install probe healthy.
    struct DoctorSequenceSpawn {
        calls: Arc<Mutex<Vec<String>>>,
        doctor_results: Arc<Mutex<VecDeque<(i32, String, String)>>>,
    }

    #[async_trait]
    impl HostProcessService for DoctorSequenceSpawn {
        async fn spawn(
            &self,
            command: &str,
            args: &[String],
            _: HostProcessOptions,
        ) -> Result<Arc<dyn HostProcess>, HostProcessError> {
            let key = format!("{} {}", command, args.join(" "));
            self.calls.lock().unwrap().push(key);
            if args.iter().any(|arg| arg == "-Command") {
                let (code, stdout, stderr) = self
                    .doctor_results
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or((1, String::new(), "unexpected doctor".to_owned()));
                return Ok(Arc::new(FakeHostProcess {
                    code,
                    stdout,
                    stderr,
                    hang: false,
                }));
            }
            let code = if args.iter().any(|arg| arg == "-File") {
                0
            } else {
                1
            };
            Ok(Arc::new(FakeHostProcess {
                code,
                stdout: String::new(),
                stderr: String::new(),
                hang: false,
            }))
        }
    }

    #[tokio::test]
    async fn installs_windows_with_the_official_setup_script_and_shared_plugin_wiring() {
        let root = temp_root("kimi-cu-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let doctor_results = Arc::new(Mutex::new(VecDeque::from([
            (3, String::new(), String::new()),
            (
                0,
                "version=0.2.14\r\nmcp=true\r\nhelper=embedded\r\nagent=running\r\n".to_owned(),
                String::new(),
            ),
        ])));
        let host = HostProcessServiceHandle(Arc::new(DoctorSequenceSpawn {
            calls: Arc::clone(&calls),
            doctor_results: Arc::clone(&doctor_results),
        }));
        let fetch: Arc<dyn FetchLike> = Arc::new(BytesFetch {
            bytes: b"Write-Host 'official setup'".to_vec(),
        });
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            platform: "win32".to_owned(),
            arch: "x64".to_owned(),
            fetch_impl: Some(fetch),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let (report, reports) = recording_reports();

        entry.install(report).await.unwrap();

        assert_eq!(
            plugins.installs.lock().unwrap().as_slice(),
            ["https://cdn.kimi.com/kimi-computer-use-windows/latest/kimi-cu-win-plugin.zip"]
        );
        let reports = reports.lock().unwrap().clone();
        assert!(reports.contains(&("plugin".to_owned(), None)));
        assert!(reports.contains(&("download".to_owned(), Some(0))));
        assert!(reports.contains(&("download".to_owned(), Some(100))));
        assert!(reports.contains(&("runtime".to_owned(), None)));
        let calls = calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .any(|call| call.contains("-ExecutionPolicy Bypass -File"))
        );
        let powershell = windows_power_shell_path();
        assert!(calls.iter().all(|call| call.starts_with(&powershell)));
        assert!(doctor_results.lock().unwrap().is_empty());
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn does_not_reinstall_a_healthy_windows_runtime_when_only_the_plugin_is_missing() {
        let root = temp_root("kimi-cu-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, calls) = scripted_host(vec![SpawnScript::new("-Command", 0).stdout(
            "version=0.2.14\nmcp=true\nhelper=embedded\nagent=running\n",
        )]);
        let fetch: Arc<dyn FetchLike> = Arc::new(FailingFetch {
            message: "download should be skipped",
        });
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            platform: "win32".to_owned(),
            arch: "x64".to_owned(),
            fetch_impl: Some(fetch),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let (report, reports) = recording_reports();

        entry.install(report).await.unwrap();

        assert_eq!(step_names(&reports.lock().unwrap()), vec!["plugin"]);
        assert_eq!(calls.lock().unwrap().len(), 1);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn detects_all_four_layers_with_details() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-cu", true, PluginState::Ok).version("0.5.4"),
        ]);
        let (host, calls) = scripted_host(vec![
            SpawnScript::new("service-status", 0)
                .stdout("SMAppService status=1 (1=enabled); fallback plist exists=false"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=false"),
        ]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });

        let detected = entry.detect().await.unwrap();
        assert_eq!(detected.version.as_deref(), Some("0.5.4"));
        assert_eq!(
            detected.steps,
            vec![
                CapabilityStep {
                    id: "plugin".to_owned(),
                    state: CapabilityStepState::Ok,
                    detail: Some("0.5.4".to_owned()),
                    optional: None,
                },
                CapabilityStep {
                    id: "app".to_owned(),
                    state: CapabilityStepState::Ok,
                    detail: Some("0.5.4".to_owned()),
                    optional: None,
                },
                CapabilityStep::new("service", CapabilityStepState::Ok),
                CapabilityStep {
                    id: "permissions".to_owned(),
                    state: CapabilityStepState::Missing,
                    detail: Some("screenRecording".to_owned()),
                    optional: None,
                },
            ]
        );
        let calls = calls.lock().unwrap().clone();
        assert!(calls.iter().any(|call| call.ends_with(" xpc-ping")));
        assert!(!calls.iter().any(|call| call.contains("request-permissions")));
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn reports_missing_layers_on_a_bare_machine() {
        let root = temp_root("kimi-cu-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(root.join("Applications")),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let detected = entry.detect().await.unwrap();
        assert_eq!(detected.version, None);
        let states: Vec<(&str, CapabilityStepState)> = detected
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step.state))
            .collect();
        assert_eq!(
            states,
            vec![
                ("plugin", CapabilityStepState::Missing),
                ("app", CapabilityStepState::Missing),
                ("service", CapabilityStepState::Missing),
                ("permissions", CapabilityStepState::Missing),
            ]
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_install_on_non_macos_before_any_side_effect() {
        let root = temp_root("kimi-cu-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            platform: "linux".to_owned(),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let error = entry.install(noop_reporter()).await.unwrap_err();
        assert!(error.to_string().contains("only supported on macOS"), "{error}");
        assert!(plugins.installs.lock().unwrap().is_empty());
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn resumes_a_partial_install_without_repeating_completed_runtime_layers() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let plugins = FakePluginService::new(vec![]);
        let (host, calls) = scripted_host(vec![
            SpawnScript::new("service-status", 0).stdout("SMAppService status=1"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=true"),
        ]);
        let fetch: Arc<dyn FetchLike> = Arc::new(FailingFetch {
            message: "download should be skipped",
        });
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            fetch_impl: Some(fetch),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let (report, reports) = recording_reports();

        entry.install(report).await.unwrap();

        assert_eq!(plugins.installs.lock().unwrap().len(), 1);
        assert_eq!(step_names(&reports.lock().unwrap()), vec!["plugin"]);
        let calls = calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .all(|call| call.contains("service-status") || call.contains("xpc-ping")),
            "{calls:?}"
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn migrates_the_exact_legacy_standalone_mcp_registration_after_installing_the_plugin() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let app_bin = applications_dir
            .join("KimiCU.app")
            .join("Contents")
            .join("MacOS")
            .join("kimi-cu");
        let kimi_home = root.join("kimi-home");
        tokio::fs::create_dir_all(&kimi_home).await.unwrap();
        tokio::fs::write(
            kimi_home.join("mcp.json"),
            format!(
                "{}\n",
                serde_json::to_string(&json!({
                    "mcpServers": {
                        "kimi-cu": {
                            "command": app_bin.to_string_lossy(),
                            "args": ["mcp", "-s", "user"],
                        },
                        "custom": { "command": "custom-mcp", "args": [] },
                    },
                }))
                .unwrap()
            ),
        )
        .await
        .unwrap();
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![
            SpawnScript::new("service-status", 0).stdout("SMAppService status=1"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=true"),
        ]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });

        let detected = entry.detect().await.unwrap();
        assert!(
            detected.steps.contains(&CapabilityStep {
                id: "legacy-mcp".to_owned(),
                state: CapabilityStepState::Missing,
                detail: Some("duplicate standalone kimi-cu MCP registration".to_owned()),
                optional: Some(true),
            }),
            "{:?}",
            detected.steps
        );
        let (report, reports) = recording_reports();
        entry.install(report).await.unwrap();

        let migrated: serde_json::Value = serde_json::from_str(
            &tokio::fs::read_to_string(kimi_home.join("mcp.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(migrated["mcpServers"].get("kimi-cu").is_none());
        assert_eq!(
            migrated["mcpServers"]["custom"],
            json!({ "command": "custom-mcp", "args": [] })
        );
        assert_eq!(step_names(&reports.lock().unwrap()), vec!["plugin", "mcp-config"]);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn leaves_the_legacy_mcp_config_untouched_when_it_changes_during_setup() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let app_bin = applications_dir
            .join("KimiCU.app")
            .join("Contents")
            .join("MacOS")
            .join("kimi-cu");
        let kimi_home = root.join("kimi-home");
        tokio::fs::create_dir_all(&kimi_home).await.unwrap();
        let config_path = kimi_home.join("mcp.json");
        let legacy = json!({ "command": app_bin.to_string_lossy(), "args": ["mcp", "-s", "user"] });
        tokio::fs::write(
            &config_path,
            format!(
                "{}\n",
                serde_json::to_string(&json!({ "mcpServers": { "kimi-cu": legacy.clone() } }))
                    .unwrap()
            ),
        )
        .await
        .unwrap();
        let concurrent = json!({
            "mcpServers": {
                "kimi-cu": legacy,
                "addedDuringSetup": { "command": "another-mcp", "args": [] },
            },
        });
        let config_for_hook = config_path.clone();
        let concurrent_for_hook = concurrent.clone();
        let plugins = FakePluginService::with_on_install(
            vec![],
            Some(Arc::new(move || {
                std::fs::write(
                    &config_for_hook,
                    format!(
                        "{}\n",
                        serde_json::to_string_pretty(&concurrent_for_hook).unwrap()
                    ),
                )
                .unwrap();
            })),
        );
        let (host, _) = scripted_host(vec![
            SpawnScript::new("service-status", 0).stdout("SMAppService status=1"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=true"),
        ]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });
        let (report, reports) = recording_reports();

        entry.install(report).await.unwrap();

        let current: serde_json::Value = serde_json::from_str(
            &tokio::fs::read_to_string(&config_path).await.unwrap(),
        )
        .unwrap();
        assert_eq!(current, concurrent);
        assert_eq!(step_names(&reports.lock().unwrap()), vec!["plugin"]);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn does_not_migrate_a_customized_standalone_mcp_registration() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let app_bin = applications_dir
            .join("KimiCU.app")
            .join("Contents")
            .join("MacOS")
            .join("kimi-cu");
        let kimi_home = root.join("kimi-home");
        tokio::fs::create_dir_all(&kimi_home).await.unwrap();
        let config_path = kimi_home.join("mcp.json");
        let custom = json!({
            "mcpServers": {
                "kimi-cu": {
                    "command": app_bin.to_string_lossy(),
                    "args": ["mcp", "-s", "user"],
                    "env": { "CUSTOM": "1" },
                },
            },
        });
        tokio::fs::write(
            &config_path,
            format!("{}\n", serde_json::to_string(&custom).unwrap()),
        )
        .await
        .unwrap();
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![
            SpawnScript::new("service-status", 0).stdout("SMAppService status=1"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=true"),
        ]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });

        entry.install(noop_reporter()).await.unwrap();

        let current: serde_json::Value = serde_json::from_str(
            &tokio::fs::read_to_string(&config_path).await.unwrap(),
        )
        .unwrap();
        assert_eq!(current, custom);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn marks_probe_steps_failed_instead_of_throwing_when_the_binary_is_wedged() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![
            SpawnScript::new("service-status", 0).hang(),
            SpawnScript::new("xpc-ping", 0).hang(),
        ]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            detect_probe_timeout: Some(Duration::from_millis(5)),
            ..make_ctx(&root, plugins.handle(), host)
        });

        let detected = entry.detect().await.unwrap();
        let service_step = detected
            .steps
            .iter()
            .find(|step| step.id == "service")
            .unwrap();
        assert_eq!(service_step.state, CapabilityStepState::Failed);
        assert!(
            service_step
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("timed out")),
            "{service_step:?}"
        );
        let permissions_step = detected
            .steps
            .iter()
            .find(|step| step.id == "permissions")
            .unwrap();
        assert_eq!(permissions_step.state, CapabilityStepState::Failed);
        assert!(
            permissions_step
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("timed out")),
            "{permissions_step:?}"
        );

        // The install path uses the same detect — it must still repair the
        // wiring layer instead of dying on the wedged probes. The service
        // itself legitimately stays broken and reports the clean error.
        let error = entry.install(noop_reporter()).await.unwrap_err();
        assert!(
            error.to_string().contains("not running after install"),
            "{error}"
        );
        assert_eq!(plugins.installs.lock().unwrap().len(), 1);
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn reenables_a_previously_disabled_wiring_plugin_during_setup() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-cu", false, PluginState::Ok).version("0.5.4"),
        ]);
        let (host, _) = scripted_host(vec![
            SpawnScript::new("service-status", 0).stdout("SMAppService status=1"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=true"),
        ]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });

        // Everything else ready, only the disabled wiring blocks readiness —
        // setup must not strand the capability at partial by leaving it off.
        entry.install(noop_reporter()).await.unwrap();
        assert_eq!(
            plugins.enabled_calls.lock().unwrap().as_slice(),
            [("kimi-cu".to_owned(), true)]
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn refreshes_the_wiring_plugin_when_permissions_are_the_only_missing_layer() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-cu", true, PluginState::Ok).version("0.5.4"),
        ]);
        let (host, _) = scripted_host(vec![
            SpawnScript::new("service-status", 0).stdout("SMAppService status=1"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=false"),
        ]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });

        entry.install(noop_reporter()).await.unwrap();

        assert_eq!(
            plugins.installs.lock().unwrap().as_slice(),
            ["https://cdn.kimi.com/kimi-computer-use/latest/kimi-cu-plugin.zip"]
        );
        rm_force(&root).await.unwrap();
    }

    /// Original: the wrapping spawn fake — the fake `ditto` must materialize
    /// the copied binary: moveAppIntoPlace rm's the old bundle first, and the
    /// post-install service check probes the new one.
    struct DittoMaterializingSpawn {
        inner: HostProcessServiceHandle,
        app_bin: PathBuf,
    }

    #[async_trait]
    impl HostProcessService for DittoMaterializingSpawn {
        async fn spawn(
            &self,
            command: &str,
            args: &[String],
            options: HostProcessOptions,
        ) -> Result<Arc<dyn HostProcess>, HostProcessError> {
            let proc = self.inner.spawn(command, args, options).await?;
            if command == "ditto" && args.last().is_some_and(|arg| arg.contains("KimiCU.app")) {
                if let Some(parent) = self.app_bin.parent() {
                    tokio::fs::create_dir_all(parent).await.unwrap();
                }
                tokio::fs::write(&self.app_bin, "#!/bin/sh\n").await.unwrap();
                make_executable(&self.app_bin).await;
            }
            Ok(proc)
        }
    }

    #[tokio::test]
    async fn continues_the_replacement_when_the_old_binary_cleanup_hangs() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let app_bin = applications_dir
            .join("KimiCU.app")
            .join("Contents")
            .join("MacOS")
            .join("kimi-cu");
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-cu", true, PluginState::Ok).version("0.5.4"),
        ]);
        let (inner, calls) = scripted_host(vec![
            // The wedged old binary makes `kimi-cu uninstall` hang — cleanup must
            // swallow the timeout (`|| true` semantics) instead of killing the
            // reinstall before ditto can replace the app.
            SpawnScript::new("uninstall", 0).hang(),
            SpawnScript::new("service-status", 0).stdout("SMAppService status=1"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=true"),
        ]);
        let host = HostProcessServiceHandle(Arc::new(DittoMaterializingSpawn {
            inner,
            app_bin,
        }));
        let fetch: Arc<dyn FetchLike> = Arc::new(BytesFetch {
            bytes: vec![1, 2, 3],
        });
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            fetch_impl: Some(fetch),
            command_timeout: Some(Duration::from_millis(5)),
            ..make_ctx(&root, plugins.handle(), host)
        });

        // Fully ready → explicit reinstall exercises the cleanup path.
        entry.install(noop_reporter()).await.unwrap();
        let calls = calls.lock().unwrap().clone();
        assert!(calls.iter().any(|call| call.contains("ditto")), "{calls:?}");
        assert!(
            !calls
                .iter()
                .any(|call| call.contains("pkill") && call.contains("+mcp")),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|call| call.contains("pkill") && call.contains("+service")),
            "{calls:?}"
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn reports_the_plugin_layer_missing_when_its_mcp_server_is_disabled() {
        let root = temp_root("kimi-cu-entry").await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-cu", true, PluginState::Ok)
                .version("0.5.4")
                .enabled_mcp(0),
        ]);
        let (host, _) = scripted_host(vec![]);
        let entry = create_kimi_cu_entry(make_ctx(&root, plugins.handle(), host));

        // The plugin toggle is on but the stdio MCP wrapper is off: readiness
        // must not claim ready — new sessions would get no Computer Use tools.
        let detected = entry.detect().await.unwrap();
        assert_eq!(
            detected.steps.iter().find(|step| step.id == "plugin"),
            Some(&CapabilityStep {
                id: "plugin".to_owned(),
                state: CapabilityStepState::Missing,
                detail: Some("mcp 0/1 enabled".to_owned()),
                optional: None,
            })
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn reenables_disabled_mcp_servers_during_setup() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        let plugins = FakePluginService::new(vec![
            FakePlugin::new("kimi-cu", true, PluginState::Ok)
                .version("0.5.4")
                .enabled_mcp(0),
        ]);
        let (host, _) = scripted_host(vec![
            SpawnScript::new("service-status", 0).stdout("SMAppService status=1"),
            SpawnScript::new("xpc-ping", 0)
                .stdout("permissionStatus: accessibility=true screenRecording=true"),
        ]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });

        // Upsert preserves the per-server disabled state, so setup repairs it
        // explicitly — the plugin toggle alone is not enough.
        entry.install(noop_reporter()).await.unwrap();
        assert_eq!(
            plugins.mcp_enabled_calls.lock().unwrap().as_slice(),
            [("kimi-cu".to_owned(), "mac".to_owned(), true)]
        );
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn never_stops_the_old_service_when_the_downloaded_archive_is_corrupt() {
        let root = temp_root("kimi-cu-entry").await;
        let plugins = FakePluginService::new(vec![]);
        let (host, calls) = scripted_host(vec![
            SpawnScript::new("ditto -x -k", 1).stderr("ditto: Not a zip file"),
        ]);
        let fetch: Arc<dyn FetchLike> = Arc::new(BytesFetch {
            bytes: b"<html>captive portal</html>".to_vec(),
        });
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(root.join("Applications")),
            fetch_impl: Some(fetch),
            ..make_ctx(&root, plugins.handle(), host)
        });

        // A corrupt archive must fail before any teardown — a failed update
        // never breaks a previously working setup.
        let error = entry.install(noop_reporter()).await.unwrap_err();
        assert!(error.to_string().contains("Failed to unzip"), "{error}");
        let calls = calls.lock().unwrap().clone();
        assert!(!calls.iter().any(|call| call.contains("uninstall")));
        assert!(!calls.iter().any(|call| call.contains("bootout")));
        assert!(!calls.iter().any(|call| call.contains("pkill")));
        rm_force(&root).await.unwrap();
    }

    #[tokio::test]
    async fn reads_a_bundle_missing_its_info_plist_as_a_broken_install() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        // Executable binary but the bundle metadata is gone (partial copy).
        tokio::fs::remove_file(
            applications_dir
                .join("KimiCU.app")
                .join("Contents")
                .join("Info.plist"),
        )
        .await
        .unwrap();
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });

        let detected = entry.detect().await.unwrap();
        assert_eq!(
            detected
                .steps
                .iter()
                .find(|step| step.id == "app")
                .map(|step| step.state),
            Some(CapabilityStepState::Missing)
        );
        rm_force(&root).await.unwrap();
    }

    // The executable bit is a POSIX concept; the TS suite only exercises it
    // meaningfully there as well.
    #[cfg(unix)]
    #[tokio::test]
    async fn reads_a_non_executable_leftover_app_binary_as_a_broken_install() {
        let root = temp_root("kimi-cu-entry").await;
        let applications_dir = fake_app_bundle(&root).await;
        // An interrupted ditto leaves the binary present but not executable.
        make_non_executable(
            &applications_dir
                .join("KimiCU.app")
                .join("Contents")
                .join("MacOS")
                .join("kimi-cu"),
        )
        .await;
        let plugins = FakePluginService::new(vec![]);
        let (host, _) = scripted_host(vec![]);
        let entry = create_kimi_cu_entry(CapabilityEntryContext {
            applications_dir: Some(applications_dir),
            ..make_ctx(&root, plugins.handle(), host)
        });

        let detected = entry.detect().await.unwrap();
        assert_eq!(
            detected.steps.iter().find(|step| step.id == "app"),
            Some(&CapabilityStep {
                id: "app".to_owned(),
                state: CapabilityStepState::Missing,
                detail: Some("not executable".to_owned()),
                optional: None,
            })
        );
        rm_force(&root).await.unwrap();
    }
}
