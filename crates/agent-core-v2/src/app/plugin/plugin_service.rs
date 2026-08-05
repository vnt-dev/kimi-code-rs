//! App-wide filesystem-backed plugin service.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/pluginService.ts`.

use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use kimi_code_oauth::KIMI_CODE_PROVIDER_NAME;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options, ErrorCause},
        event::{Emitter, Event},
    },
    agent::{external_hooks::HookDef, mcp::McpServerConfig},
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        skill_catalog::{SKILL_DISCOVERY_SERVICE_ID, SkillDiscoveryHandle, SkillRoot},
    },
    kosong::provider::{PROVIDER_SERVICE_ID, ProviderServiceHandle},
};

use super::{
    contract::{
        GetPluginInfoInput, InstallPluginInput, PLUGIN_SERVICE_ID, PluginServiceContract,
        PluginServiceHandle, PluginServiceResult, RemovePluginInput, SetPluginEnabledInput,
        SetPluginMcpServerEnabledInput,
    },
    errors::{PLUGIN_LOAD_FAILED, PLUGIN_NOT_FOUND, ensure_plugin_errors_registered},
    manager::{PluginManager, PluginManagerOptions},
    types::{
        EnabledPluginSessionStart, PluginCommandDef, PluginInfo, PluginInstallProgressCallback,
        PluginSummary, PluginUpdateStatus, ReloadSummary,
    },
};

const KIMI_CODE_BASE_URL_ENV: &str = "KIMI_CODE_BASE_URL";
const KIMI_CODE_OAUTH_HOST_ENV: &str = "KIMI_CODE_OAUTH_HOST";
const KIMI_OAUTH_HOST_ENV: &str = "KIMI_OAUTH_HOST";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitialLoadState {
    NotStarted,
    Loading,
    Complete,
}

#[derive(Default)]
struct ServiceStatus {
    has_loaded_snapshot: bool,
    load_error: Option<Arc<dyn Error + Send + Sync>>,
}

struct InitialLoadGuard<'a> {
    state: &'a Mutex<InitialLoadState>,
    notify: &'a Notify,
    finished: bool,
}

impl InitialLoadGuard<'_> {
    fn finish(mut self) {
        *self.state.lock().unwrap() = InitialLoadState::Complete;
        self.finished = true;
        self.notify.notify_waiters();
    }
}

impl Drop for InitialLoadGuard<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        *self.state.lock().unwrap() = InitialLoadState::NotStarted;
        self.notify.notify_waiters();
    }
}

pub struct PluginService {
    home_dir: String,
    env_base_url: Option<String>,
    env_oauth_host: Option<String>,
    manager: AsyncMutex<PluginManager>,
    providers: ProviderServiceHandle,
    initial_load: Mutex<InitialLoadState>,
    initial_load_notify: Notify,
    status: Mutex<ServiceStatus>,
    on_did_reload: Arc<Emitter<ReloadSummary>>,
    disposables: DisposableStore,
}

impl PluginService {
    // Original: PluginService.constructor().
    pub fn new(
        bootstrap: BootstrapServiceHandle,
        discovery: SkillDiscoveryHandle,
        providers: ProviderServiceHandle,
    ) -> Self {
        let emitter = Arc::new(Emitter::new());
        let disposables = DisposableStore::new();
        disposables.add(Arc::clone(&emitter) as Arc<dyn Disposable>);
        let home_dir = bootstrap.home_dir().to_string_lossy().into_owned();
        let env_base_url = bootstrap.get_env(KIMI_CODE_BASE_URL_ENV).map(str::to_owned);
        let env_oauth_host = bootstrap
            .get_env(KIMI_CODE_OAUTH_HOST_ENV)
            .or_else(|| bootstrap.get_env(KIMI_OAUTH_HOST_ENV))
            .map(str::to_owned);
        let manager = PluginManager::new(PluginManagerOptions {
            kimi_home_dir: home_dir.clone(),
            discover_skills: Some(Arc::clone(&discovery.0)),
        });
        Self {
            home_dir,
            env_base_url,
            env_oauth_host,
            manager: AsyncMutex::new(manager),
            providers,
            initial_load: Mutex::new(InitialLoadState::NotStarted),
            initial_load_notify: Notify::new(),
            status: Mutex::new(ServiceStatus::default()),
            on_did_reload: emitter,
            disposables,
        }
    }

    // Original: startInitialLoad() + loadOnce().
    async fn start_initial_load(&self) {
        loop {
            let notified = self.initial_load_notify.notified();
            if let Some(guard) = self.try_claim_initial_load() {
                let result = self.manager.lock().await.load().await;
                let mut status = self.status.lock().unwrap();
                match result {
                    Ok(()) => {
                        status.has_loaded_snapshot = true;
                        status.load_error = None;
                    }
                    Err(error) => status.load_error = Some(Arc::from(error)),
                }
                drop(status);
                guard.finish();
                return;
            }
            if *self.initial_load.lock().unwrap() == InitialLoadState::Complete {
                return;
            }
            notified.await;
        }
    }

    fn try_claim_initial_load(&self) -> Option<InitialLoadGuard<'_>> {
        let mut state = self.initial_load.lock().unwrap();
        if *state == InitialLoadState::NotStarted {
            *state = InitialLoadState::Loading;
            Some(InitialLoadGuard {
                state: &self.initial_load,
                notify: &self.initial_load_notify,
                finished: false,
            })
        } else {
            None
        }
    }

    fn assert_loaded(&self) -> PluginServiceResult<()> {
        let status = self.status.lock().unwrap();
        let Some(error) = &status.load_error else {
            return Ok(());
        };
        Err(Box::new(Error2::with_options(
            PLUGIN_LOAD_FAILED,
            format!(
                "Plugin state failed to load: {error}. Fix the file at {}/plugins/installed.json and run /plugins reload.",
                self.home_dir
            ),
            Error2Options {
                cause: Some(ErrorCause::Error(Arc::clone(error))),
                details: Some(home_details(&self.home_dir)),
                ..Error2Options::default()
            },
        )))
    }

    fn has_loaded_snapshot(&self) -> bool {
        self.status.lock().unwrap().has_loaded_snapshot
    }

    // Original: managedKimiCodeEnvForPlugins().
    async fn managed_kimi_code_env_for_plugins(
        &self,
    ) -> PluginServiceResult<HashMap<String, String>> {
        self.providers.ready().await?;
        let provider = self.providers.get(KIMI_CODE_PROVIDER_NAME);
        let has_env_override = self.env_base_url.is_some() || self.env_oauth_host.is_some();
        let base_url = self
            .env_base_url
            .as_ref()
            .map(|value| value.trim_end_matches('/').to_owned())
            .or_else(|| {
                provider
                    .as_ref()
                    .and_then(|provider| provider.base_url.clone())
            });
        let oauth_host = if has_env_override {
            self.env_oauth_host.clone()
        } else {
            provider
                .as_ref()
                .and_then(|provider| provider.oauth.as_ref())
                .and_then(|oauth| oauth.oauth_host.clone())
        };
        let mut env = HashMap::new();
        if let Some(base_url) = base_url {
            env.insert(KIMI_CODE_BASE_URL_ENV.to_owned(), base_url);
        }
        if let Some(oauth_host) = oauth_host {
            env.insert(KIMI_CODE_OAUTH_HOST_ENV.to_owned(), oauth_host);
        }
        Ok(env)
    }
}

#[async_trait]
impl PluginServiceContract for PluginService {
    async fn list_plugins(&self) -> PluginServiceResult<Vec<PluginSummary>> {
        self.start_initial_load().await;
        let manager = self.manager.lock().await;
        self.assert_loaded()?;
        Ok(manager.summaries())
    }

    async fn install_plugin(
        &self,
        input: InstallPluginInput,
    ) -> PluginServiceResult<PluginSummary> {
        self.start_initial_load().await;
        let mut manager = self.manager.lock().await;
        self.assert_loaded()?;
        let record = manager.install(&input.source).await?;
        manager
            .info(&record.id)
            .map(|info| info.summary)
            .ok_or_else(|| {
                message_error(format!(
                    "Plugin \"{}\" missing right after install",
                    record.id
                ))
            })
    }

    async fn install_plugin_with_progress(
        &self,
        input: InstallPluginInput,
        progress: PluginInstallProgressCallback,
    ) -> PluginServiceResult<PluginSummary> {
        self.start_initial_load().await;
        let mut manager = self.manager.lock().await;
        self.assert_loaded()?;
        let record = manager
            .install_with_progress(&input.source, Some(progress))
            .await?;
        manager
            .info(&record.id)
            .map(|info| info.summary)
            .ok_or_else(|| {
                message_error(format!(
                    "Plugin \"{}\" missing right after install",
                    record.id
                ))
            })
    }

    async fn set_plugin_enabled(&self, input: SetPluginEnabledInput) -> PluginServiceResult<()> {
        self.start_initial_load().await;
        let mut manager = self.manager.lock().await;
        self.assert_loaded()?;
        manager.set_enabled(&input.id, input.enabled).await
    }

    async fn set_plugin_mcp_server_enabled(
        &self,
        input: SetPluginMcpServerEnabledInput,
    ) -> PluginServiceResult<()> {
        self.start_initial_load().await;
        let mut manager = self.manager.lock().await;
        self.assert_loaded()?;
        manager
            .set_mcp_server_enabled(&input.id, &input.server, input.enabled)
            .await
    }

    async fn remove_plugin(&self, input: RemovePluginInput) -> PluginServiceResult<()> {
        self.start_initial_load().await;
        let mut manager = self.manager.lock().await;
        self.assert_loaded()?;
        manager.remove(&input.id).await
    }

    async fn reload_plugins(&self) -> PluginServiceResult<ReloadSummary> {
        let initial_guard = self.try_claim_initial_load();
        if initial_guard.is_none() {
            self.start_initial_load().await;
        }
        let result = self.manager.lock().await.reload().await;
        match result {
            Ok(summary) => {
                let mut status = self.status.lock().unwrap();
                status.has_loaded_snapshot = true;
                status.load_error = None;
                drop(status);
                if let Some(guard) = initial_guard {
                    guard.finish();
                }
                self.on_did_reload.fire(&summary);
                Ok(summary)
            }
            Err(error) => {
                let error: Arc<dyn Error + Send + Sync> = Arc::from(error);
                self.status.lock().unwrap().load_error = Some(Arc::clone(&error));
                if let Some(guard) = initial_guard {
                    guard.finish();
                }
                Err(Box::new(Error2::with_options(
                    PLUGIN_LOAD_FAILED,
                    format!("Failed to reload plugins: {error}"),
                    Error2Options {
                        cause: Some(ErrorCause::Error(error)),
                        details: Some(home_details(&self.home_dir)),
                        ..Error2Options::default()
                    },
                )))
            }
        }
    }

    async fn get_plugin_info(&self, input: GetPluginInfoInput) -> PluginServiceResult<PluginInfo> {
        self.start_initial_load().await;
        let manager = self.manager.lock().await;
        self.assert_loaded()?;
        manager.info(&input.id).ok_or_else(|| {
            Box::new(Error2::with_options(
                PLUGIN_NOT_FOUND,
                format!("Plugin \"{}\" is not installed", input.id),
                Error2Options {
                    details: Some(Map::from_iter([("id".to_owned(), Value::String(input.id))])),
                    ..Error2Options::default()
                },
            )) as Box<dyn Error + Send + Sync>
        })
    }

    async fn list_plugin_commands(&self) -> PluginServiceResult<Vec<PluginCommandDef>> {
        self.start_initial_load().await;
        let manager = self.manager.lock().await;
        self.assert_loaded()?;
        Ok(manager.enabled_commands().await)
    }

    async fn check_updates(&self) -> PluginServiceResult<Vec<PluginUpdateStatus>> {
        self.start_initial_load().await;
        let manager = self.manager.lock().await;
        self.assert_loaded()?;
        Ok(manager.check_updates().await)
    }

    async fn plugin_skill_roots(&self) -> PluginServiceResult<Vec<SkillRoot>> {
        self.start_initial_load().await;
        let manager = self.manager.lock().await;
        if !self.has_loaded_snapshot() {
            return Ok(Vec::new());
        }
        Ok(manager.plugin_skill_roots())
    }

    async fn enabled_session_starts(&self) -> PluginServiceResult<Vec<EnabledPluginSessionStart>> {
        self.start_initial_load().await;
        let manager = self.manager.lock().await;
        if !self.has_loaded_snapshot() {
            return Ok(Vec::new());
        }
        Ok(manager.enabled_session_starts())
    }

    async fn enabled_mcp_servers(&self) -> PluginServiceResult<HashMap<String, McpServerConfig>> {
        self.start_initial_load().await;
        let plugin_servers = {
            let manager = self.manager.lock().await;
            if !self.has_loaded_snapshot() {
                return Ok(HashMap::new());
            }
            manager.enabled_mcp_servers()
        };
        if !plugin_servers
            .values()
            .any(|server| matches!(server, McpServerConfig::Stdio(_)))
        {
            return Ok(plugin_servers);
        }
        let managed_env = self.managed_kimi_code_env_for_plugins().await?;
        Ok(with_managed_kimi_plugin_env(plugin_servers, &managed_env))
    }

    async fn enabled_hooks(&self) -> PluginServiceResult<Vec<HookDef>> {
        self.start_initial_load().await;
        let manager = self.manager.lock().await;
        if !self.has_loaded_snapshot() {
            return Ok(Vec::new());
        }
        Ok(manager.enabled_hooks())
    }

    fn on_did_reload(&self) -> Event<ReloadSummary> {
        self.on_did_reload.event()
    }
}

impl Disposable for PluginService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

// Original: withManagedKimiPluginEnv().
fn with_managed_kimi_plugin_env(
    mut plugin_servers: HashMap<String, McpServerConfig>,
    managed_env: &HashMap<String, String>,
) -> HashMap<String, McpServerConfig> {
    if managed_env.is_empty() {
        return plugin_servers;
    }
    for server in plugin_servers.values_mut() {
        let McpServerConfig::Stdio(config) = server else {
            continue;
        };
        let env = config.env.get_or_insert_with(HashMap::new);
        env.extend(managed_env.clone());
    }
    plugin_servers
}

fn home_details(home_dir: &str) -> Map<String, Value> {
    Map::from_iter([("kimiHomeDir".to_owned(), Value::String(home_dir.to_owned()))])
}

fn message_error(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(ServiceMessageError(message.into()))
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct ServiceMessageError(String);

pub fn register_plugin_service() {
    ensure_plugin_errors_registered();
    register_scoped_service(
        LifecycleScope::App,
        PLUGIN_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let discovery = accessor.get(SKILL_DISCOVERY_SERVICE_ID)?;
            let providers = accessor.get(PROVIDER_SERVICE_ID)?;
            let service: Arc<dyn PluginServiceContract> = Arc::new(PluginService::new(
                (*bootstrap).clone(),
                (*discovery).clone(),
                (*providers).clone(),
            ));
            Ok(PluginServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "plugin",
    );
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use indexmap::IndexMap;

    use crate::{
        _base::di::lifecycle::DisposeResult,
        app::{
            bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
            skill_catalog::{SkillDiscoveryContract, SkillDiscoveryResult},
        },
        kosong::provider::{
            ProviderConfig, ProviderServiceContract, ProviderServiceResult, ProvidersChangedEvent,
            ProvidersSection,
        },
    };

    use super::*;

    struct EmptyDiscovery;

    #[async_trait]
    impl SkillDiscoveryContract for EmptyDiscovery {
        async fn discover(&self, roots: &[SkillRoot]) -> SkillDiscoveryResult {
            SkillDiscoveryResult {
                scanned_roots: roots.iter().map(|root| root.path.clone()).collect(),
                ..SkillDiscoveryResult::default()
            }
        }
    }

    struct StubProviders {
        provider: Option<ProviderConfig>,
    }

    #[async_trait]
    impl ProviderServiceContract for StubProviders {
        async fn ready(&self) -> ProviderServiceResult<()> {
            Ok(())
        }
        fn on_did_change_providers(&self) -> Event<ProvidersChangedEvent> {
            Event::none()
        }
        fn get(&self, _name: &str) -> Option<ProviderConfig> {
            self.provider.clone()
        }
        fn list(&self) -> ProvidersSection {
            IndexMap::new()
        }
        async fn set(&self, _name: &str, _config: ProviderConfig) -> ProviderServiceResult<()> {
            Ok(())
        }
        async fn delete(&self, _name: &str) -> ProviderServiceResult<()> {
            Ok(())
        }
    }

    impl Disposable for StubProviders {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    fn service(home: &Path, env: HashMap<String, String>) -> PluginService {
        let bootstrap: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                home_dir: home.to_owned(),
                config_path: home.join("config.toml"),
                os_home_dir: home.to_owned(),
                platform: "linux".to_owned(),
                arch: "x64".to_owned(),
                cwd: home.to_owned(),
                env,
                client_version: "test".to_owned(),
            }));
        let discovery: Arc<dyn SkillDiscoveryContract> = Arc::new(EmptyDiscovery);
        let providers: Arc<dyn ProviderServiceContract> = Arc::new(StubProviders {
            provider: Some(ProviderConfig {
                base_url: Some("https://provider.test/".to_owned()),
                ..ProviderConfig::default()
            }),
        });
        PluginService::new(
            BootstrapServiceHandle(bootstrap),
            SkillDiscoveryHandle(discovery),
            ProviderServiceHandle(providers),
        )
    }

    #[tokio::test]
    async fn load_failure_blocks_management_but_consumption_returns_fallback() {
        let home = std::env::temp_dir().join(format!("plugin-service-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(home.join("plugins"))
            .await
            .unwrap();
        tokio::fs::write(home.join("plugins/installed.json"), "{")
            .await
            .unwrap();
        let service = service(&home, HashMap::new());
        assert!(service.plugin_skill_roots().await.unwrap().is_empty());
        let error = service.list_plugins().await.unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error2>().unwrap().code,
            PLUGIN_LOAD_FAILED
        );
        tokio::fs::remove_dir_all(home).await.unwrap();
    }

    #[test]
    fn managed_env_overrides_existing_stdio_only() {
        use crate::agent::mcp::{
            McpServerCommonFields, McpServerRemoteConfig, McpServerStdioConfig,
        };

        let servers = HashMap::from([
            (
                "stdio".to_owned(),
                McpServerConfig::Stdio(McpServerStdioConfig {
                    command: "tool".to_owned(),
                    args: None,
                    env: Some(HashMap::from([(
                        "KIMI_CODE_BASE_URL".to_owned(),
                        "old".to_owned(),
                    )])),
                    cwd: None,
                    executor: None,
                    common: McpServerCommonFields::default(),
                }),
            ),
            (
                "http".to_owned(),
                McpServerConfig::Http(McpServerRemoteConfig {
                    url: "https://example.test".to_owned(),
                    headers: None,
                    bearer_token_env_var: None,
                    common: McpServerCommonFields::default(),
                }),
            ),
        ]);
        let output = with_managed_kimi_plugin_env(
            servers,
            &HashMap::from([("KIMI_CODE_BASE_URL".to_owned(), "new".to_owned())]),
        );
        let McpServerConfig::Stdio(stdio) = &output["stdio"] else {
            panic!("stdio")
        };
        assert_eq!(stdio.env.as_ref().unwrap()["KIMI_CODE_BASE_URL"], "new");
        let McpServerConfig::Http(http) = &output["http"] else {
            panic!("http")
        };
        assert!(http.headers.is_none());
    }
}
