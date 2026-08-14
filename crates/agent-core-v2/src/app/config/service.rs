//! Layered global configuration service.
//!
//! Original: `packages/agent-core-v2/src/app/config/configService.ts`,
//! `ConfigService`.

use parking_lot::Mutex;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::{Emitter, Event},
        log::{LOG_SERVICE_ID, LogPayload, LogServiceHandle},
    },
    app::bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
    persistence::interface::atomic_document_store::{
        ATOMIC_TOML_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreHandle,
    },
};

use super::{
    contract::{
        CONFIG_REGISTRY_SERVICE_ID, CONFIG_SERVICE_ID, ConfigChangeSource, ConfigChangedEvent,
        ConfigDiagnostic, ConfigDiagnosticSeverity, ConfigInspectValue, ConfigRegistryHandle,
        ConfigSectionChangedEvent, ConfigServiceContract, ConfigServiceError, ConfigServiceHandle,
        ConfigTarget, ResolvedConfig,
    },
    env::apply_section_env,
    migrations::migrate_thinking_effort_max_to_high,
    pure::deep_equal,
    toml::{apply_section_to_toml, camel_to_snake, transform_toml_data},
};

const CONFIG_SCOPE: &str = "";

#[derive(Default)]
struct ConfigState {
    raw_snake: ResolvedConfig,
    raw: ResolvedConfig,
    effective: ResolvedConfig,
    memory: ResolvedConfig,
    delivered: ResolvedConfig,
    diagnostics: Vec<ConfigDiagnostic>,
}

pub struct ConfigService {
    registry: ConfigRegistryHandle,
    bootstrap: BootstrapServiceHandle,
    log: LogServiceHandle,
    document_store: AtomicDocumentStoreHandle,
    config_key: String,
    state: Mutex<ConfigState>,
    transition: AsyncMutex<()>,
    initialized: OnceCell<()>,
    changed: Arc<Emitter<ConfigChangedEvent>>,
    section_changed: Arc<Emitter<ConfigSectionChangedEvent>>,
    disposables: DisposableStore,
}

impl ConfigService {
    // Original: ConfigService.constructor().
    pub fn new(
        registry: ConfigRegistryHandle,
        bootstrap: BootstrapServiceHandle,
        log: LogServiceHandle,
        document_store: AtomicDocumentStoreHandle,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            config_key: bootstrap.config_key().into(),
            registry,
            bootstrap,
            log,
            document_store,
            state: Mutex::new(ConfigState::default()),
            transition: AsyncMutex::new(()),
            initialized: OnceCell::new(),
            changed: Arc::new(Emitter::new()),
            section_changed: Arc::new(Emitter::new()),
            disposables: DisposableStore::new(),
        });

        let weak = Arc::downgrade(&service);
        service
            .disposables
            .add(
                service
                    .registry
                    .on_did_register_section()
                    .subscribe(move |event| {
                        if let Some(service) = weak.upgrade() {
                            service.revalidate_domain(&event.domain);
                        }
                    }),
            );
        let weak = Arc::downgrade(&service);
        service.disposables.add(
            service
                .registry
                .on_did_register_overlay()
                .subscribe(move |_| {
                    if let Some(service) = weak.upgrade() {
                        service.reapply_overlays();
                    }
                }),
        );
        let weak = Arc::downgrade(&service);
        service.disposables.add(
            service
                .document_store
                .watch(CONFIG_SCOPE, &service.config_key)
                .subscribe(move |_| {
                    if let Some(service) = weak.upgrade()
                        && let Ok(runtime) = tokio::runtime::Handle::try_current()
                    {
                        runtime.spawn(async move {
                            let _ = service.reload().await;
                        });
                    }
                }),
        );
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let startup = Arc::clone(&service);
            runtime.spawn(async move {
                let _ = startup.ready().await;
            });
        }
        service
    }

    async fn initialize(&self) {
        migrate_thinking_effort_max_to_high(
            &self.document_store,
            &self.config_key,
            self.bootstrap.home_dir(),
        )
        .await;
        let _transition = self.transition.lock().await;
        self.load(ConfigChangeSource::Load).await;
    }

    async fn load(&self, source: ConfigChangeSource) {
        let result = self
            .document_store
            .get::<ResolvedConfig>(CONFIG_SCOPE, &self.config_key)
            .await;
        let load_failed = result.is_err();
        let file_data = match result {
            Ok(Some(value)) => value,
            Ok(None) => Map::new(),
            Err(error) => {
                let message = error.to_string();
                self.log.0.warn(
                    "config load failed",
                    Some(LogPayload::Value(Value::String(message.clone()))),
                );
                let mut state = self.state.lock();
                state.diagnostics.clear();
                state.diagnostics.push(ConfigDiagnostic {
                    domain: None,
                    severity: ConfigDiagnosticSeverity::Error,
                    message,
                });
                Map::new()
            }
        };
        let mut state = self.state.lock();
        if !matches!(source, ConfigChangeSource::Load)
            && serde_json::to_string(&file_data).ok()
                == serde_json::to_string(&state.raw_snake).ok()
        {
            return;
        }
        if !load_failed {
            state.diagnostics.clear();
        }
        state.raw_snake = file_data.clone();
        state.raw = transform_toml_data(&file_data, self.registry.0.as_ref());
        let commits = self.rebuild_effective_locked(&mut state, source, None);
        drop(state);
        self.fire_commits(commits);
    }

    fn build_effective(
        &self,
        raw: &ResolvedConfig,
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) -> ResolvedConfig {
        let mut effective = Map::new();
        for (domain, value) in raw {
            match self.registry.validate(domain, value) {
                Ok(value) => {
                    effective.insert(domain.clone(), value);
                }
                Err(error) => diagnostics.push(ConfigDiagnostic {
                    domain: Some(domain.clone()),
                    severity: ConfigDiagnosticSeverity::Warning,
                    message: format!("Ignored invalid config section '{domain}': {error}"),
                }),
            }
        }
        for section in self.registry.list_sections() {
            if !effective.contains_key(&section.domain)
                && let Some(default) = section.default_value
            {
                effective.insert(section.domain, default);
            }
        }
        for section in self.registry.list_sections() {
            let Some(env) = section.env else { continue };
            let get_env = |name: &str| self.bootstrap.get_env(name).map(str::to_owned);
            let result = apply_section_env(effective.get(&section.domain), &env, &get_env)
                .and_then(|value| match value {
                    Some(value) => self
                        .registry
                        .validate(&section.domain, &value)
                        .map(Some)
                        .map_err(|error| {
                            super::contract::ConfigValidationError::new(error.to_string())
                        }),
                    None => Ok(None),
                });
            match result {
                Ok(Some(value)) => {
                    effective.insert(section.domain, value);
                }
                Ok(None) => {
                    effective.shift_remove(&section.domain);
                }
                Err(error) => diagnostics.push(ConfigDiagnostic {
                    domain: Some(section.domain.clone()),
                    severity: ConfigDiagnosticSeverity::Warning,
                    message: format!("Ignoring env overlay for '{}': {error}", section.domain),
                }),
            }
        }
        self.apply_overlays(&mut effective, diagnostics);
        effective
    }

    fn apply_overlays(
        &self,
        effective: &mut ResolvedConfig,
        diagnostics: &mut Vec<ConfigDiagnostic>,
    ) {
        let get_env = |name: &str| self.bootstrap.get_env(name).map(str::to_owned);
        let validate = |domain: &str, value: &Value| {
            self.registry
                .validate(domain, value)
                .map_err(|error| super::contract::ConfigValidationError::new(error.to_string()))
        };
        for overlay in self.registry.list_effective_overlays() {
            if let Err(error) = overlay.apply(effective, &get_env, &validate) {
                diagnostics.push(ConfigDiagnostic {
                    domain: None,
                    severity: ConfigDiagnosticSeverity::Warning,
                    message: format!("Ignoring config environment overlay: {error}"),
                });
            }
        }
    }

    fn rebuild_effective_locked(
        &self,
        state: &mut ConfigState,
        source: ConfigChangeSource,
        domains: Option<Vec<String>>,
    ) -> Vec<ConfigChangedEvent> {
        let previous = state.effective.clone();
        state.effective = self.build_effective(&state.raw, &mut state.diagnostics);
        let domains = domains.unwrap_or_else(|| union_keys(&previous, &state.effective));
        commit_locked(state, source, &domains)
    }

    fn strip_env_locked(
        &self,
        state: &ConfigState,
        domain: &str,
        value: Option<Value>,
    ) -> Option<Value> {
        let mut result = value;
        if let Some(strip) = self
            .registry
            .get_section(domain)
            .and_then(|section| section.strip_env)
            && let Some(value) = result.as_ref()
        {
            result = strip(value, state.raw_snake.get(domain));
        }
        for overlay in self.registry.list_effective_overlays() {
            let value = result.as_ref()?;
            result = overlay.strip(domain, value, &state.raw_snake);
        }
        result
    }

    fn reapply_overlays(&self) {
        let mut state = self.state.lock();
        let commits = self.rebuild_effective_locked(&mut state, ConfigChangeSource::Reload, None);
        drop(state);
        self.fire_commits(commits);
    }

    fn revalidate_domain(&self, domain: &str) {
        let Some(section) = self.registry.get_section(domain) else {
            return;
        };
        let mut state = self.state.lock();
        if let Some(from_toml) = section.from_toml
            && let Some(raw) = state.raw_snake.get(&camel_to_snake(domain)).cloned()
        {
            state.raw.insert(domain.into(), from_toml(&raw));
        }
        let candidate = state
            .raw
            .get(domain)
            .cloned()
            .or(section.default_value.clone());
        let Some(candidate) = candidate else { return };
        let Ok(validated) = self.registry.validate(domain, &candidate) else {
            return;
        };
        state.effective.insert(domain.into(), validated);
        let ConfigState {
            effective,
            diagnostics,
            ..
        } = &mut *state;
        self.apply_overlays(effective, diagnostics);
        if let Some(env) = section.env {
            let get_env = |name: &str| self.bootstrap.get_env(name).map(str::to_owned);
            let resolved =
                apply_section_env(effective.get(domain), &env, &get_env).and_then(|value| {
                    match value {
                        Some(value) => {
                            self.registry
                                .validate(domain, &value)
                                .map(Some)
                                .map_err(|error| {
                                    super::contract::ConfigValidationError::new(error.to_string())
                                })
                        }
                        None => Ok(None),
                    }
                });
            match resolved {
                Ok(Some(value)) => {
                    effective.insert(domain.into(), value);
                }
                Ok(None) => {
                    effective.shift_remove(domain);
                }
                Err(error) => diagnostics.push(ConfigDiagnostic {
                    domain: Some(domain.into()),
                    severity: ConfigDiagnosticSeverity::Warning,
                    message: format!("Ignoring env overlay for '{domain}': {error}"),
                }),
            }
        }
        let commits = commit_locked(&mut state, ConfigChangeSource::Reload, &[domain.into()]);
        drop(state);
        self.fire_commits(commits);
    }

    fn fire_commits(&self, commits: Vec<ConfigChangedEvent>) {
        for event in commits {
            self.changed.fire(&event);
            if !same_optional_value(&event.value, &event.previous_value) {
                self.section_changed.fire(&event);
            }
        }
    }
}

#[async_trait]
impl ConfigServiceContract for ConfigService {
    async fn ready(&self) -> Result<(), ConfigServiceError> {
        self.initialized.get_or_init(|| self.initialize()).await;
        Ok(())
    }

    fn on_did_change_configuration(&self) -> Event<ConfigChangedEvent> {
        self.changed.event()
    }

    fn on_did_section_change(&self) -> Event<ConfigSectionChangedEvent> {
        self.section_changed.event()
    }

    // Original: ConfigService.get().
    fn get(&self, domain: &str) -> Option<Value> {
        let mut state = self.state.lock();
        if let Some(value) = state.memory.get(domain) {
            return Some(value.clone());
        }
        if let Some(env) = self
            .registry
            .get_section(domain)
            .and_then(|section| section.env)
        {
            let get_env = |name: &str| self.bootstrap.get_env(name).map(str::to_owned);
            if let Ok(Some(next)) = apply_section_env(state.effective.get(domain), &env, &get_env)
                && let Ok(validated) = self.registry.validate(domain, &next)
            {
                state.effective.insert(domain.into(), validated);
            }
        }
        state.effective.get(domain).cloned()
    }

    fn inspect(&self, domain: &str) -> ConfigInspectValue {
        let value = self.get(domain);
        let state = self.state.lock();
        ConfigInspectValue {
            value,
            default_value: self.registry.default_value(domain),
            user_value: state.raw.get(domain).cloned(),
            memory_value: state.memory.get(domain).cloned(),
        }
    }

    fn get_all(&self) -> ResolvedConfig {
        let state = self.state.lock();
        let mut result = state.effective.clone();
        result.extend(state.memory.clone());
        result
    }

    async fn set(
        &self,
        domain: &str,
        patch: Option<Value>,
        target: ConfigTarget,
    ) -> Result<(), ConfigServiceError> {
        self.ready().await?;
        if matches!(target, ConfigTarget::Memory) {
            let mut state = self.state.lock();
            let next = self
                .registry
                .merge(domain, state.memory.get(domain), patch.as_ref());
            match next {
                Some(value) => {
                    let validated = self.registry.validate(domain, &value)?;
                    state.memory.insert(domain.into(), validated);
                }
                None => {
                    state.memory.shift_remove(domain);
                }
            }
            let commits = commit_locked(&mut state, ConfigChangeSource::Set, &[domain.into()]);
            drop(state);
            self.fire_commits(commits);
            return Ok(());
        }
        let _transition = self.transition.lock().await;
        let persisted = {
            let mut state = self.state.lock();
            let next = self
                .registry
                .merge(domain, state.raw.get(domain), patch.as_ref());
            let validated = next
                .map(|value| self.registry.validate(domain, &value))
                .transpose()?;
            let stripped = self.strip_env_locked(&state, domain, validated);
            match stripped {
                Some(value) => {
                    state.raw.insert(domain.into(), value);
                }
                None => {
                    state.raw.shift_remove(domain);
                }
            }
            let raw_value = state.raw.get(domain).cloned();
            apply_section_to_toml(
                &mut state.raw_snake,
                domain,
                raw_value.as_ref(),
                self.registry.0.as_ref(),
            );
            state.raw_snake.clone()
        };
        self.document_store
            .set(CONFIG_SCOPE, &self.config_key, &persisted)
            .await?;
        let mut state = self.state.lock();
        let commits = self.rebuild_effective_locked(
            &mut state,
            ConfigChangeSource::Set,
            Some(vec![domain.into()]),
        );
        drop(state);
        self.fire_commits(commits);
        Ok(())
    }

    async fn replace(
        &self,
        domain: &str,
        value: Option<Value>,
        target: ConfigTarget,
    ) -> Result<(), ConfigServiceError> {
        if matches!(target, ConfigTarget::Memory) {
            self.ready().await?;
            let mut state = self.state.lock();
            match value {
                Some(value) => {
                    let validated = self.registry.validate(domain, &value)?;
                    state.memory.insert(domain.into(), validated);
                }
                None => {
                    state.memory.shift_remove(domain);
                }
            }
            let commits = commit_locked(&mut state, ConfigChangeSource::Set, &[domain.into()]);
            drop(state);
            self.fire_commits(commits);
            return Ok(());
        }
        self.ready().await?;
        let _transition = self.transition.lock().await;
        let persisted = {
            let mut state = self.state.lock();
            let stripped = self.strip_env_locked(&state, domain, value);
            let validated = stripped
                .map(|value| self.registry.validate(domain, &value))
                .transpose()?;
            match validated {
                Some(value) => {
                    state.raw.insert(domain.into(), value);
                }
                None => {
                    state.raw.shift_remove(domain);
                }
            }
            let raw_value = state.raw.get(domain).cloned();
            apply_section_to_toml(
                &mut state.raw_snake,
                domain,
                raw_value.as_ref(),
                self.registry.0.as_ref(),
            );
            state.raw_snake.clone()
        };
        self.document_store
            .set(CONFIG_SCOPE, &self.config_key, &persisted)
            .await?;
        let mut state = self.state.lock();
        let commits = self.rebuild_effective_locked(
            &mut state,
            ConfigChangeSource::Set,
            Some(vec![domain.into()]),
        );
        drop(state);
        self.fire_commits(commits);
        Ok(())
    }

    async fn reload(&self) -> Result<(), ConfigServiceError> {
        self.ready().await?;
        let _transition = self.transition.lock().await;
        self.load(ConfigChangeSource::Reload).await;
        Ok(())
    }

    fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
        self.state.lock().diagnostics.clone()
    }
}

impl Disposable for ConfigService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()?;
        self.changed.dispose()?;
        self.section_changed.dispose()
    }
}

fn commit_locked(
    state: &mut ConfigState,
    source: ConfigChangeSource,
    domains: &[String],
) -> Vec<ConfigChangedEvent> {
    domains
        .iter()
        .map(|domain| {
            let previous_value = state.delivered.get(domain).cloned();
            let value = state
                .memory
                .get(domain)
                .or_else(|| state.effective.get(domain))
                .cloned();
            match &value {
                Some(value) => {
                    state.delivered.insert(domain.clone(), value.clone());
                }
                None => {
                    state.delivered.shift_remove(domain);
                }
            }
            ConfigChangedEvent {
                domain: domain.clone(),
                source,
                value,
                previous_value,
            }
        })
        .collect()
}

fn union_keys(left: &ResolvedConfig, right: &ResolvedConfig) -> Vec<String> {
    left.keys()
        .chain(right.keys())
        .fold(Vec::new(), |mut keys, key| {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
            keys
        })
}

fn same_optional_value(left: &Option<Value>, right: &Option<Value>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => deep_equal(left, right),
        (None, None) => true,
        _ => false,
    }
}

// Original: registerScopedService(... ConfigService ...).
pub fn register_config_service() {
    register_scoped_service(
        LifecycleScope::App,
        CONFIG_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let registry = accessor.get(CONFIG_REGISTRY_SERVICE_ID)?;
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let store = accessor.get(ATOMIC_TOML_DOCUMENT_STORE_SERVICE_ID)?;
            let service: Arc<dyn ConfigServiceContract> = ConfigService::new(
                (*registry).clone(),
                (*bootstrap).clone(),
                (*log).clone(),
                (*store).clone(),
            );
            Ok(ConfigServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "config",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        path::PathBuf,
    };
    use std::sync::{Arc};
    use parking_lot::Mutex;

    use futures_util::future::{BoxFuture, ready};
    use serde_json::json;

    use crate::{
        _base::log::{LogContext, LogLevel, LogPayload, LogService, Logger},
        app::bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
        persistence::{
            backends::{
                memory::in_memory_storage_service::InMemoryStorageService,
                node_fs::atomic_document_store::TomlAtomicDocumentStore,
            },
            interface::{
                atomic_document_store::{AtomicDocumentStoreHandle, AtomicDocumentStoreService},
                storage::FileSystemStorageService,
            },
        },
    };

    use super::*;
    use crate::app::config::{
        ConfigRegistry, ConfigRegistryContract, ConfigSchema, RegisterSectionOptions,
    };

    struct StubLog;

    impl Logger for StubLog {
        fn error(&self, _: &str, _: Option<LogPayload>) {}
        fn warn(&self, _: &str, _: Option<LogPayload>) {}
        fn info(&self, _: &str, _: Option<LogPayload>) {}
        fn debug(&self, _: &str, _: Option<LogPayload>) {}
        fn child(&self, _: LogContext) -> Arc<dyn Logger> {
            Arc::new(Self)
        }
    }

    impl Disposable for StubLog {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    impl LogService for StubLog {
        fn level(&self) -> LogLevel {
            LogLevel::Off
        }

        fn set_level(&self, _: LogLevel) {}

        fn flush(&self) -> BoxFuture<'_, std::io::Result<()>> {
            Box::pin(ready(Ok(())))
        }
    }

    struct Fixture {
        service: Arc<ConfigService>,
        store: AtomicDocumentStoreHandle,
        home: PathBuf,
    }

    impl Fixture {
        async fn new(env: HashMap<String, String>) -> Self {
            let home = std::env::temp_dir().join(format!("kimi-config-{}", uuid::Uuid::new_v4()));
            let bootstrap: Arc<dyn BootstrapServiceContract> =
                Arc::new(BootstrapService::new(BootstrapOptions {
                    home_dir: home.clone(),
                    config_path: home.join("config.toml"),
                    os_home_dir: home.clone(),
                    platform: "linux".into(),
                    arch: "x64".into(),
                    cwd: home.clone(),
                    env,
                    client_version: "test".into(),
                }));
            let registry = Arc::new(ConfigRegistry::new().unwrap());
            registry
                .register_section(
                    "agent",
                    ConfigSchema::new(|value| {
                        value.is_object().then(|| value.clone()).ok_or_else(|| {
                            super::super::ConfigValidationError::new("expected object")
                        })
                    }),
                    RegisterSectionOptions {
                        default_value: Some(json!({"enabled": true, "nested": {"keep": 1}})),
                        ..RegisterSectionOptions::default()
                    },
                )
                .unwrap();
            let registry_contract: Arc<dyn ConfigRegistryContract> = registry;
            let storage: Arc<dyn FileSystemStorageService> =
                Arc::new(InMemoryStorageService::default());
            let backend: Arc<dyn AtomicDocumentStoreService> =
                Arc::new(TomlAtomicDocumentStore::new(storage));
            let store = AtomicDocumentStoreHandle(backend);
            let log: Arc<dyn LogService> = Arc::new(StubLog);
            let service = ConfigService::new(
                ConfigRegistryHandle(registry_contract),
                BootstrapServiceHandle(bootstrap),
                LogServiceHandle(log),
                store.clone(),
            );
            service.ready().await.unwrap();
            Self {
                service,
                store,
                home,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    #[tokio::test]
    async fn resolves_default_user_and_memory_layers_and_persists_only_user() {
        let fixture = Fixture::new(HashMap::new()).await;
        assert_eq!(
            fixture.service.get("agent"),
            Some(json!({"enabled": true, "nested": {"keep": 1}}))
        );

        fixture
            .service
            .set("agent", Some(json!({"enabled": false})), ConfigTarget::User)
            .await
            .unwrap();
        assert_eq!(
            fixture.service.get("agent"),
            Some(json!({"enabled": false}))
        );
        let persisted = fixture
            .store
            .get::<ResolvedConfig>(CONFIG_SCOPE, "config.toml")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted["agent"]["enabled"], false);

        fixture
            .service
            .replace("agent", Some(json!({"memory": true})), ConfigTarget::Memory)
            .await
            .unwrap();
        assert_eq!(fixture.service.get("agent"), Some(json!({"memory": true})));
        assert_eq!(
            fixture.service.inspect("agent").user_value.unwrap()["enabled"],
            false
        );
        fixture
            .service
            .replace("agent", None, ConfigTarget::Memory)
            .await
            .unwrap();
        assert_eq!(fixture.service.get("agent").unwrap()["enabled"], false);
    }

    #[tokio::test]
    async fn emits_touched_and_value_changed_events_with_previous_values() {
        let fixture = Fixture::new(HashMap::new()).await;
        let touched = Arc::new(Mutex::new(Vec::new()));
        let changed = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&touched);
        let _touched = fixture
            .service
            .on_did_change_configuration()
            .subscribe(move |event| target.lock().push(event.clone()));
        let target = Arc::clone(&changed);
        let _changed = fixture
            .service
            .on_did_section_change()
            .subscribe(move |event| target.lock().push(event.clone()));

        fixture
            .service
            .set("agent", Some(json!({"enabled": false})), ConfigTarget::User)
            .await
            .unwrap();
        fixture
            .service
            .set("agent", Some(json!({"enabled": false})), ConfigTarget::User)
            .await
            .unwrap();
        assert_eq!(touched.lock().len(), 2);
        assert_eq!(changed.lock().len(), 1);
        assert_eq!(
            changed.lock()[0].previous_value.as_ref().unwrap()["enabled"],
            true
        );
        assert_eq!(
            changed.lock()[0].value.as_ref().unwrap()["enabled"],
            false
        );
    }

    #[tokio::test]
    async fn reloads_external_toml_and_preserves_unknown_sections() {
        let fixture = Fixture::new(HashMap::new()).await;
        fixture
            .store
            .set(
                CONFIG_SCOPE,
                "config.toml",
                &json!({"agent": {"enabled": false}, "unknown_section": {"raw_key": 1}}),
            )
            .await
            .unwrap();
        fixture.service.reload().await.unwrap();
        assert_eq!(fixture.service.get("agent").unwrap()["enabled"], false);
        assert_eq!(
            fixture.service.get("unknownSection"),
            Some(json!({"rawKey": 1}))
        );
        assert!(fixture.service.diagnostics().is_empty());
    }
}
