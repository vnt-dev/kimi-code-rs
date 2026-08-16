//! User and brand-directory skill contribution source.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/userFileSkillSource.ts`.

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
    },
};

use super::{
    config_section::MERGE_ALL_AVAILABLE_SKILLS_SECTION,
    discovery::{SKILL_DISCOVERY_SERVICE_ID, SkillDiscoveryHandle},
    roots::{SkillRootsOptions, user_roots},
    runtime_options::{SKILL_CATALOG_RUNTIME_OPTIONS_ID, SkillCatalogRuntimeOptions},
    source::{SKILL_SOURCE_PRIORITY, SkillContribution, SkillSourceContract, SkillSourceResult},
};

pub struct UserFileSkillSource {
    discovery: SkillDiscoveryHandle,
    bootstrap: BootstrapServiceHandle,
    config: ConfigServiceHandle,
    runtime_options: Arc<SkillCatalogRuntimeOptions>,
    on_did_change_emitter: Arc<Emitter<()>>,
    disposables: DisposableStore,
}

impl UserFileSkillSource {
    // Original: UserFileSkillSource.constructor(). Arc and Weak keep the config
    // subscription from retaining the source after its App scope is disposed.
    pub fn new(
        discovery: SkillDiscoveryHandle,
        bootstrap: BootstrapServiceHandle,
        config: ConfigServiceHandle,
        runtime_options: Arc<SkillCatalogRuntimeOptions>,
    ) -> Arc<Self> {
        let emitter = Arc::new(Emitter::new());
        let service = Arc::new(Self {
            discovery,
            bootstrap,
            config: config.clone(),
            runtime_options,
            on_did_change_emitter: Arc::clone(&emitter),
            disposables: DisposableStore::new(),
        });
        service.disposables.add(emitter);
        let weak = Arc::downgrade(&service);
        let subscription = config.on_did_section_change().subscribe(move |event| {
            if event.domain == MERGE_ALL_AVAILABLE_SKILLS_SECTION
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
impl SkillSourceContract for UserFileSkillSource {
    fn id(&self) -> &str {
        "user"
    }

    fn priority(&self) -> i32 {
        SKILL_SOURCE_PRIORITY.user
    }

    fn on_did_change(&self) -> Option<Event<()>> {
        Some(self.on_did_change_emitter.event())
    }

    // Original: UserFileSkillSource.load(). Sequential awaits preserve the
    // config-ready -> root-resolution -> discovery call order.
    async fn load(&self) -> SkillSourceResult<SkillContribution> {
        if self
            .runtime_options
            .explicit_dirs
            .as_ref()
            .is_some_and(|directories| !directories.is_empty())
        {
            return Ok(SkillContribution::default());
        }
        self.config.ready().await?;
        let merge_all_available_skills = self
            .config
            .get(MERGE_ALL_AVAILABLE_SKILLS_SECTION)
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        let roots = user_roots(
            self.bootstrap.home_dir(),
            self.bootstrap.os_home_dir(),
            SkillRootsOptions {
                merge_all_available_skills: Some(merge_all_available_skills),
            },
        )
        .await?;
        let result = self.discovery.discover(&roots).await;
        Ok(result.into())
    }
}

impl Disposable for UserFileSkillSource {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

#[derive(Clone)]
pub struct UserFileSkillSourceHandle(pub Arc<UserFileSkillSource>);

impl Deref for UserFileSkillSourceHandle {
    type Target = dyn SkillSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for UserFileSkillSourceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const USER_FILE_SKILL_SOURCE_ID: ServiceIdentifier<UserFileSkillSourceHandle> =
    ServiceIdentifier::new("userFileSkillSource");

pub fn register_user_file_skill_source() {
    register_scoped_service(
        LifecycleScope::App,
        USER_FILE_SKILL_SOURCE_ID,
        SyncDescriptor::new(|accessor| {
            let discovery = accessor.get(SKILL_DISCOVERY_SERVICE_ID)?;
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let runtime_options = accessor.get(SKILL_CATALOG_RUNTIME_OPTIONS_ID)?;
            Ok(UserFileSkillSourceHandle(UserFileSkillSource::new(
                (*discovery).clone(),
                (*bootstrap).clone(),
                (*config).clone(),
                runtime_options,
            )))
        })
        .disposable(),
        InstantiationType::Eager,
        "skillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::{collections::HashMap, path::PathBuf};

    use serde_json::{Map, Value};

    use super::*;
    use crate::{
        _base::di::lifecycle::DisposeResult,
        app::{
            bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
            config::{
                ConfigChangeSource, ConfigChangedEvent, ConfigDiagnostic, ConfigInspectValue,
                ConfigSectionChangedEvent, ConfigServiceContract, ConfigServiceError, ConfigTarget,
                ResolvedConfig,
            },
            skill_catalog::{SkillDiscoveryContract, SkillDiscoveryResult, SkillRoot},
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
            (domain == MERGE_ALL_AVAILABLE_SKILLS_SECTION)
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
            _domain: &str,
            _patch: Option<Value>,
            _target: ConfigTarget,
        ) -> Result<(), ConfigServiceError> {
            unreachable!("not used")
        }

        async fn replace(
            &self,
            _domain: &str,
            _value: Option<Value>,
            _target: ConfigTarget,
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
            "kimi-user-skill-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn bootstrap(home: PathBuf, os_home: PathBuf) -> BootstrapServiceHandle {
        let service: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                config_path: home.join("config.toml"),
                home_dir: home,
                os_home_dir: os_home,
                platform: "linux".into(),
                arch: "x64".into(),
                cwd: "/tmp".into(),
                env: HashMap::new(),
                client_version: "test".into(),
            }));
        BootstrapServiceHandle(service)
    }

    #[tokio::test]
    async fn discovers_user_roots_short_circuits_explicit_dirs_and_filters_events() {
        let directory = temp_dir();
        let home = directory.join("kimi-home");
        let os_home = directory.join("os-home");
        tokio::fs::create_dir_all(home.join("skills"))
            .await
            .unwrap();
        let discovery = Arc::new(RecordingDiscovery::default());
        let discovery_handle = SkillDiscoveryHandle(discovery.clone());
        let config = Arc::new(StubConfig {
            value: Some(Value::Bool(false)),
            changed: Emitter::new(),
        });
        let config_handle = ConfigServiceHandle(config.clone());
        let source = UserFileSkillSource::new(
            discovery_handle.clone(),
            bootstrap(home, os_home),
            config_handle.clone(),
            Arc::new(SkillCatalogRuntimeOptions::default()),
        );

        let contribution = source.load().await.unwrap();
        assert_eq!(contribution.scanned_roots.as_ref().unwrap().len(), 1);
        assert_eq!(source.id(), "user");
        assert_eq!(source.priority(), 20);

        let events = Arc::new(AtomicU64::new(0));
        let events_for_listener = Arc::clone(&events);
        let _subscription = source.on_did_change().unwrap().subscribe(move |_| {
            events_for_listener.fetch_add(1, Ordering::Relaxed);
        });
        config.changed.fire(&ConfigChangedEvent {
            domain: "other".into(),
            source: ConfigChangeSource::Set,
            value: None,
            previous_value: None,
        });
        config.changed.fire(&ConfigChangedEvent {
            domain: MERGE_ALL_AVAILABLE_SKILLS_SECTION.into(),
            source: ConfigChangeSource::Set,
            value: Some(Value::Bool(true)),
            previous_value: Some(Value::Bool(false)),
        });
        assert_eq!(events.load(Ordering::Relaxed), 1);

        let explicit = UserFileSkillSource::new(
            discovery_handle,
            bootstrap(directory.join("missing"), directory.join("also-missing")),
            config_handle,
            Arc::new(SkillCatalogRuntimeOptions::new(Some(vec![
                "explicit".into(),
            ]))),
        );
        assert_eq!(explicit.load().await.unwrap(), SkillContribution::default());

        source.dispose().unwrap();
        explicit.dispose().unwrap();
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
