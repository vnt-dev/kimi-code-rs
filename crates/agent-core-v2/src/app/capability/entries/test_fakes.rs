//! Test doubles shared by the capability entry tests: scripted host
//! processes, a fake plugin service, and scripted fetch implementations.
//! Mirrors the fakes in the original TS test files (scripted spawn results,
//! upsert-plugin semantics, per-URL fetch routing).

use parking_lot::Mutex;
use std::sync::Arc;
use std::{collections::HashMap, error::Error};

use async_trait::async_trait;
use futures_util::stream;
use tokio::{io::AsyncRead, sync::Mutex as AsyncMutex};

use crate::{
    _base::{
        di::lifecycle::{Disposable, DisposeResult},
        errors::errors::ExpectedError,
        event::Event,
    },
    agent::{external_hooks::HookDef, mcp::McpServerConfig},
    app::{
        capability::host::{FetchBodyStream, FetchLike, FetchResponse},
        plugin::{
            EnabledPluginSessionStart, GetPluginInfoInput, InstallPluginInput, PluginCommandDef,
            PluginInfo, PluginInstallOperation, PluginMcpServerInfo, PluginMcpTransport,
            PluginServiceContract, PluginServiceHandle, PluginServiceResult, PluginSource,
            PluginState, PluginSummary, PluginUpdateStatus, ReloadSummary, RemovePluginInput,
            SetPluginEnabledInput, SetPluginMcpServerEnabledInput,
        },
        skill_catalog::SkillRoot,
    },
    os::interface::host_process::{
        HostProcess, HostProcessError, HostProcessOptions, HostProcessService,
        HostProcessServiceHandle, ProcessSignal, SharedProcessReader, SharedProcessWriter,
    },
};

// ---------------------------------------------------------------------------
// Scripted host processes.
// ---------------------------------------------------------------------------

pub struct SpawnScript {
    pub contains: &'static str,
    pub code: i32,
    pub stdout: &'static str,
    pub stderr: &'static str,
    pub hang: bool,
}

impl SpawnScript {
    pub fn new(contains: &'static str, code: i32) -> Self {
        Self {
            contains,
            code,
            stdout: "",
            stderr: "",
            hang: false,
        }
    }

    pub fn stdout(mut self, stdout: &'static str) -> Self {
        self.stdout = stdout;
        self
    }

    pub fn stderr(mut self, stderr: &'static str) -> Self {
        self.stderr = stderr;
        self
    }

    /// Original: the fake whose `wait()` never settles — the caller's own
    /// timeout must fire.
    pub fn hang(mut self) -> Self {
        self.hang = true;
        self
    }
}

pub fn data_reader(bytes: &[u8]) -> SharedProcessReader {
    Arc::new(AsyncMutex::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())) as Box<dyn AsyncRead + Send + Unpin>,
    ))
}

pub struct FakeHostProcess {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub hang: bool,
}

#[async_trait]
impl HostProcess for FakeHostProcess {
    fn pid(&self) -> i64 {
        1234
    }

    fn exit_code(&self) -> Option<i32> {
        Some(self.code)
    }

    fn stdin(&self) -> SharedProcessWriter {
        Arc::new(AsyncMutex::new(
            Box::new(tokio::io::sink()) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>
        ))
    }

    fn stdout(&self) -> SharedProcessReader {
        data_reader(self.stdout.as_bytes())
    }

    fn stderr(&self) -> SharedProcessReader {
        data_reader(self.stderr.as_bytes())
    }

    async fn wait(&self) -> Result<i32, HostProcessError> {
        if self.hang {
            std::future::pending::<()>().await;
        }
        Ok(self.code)
    }

    async fn kill(&self, _: Option<ProcessSignal>) -> Result<(), HostProcessError> {
        Ok(())
    }

    fn dispose(&self) {}
}

/// Original: `fakeHostProcess(script)` — the first script whose `contains`
/// matches the joined spawn key wins; unmatched spawns exit 0 silently.
pub struct ScriptedHostProcessService {
    pub scripts: Vec<SpawnScript>,
    pub calls: Arc<Mutex<Vec<String>>>,
}

impl ScriptedHostProcessService {
    pub fn new(scripts: Vec<SpawnScript>) -> Self {
        Self {
            scripts,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

/// Builds the handle directly, moving the scripts.
pub fn scripted_host(
    scripts: Vec<SpawnScript>,
) -> (HostProcessServiceHandle, Arc<Mutex<Vec<String>>>) {
    let service = ScriptedHostProcessService::new(scripts);
    let calls = Arc::clone(&service.calls);
    (HostProcessServiceHandle(Arc::new(service)), calls)
}

#[async_trait]
impl HostProcessService for ScriptedHostProcessService {
    async fn spawn(
        &self,
        command: &str,
        args: &[String],
        _: HostProcessOptions,
    ) -> Result<Arc<dyn HostProcess>, HostProcessError> {
        let key = format!("{} {}", command, args.join(" "));
        self.calls.lock().push(key.clone());
        let hit = self
            .scripts
            .iter()
            .find(|script| key.contains(script.contains));
        let (code, stdout, stderr, hang) = match hit {
            Some(script) => (script.code, script.stdout, script.stderr, script.hang),
            None => (0, "", "", false),
        };
        Ok(Arc::new(FakeHostProcess {
            code,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            hang,
        }))
    }
}

// ---------------------------------------------------------------------------
// Fake plugin service.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct FakePlugin {
    pub id: String,
    pub enabled: bool,
    pub state: PluginState,
    pub version: Option<String>,
    pub enabled_mcp: usize,
}

impl FakePlugin {
    pub fn new(id: &str, enabled: bool, state: PluginState) -> Self {
        Self {
            id: id.to_owned(),
            enabled,
            state,
            version: None,
            enabled_mcp: 1,
        }
    }

    pub fn version(mut self, version: &str) -> Self {
        self.version = Some(version.to_owned());
        self
    }

    pub fn enabled_mcp(mut self, enabled_mcp: usize) -> Self {
        self.enabled_mcp = enabled_mcp;
        self
    }
}

fn summary_of(plugin: &FakePlugin) -> PluginSummary {
    PluginSummary {
        id: plugin.id.clone(),
        display_name: plugin.id.clone(),
        version: plugin.version.clone(),
        enabled: plugin.enabled,
        state: plugin.state,
        skill_count: 1,
        mcp_server_count: 1,
        enabled_mcp_server_count: plugin.enabled_mcp,
        hook_count: 0,
        command_count: 0,
        has_errors: false,
        source: PluginSource::ZipUrl,
        original_source: None,
        github: None,
    }
}

/// Original: `fakePlugins(installed, onInstall)` — upsert semantics of the
/// real manager: a new id installs enabled, an existing record keeps its
/// (possibly disabled) enabled flag.
pub struct FakePluginService {
    pub installed: Arc<Mutex<Vec<FakePlugin>>>,
    pub installs: Arc<Mutex<Vec<String>>>,
    pub enabled_calls: Arc<Mutex<Vec<(String, bool)>>>,
    pub mcp_enabled_calls: Arc<Mutex<Vec<(String, String, bool)>>>,
    on_install: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl FakePluginService {
    pub fn new(installed: Vec<FakePlugin>) -> Arc<Self> {
        Self::with_on_install(installed, None)
    }

    pub fn with_on_install(
        installed: Vec<FakePlugin>,
        on_install: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            installed: Arc::new(Mutex::new(installed)),
            installs: Arc::new(Mutex::new(Vec::new())),
            enabled_calls: Arc::new(Mutex::new(Vec::new())),
            mcp_enabled_calls: Arc::new(Mutex::new(Vec::new())),
            on_install,
        })
    }

    pub fn handle(self: &Arc<Self>) -> PluginServiceHandle {
        PluginServiceHandle(Arc::clone(self) as Arc<dyn PluginServiceContract>)
    }
}

#[async_trait]
impl PluginServiceContract for FakePluginService {
    async fn list_plugins(&self) -> PluginServiceResult<Vec<PluginSummary>> {
        Ok(self.installed.lock().iter().map(summary_of).collect())
    }

    async fn install_plugin(
        &self,
        input: InstallPluginInput,
    ) -> PluginServiceResult<PluginSummary> {
        self.installs.lock().push(input.source.clone());
        if let Some(on_install) = &self.on_install {
            on_install();
        }
        let id = if input.source.contains("computer-use-windows") {
            "kimi-cu-win"
        } else if input.source.contains("computer-use") {
            "kimi-cu"
        } else {
            "kimi-webbridge"
        };
        let mut installed = self.installed.lock();
        match installed.iter_mut().find(|plugin| plugin.id == id) {
            None => {
                let plugin = FakePlugin {
                    id: id.to_owned(),
                    enabled: true,
                    state: PluginState::Ok,
                    version: (id == "kimi-webbridge").then(|| "1.11.3".to_owned()),
                    enabled_mcp: 1,
                };
                installed.push(plugin.clone());
                Ok(summary_of(&plugin))
            }
            Some(existing) => {
                existing.state = PluginState::Ok;
                if id == "kimi-webbridge" {
                    existing.version = Some("1.11.3".to_owned());
                }
                Ok(summary_of(existing))
            }
        }
    }

    async fn set_plugin_enabled(&self, input: SetPluginEnabledInput) -> PluginServiceResult<()> {
        self.enabled_calls
            .lock()
            .push((input.id.clone(), input.enabled));
        if let Some(existing) = self
            .installed
            .lock()
            .iter_mut()
            .find(|plugin| plugin.id == input.id)
        {
            existing.enabled = input.enabled;
        }
        Ok(())
    }

    async fn set_plugin_mcp_server_enabled(
        &self,
        input: SetPluginMcpServerEnabledInput,
    ) -> PluginServiceResult<()> {
        self.mcp_enabled_calls
            .lock()
            .push((input.id.clone(), input.server.clone(), input.enabled));
        if let Some(existing) = self
            .installed
            .lock()
            .iter_mut()
            .find(|plugin| plugin.id == input.id)
        {
            existing.enabled_mcp = usize::from(input.enabled);
        }
        Ok(())
    }

    async fn get_plugin_info(&self, input: GetPluginInfoInput) -> PluginServiceResult<PluginInfo> {
        let installed = self.installed.lock();
        let existing = installed
            .iter()
            .find(|plugin| plugin.id == input.id)
            .cloned();
        let name = if input.id == "kimi-cu-win" {
            "win"
        } else {
            "mac"
        };
        Ok(PluginInfo {
            summary: existing
                .as_ref()
                .map(summary_of)
                .unwrap_or_else(|| summary_of(&FakePlugin::new(&input.id, true, PluginState::Ok))),
            root: String::new(),
            installed_at: String::new(),
            updated_at: None,
            manifest_kind: None,
            manifest_path: None,
            manifest: None,
            mcp_servers: vec![PluginMcpServerInfo {
                name: name.to_owned(),
                runtime_name: name.to_owned(),
                enabled: existing.map(|plugin| plugin.enabled_mcp).unwrap_or(1) == 1,
                transport: PluginMcpTransport::Stdio,
                command: None,
                args: None,
                cwd: None,
                url: None,
                env_keys: None,
                header_keys: None,
            }],
            shadowed_manifest_path: None,
            diagnostics: Vec::new(),
        })
    }

    async fn remove_plugin(&self, _: RemovePluginInput) -> PluginServiceResult<()> {
        unimplemented!()
    }

    async fn install_plugin_in_background(
        self: Arc<Self>,
        _: InstallPluginInput,
        _: String,
    ) -> PluginServiceResult<()> {
        unimplemented!()
    }

    fn plugin_install_progress(&self, _: &str) -> Option<PluginInstallOperation> {
        None
    }

    async fn reload_plugins(&self) -> PluginServiceResult<ReloadSummary> {
        Ok(ReloadSummary::default())
    }

    async fn list_plugin_commands(&self) -> PluginServiceResult<Vec<PluginCommandDef>> {
        Ok(Vec::new())
    }

    async fn check_updates(&self) -> PluginServiceResult<Vec<PluginUpdateStatus>> {
        Ok(Vec::new())
    }

    async fn plugin_skill_roots(&self) -> PluginServiceResult<Vec<SkillRoot>> {
        Ok(Vec::new())
    }

    async fn enabled_session_starts(&self) -> PluginServiceResult<Vec<EnabledPluginSessionStart>> {
        Ok(Vec::new())
    }

    async fn enabled_mcp_servers(&self) -> PluginServiceResult<HashMap<String, McpServerConfig>> {
        Ok(HashMap::new())
    }

    async fn enabled_hooks(&self) -> PluginServiceResult<Vec<HookDef>> {
        Ok(Vec::new())
    }

    fn on_did_reload(&self) -> Event<ReloadSummary> {
        Event::none()
    }
}

impl Disposable for FakePluginService {
    fn dispose(&self) -> DisposeResult {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Scripted fetch implementations.
// ---------------------------------------------------------------------------

pub fn body_stream(bytes: Vec<u8>) -> FetchBodyStream {
    Box::pin(stream::iter(vec![Ok::<
        Vec<u8>,
        Box<dyn Error + Send + Sync>,
    >(bytes)]))
}

/// Original: the test fetch answering with a single 200 body chunk.
pub struct BytesFetch {
    pub bytes: Vec<u8>,
}

#[async_trait]
impl FetchLike for BytesFetch {
    async fn fetch(&self, _: &str) -> Result<FetchResponse, Box<dyn Error + Send + Sync>> {
        Ok(FetchResponse {
            ok: true,
            status: 200,
            content_length: Some(self.bytes.len() as u64),
            body: Some(body_stream(self.bytes.clone())),
        })
    }
}

/// Original: `(() => Promise.reject(new Error(...))) as never` — a fetch
/// that must never be called on paths expected to skip the download.
pub struct FailingFetch {
    pub message: &'static str,
}

#[async_trait]
impl FetchLike for FailingFetch {
    async fn fetch(&self, _: &str) -> Result<FetchResponse, Box<dyn Error + Send + Sync>> {
        Err(Box::new(ExpectedError::new(self.message)))
    }
}

/// Original: `fakeFetch({ statusSequence, binary })` — answers the daemon
/// `/status` endpoint from a clamped sequence and CDN binary downloads.
pub struct WebbridgeFetch {
    pub status_sequence: Vec<Option<serde_json::Value>>,
    pub binary: Vec<u8>,
    pub status_calls: Mutex<usize>,
}

impl WebbridgeFetch {
    pub fn new(status_sequence: Vec<Option<serde_json::Value>>) -> Self {
        Self {
            status_sequence,
            binary: vec![1, 2, 3, 4],
            status_calls: Mutex::new(0),
        }
    }

    pub fn binary(mut self, binary: &[u8]) -> Self {
        self.binary = binary.to_vec();
        self
    }
}

#[async_trait]
impl FetchLike for WebbridgeFetch {
    async fn fetch(&self, url: &str) -> Result<FetchResponse, Box<dyn Error + Send + Sync>> {
        if url.ends_with("/status") {
            let mut calls = self.status_calls.lock();
            let step = self
                .status_sequence
                .get(*calls)
                .or_else(|| self.status_sequence.last())
                .cloned()
                .flatten();
            *calls += 1;
            let Some(step) = step else {
                return Err(Box::new(ExpectedError::new("connection refused")));
            };
            let bytes = serde_json::to_vec(&step).unwrap();
            return Ok(FetchResponse {
                ok: true,
                status: 200,
                content_length: Some(bytes.len() as u64),
                body: Some(body_stream(bytes)),
            });
        }
        if url.contains("cdn.kimi.com/webbridge/") {
            return Ok(FetchResponse {
                ok: true,
                status: 200,
                content_length: Some(self.binary.len() as u64),
                body: Some(body_stream(self.binary.clone())),
            });
        }
        Err(Box::new(ExpectedError::new(format!(
            "unexpected fetch: {url}"
        ))))
    }
}
