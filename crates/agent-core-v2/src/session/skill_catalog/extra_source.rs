//! Configured-extra-directory skill contribution source.
//!
//! Original: `packages/agent-core-v2/src/session/sessionSkillCatalog/extraFileSkillSource.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::{Emitter, Event},
    },
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        skill_catalog::{
            EXTRA_SKILL_DIRS_SECTION, SKILL_DISCOVERY_SERVICE_ID, SKILL_SOURCE_PRIORITY,
            SkillContribution, SkillDiscoveryHandle, SkillSource as SkillDefinitionSource,
            SkillSourceContract, SkillSourceResult, configured_roots,
        },
    },
    session::workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
};

pub struct ExtraFileSkillSource {
    discovery: SkillDiscoveryHandle,
    config: ConfigServiceHandle,
    workspace: SessionWorkspaceContextHandle,
    bootstrap: BootstrapServiceHandle,
    on_did_change_emitter: Arc<Emitter<()>>,
    disposables: DisposableStore,
}

impl ExtraFileSkillSource {
    pub fn new(
        discovery: SkillDiscoveryHandle,
        config: ConfigServiceHandle,
        workspace: SessionWorkspaceContextHandle,
        bootstrap: BootstrapServiceHandle,
    ) -> Arc<Self> {
        let emitter = Arc::new(Emitter::new());
        let service = Arc::new(Self {
            discovery,
            config: config.clone(),
            workspace,
            bootstrap,
            on_did_change_emitter: Arc::clone(&emitter),
            disposables: DisposableStore::new(),
        });
        service.disposables.add(emitter);
        let weak = Arc::downgrade(&service);
        let subscription = config.on_did_section_change().subscribe(move |event| {
            if event.domain == EXTRA_SKILL_DIRS_SECTION
                && let Some(service) = weak.upgrade()
            {
                service.on_did_change_emitter.fire(&());
            }
        });
        service.disposables.add(subscription);
        service
    }
}

#[async_trait]
impl SkillSourceContract for ExtraFileSkillSource {
    fn id(&self) -> &str {
        "extra"
    }

    fn priority(&self) -> i32 {
        SKILL_SOURCE_PRIORITY.extra
    }

    fn on_did_change(&self) -> Option<Event<()>> {
        Some(self.on_did_change_emitter.event())
    }

    // Original: ExtraFileSkillSource.load().
    async fn load(&self) -> SkillSourceResult<SkillContribution> {
        self.config.ready().await?;
        let directories = self
            .config
            .get(EXTRA_SKILL_DIRS_SECTION)
            .and_then(|value| value.as_array().cloned())
            .map(|values| {
                values
                    .into_iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let roots = configured_roots(
            &directories,
            &self.workspace.work_dir(),
            self.bootstrap.os_home_dir(),
            SkillDefinitionSource::Extra,
        )
        .await?;
        let result = self.discovery.discover(&roots).await;
        Ok(result.into())
    }
}

impl Disposable for ExtraFileSkillSource {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

#[derive(Clone)]
pub struct ExtraFileSkillSourceHandle(pub Arc<ExtraFileSkillSource>);

impl Deref for ExtraFileSkillSourceHandle {
    type Target = dyn SkillSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for ExtraFileSkillSourceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const EXTRA_FILE_SKILL_SOURCE_ID: ServiceIdentifier<ExtraFileSkillSourceHandle> =
    ServiceIdentifier::new("extraFileSkillSource");

pub fn register_extra_file_skill_source() {
    register_scoped_service(
        LifecycleScope::Session,
        EXTRA_FILE_SKILL_SOURCE_ID,
        SyncDescriptor::new(|accessor| {
            let discovery = accessor.get(SKILL_DISCOVERY_SERVICE_ID)?;
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            Ok(ExtraFileSkillSourceHandle(ExtraFileSkillSource::new(
                (*discovery).clone(),
                (*config).clone(),
                (*workspace).clone(),
                (*bootstrap).clone(),
            )))
        })
        .disposable(),
        InstantiationType::Eager,
        "sessionSkillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

    use serde_json::{Map, Value, json};

    use super::*;
    use crate::{
        app::{
            bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
            config::{
                ConfigChangeSource, ConfigChangedEvent, ConfigDiagnostic, ConfigInspectValue,
                ConfigSectionChangedEvent, ConfigServiceContract, ConfigServiceError, ConfigTarget,
                ResolvedConfig,
            },
            skill_catalog::{SkillDiscoveryContract, SkillDiscoveryResult, SkillRoot},
        },
        session::{
            session_context::{SessionContextInput, make_session_context},
            workspace_context::{SessionWorkspaceContextContract, SessionWorkspaceContextService},
        },
    };

    struct StubConfig {
        value: Option<Value>,
        changed: Emitter<ConfigSectionChangedEvent>,
    }

    impl Disposable for StubConfig {
        fn dispose(&self) -> DisposeResult {
            self.changed.dispose()
        }
    }

    #[async_trait]
    impl ConfigServiceContract for StubConfig {
        async fn ready(&self) -> Result<(), ConfigServiceError> {
            Ok(())
        }
        fn on_did_change_configuration(&self) -> Event<ConfigChangedEvent> {
            Event::none()
        }
        fn on_did_section_change(&self) -> Event<ConfigSectionChangedEvent> {
            self.changed.event()
        }
        fn get(&self, domain: &str) -> Option<Value> {
            (domain == EXTRA_SKILL_DIRS_SECTION)
                .then(|| self.value.clone())
                .flatten()
        }
        fn inspect(&self, _domain: &str) -> ConfigInspectValue {
            ConfigInspectValue::default()
        }
        fn get_all(&self) -> ResolvedConfig {
            Map::new()
        }
        async fn set(
            &self,
            _: &str,
            _: Option<Value>,
            _: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            unreachable!("not used")
        }
        async fn replace(
            &self,
            _: &str,
            _: Option<Value>,
            _: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            unreachable!("not used")
        }
        async fn reload(&self) -> Result<(), ConfigServiceError> {
            unreachable!("not used")
        }
        fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
            Vec::new()
        }
    }

    #[derive(Default)]
    struct RecordingDiscovery {
        roots: Mutex<Vec<SkillRoot>>,
    }

    #[async_trait]
    impl SkillDiscoveryContract for RecordingDiscovery {
        async fn discover(&self, roots: &[SkillRoot]) -> SkillDiscoveryResult {
            *self.roots.lock() = roots.to_vec();
            SkillDiscoveryResult {
                scanned_roots: roots.iter().map(|root| root.path.clone()).collect(),
                ..SkillDiscoveryResult::default()
            }
        }
    }

    fn temp_dir() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "kimi-extra-skill-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn workspace(path: &Path) -> SessionWorkspaceContextHandle {
        let context = make_session_context(SessionContextInput {
            session_id: "s".into(),
            workspace_id: "w".into(),
            session_dir: "/sessions/s".into(),
            session_scope: "sessions/s".into(),
            cwd: path.to_string_lossy().into_owned(),
            meta_scope: None,
        });
        let service: Arc<dyn SessionWorkspaceContextContract> =
            Arc::new(SessionWorkspaceContextService::new(&context).unwrap());
        SessionWorkspaceContextHandle(service)
    }

    fn bootstrap(home: &Path) -> BootstrapServiceHandle {
        let service: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                home_dir: home.join("kimi"),
                config_path: home.join("kimi/config.toml"),
                os_home_dir: home.to_path_buf(),
                platform: "linux".into(),
                arch: "x64".into(),
                cwd: home.to_path_buf(),
                env: HashMap::new(),
                client_version: "test".into(),
            }));
        BootstrapServiceHandle(service)
    }

    #[tokio::test]
    async fn loads_configured_extra_roots_and_filters_change_events() {
        let directory = temp_dir();
        let project = directory.join("project");
        tokio::fs::create_dir_all(project.join(".git"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.join("extra-skills"))
            .await
            .unwrap();
        let discovery = Arc::new(RecordingDiscovery::default());
        let config = Arc::new(StubConfig {
            value: Some(json!(["extra-skills", "missing"])),
            changed: Emitter::new(),
        });
        let source = ExtraFileSkillSource::new(
            SkillDiscoveryHandle(discovery.clone()),
            ConfigServiceHandle(config.clone()),
            workspace(&project),
            bootstrap(&directory),
        );

        let result = source.load().await.unwrap();
        assert_eq!(source.id(), "extra");
        assert_eq!(source.priority(), 10);
        assert_eq!(result.scanned_roots.as_ref().unwrap().len(), 1);
        assert_eq!(
            discovery.roots.lock()[0].source,
            SkillDefinitionSource::Extra
        );

        let count = Arc::new(AtomicU64::new(0));
        let listener_count = Arc::clone(&count);
        let _subscription = source.on_did_change().unwrap().subscribe(move |_| {
            listener_count.fetch_add(1, Ordering::Relaxed);
        });
        for domain in ["other", EXTRA_SKILL_DIRS_SECTION] {
            config.changed.fire(&ConfigChangedEvent {
                domain: domain.into(),
                source: ConfigChangeSource::Set,
                value: None,
                previous_value: None,
            });
        }
        assert_eq!(count.load(Ordering::Relaxed), 1);

        source.dispose().unwrap();
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
