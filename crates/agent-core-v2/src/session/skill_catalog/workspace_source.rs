//! Workspace-directory skill contribution source.
//!
//! Original: `packages/agent-core-v2/src/session/sessionSkillCatalog/workspaceFileSkillSource.ts`.

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
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        skill_catalog::{
            MERGE_ALL_AVAILABLE_SKILLS_SECTION, SKILL_CATALOG_RUNTIME_OPTIONS_ID,
            SKILL_DISCOVERY_SERVICE_ID, SKILL_SOURCE_PRIORITY, SkillCatalogRuntimeOptions,
            SkillContribution, SkillDiscoveryHandle, SkillRootsOptions, SkillSourceContract,
            SkillSourceResult, project_roots,
        },
    },
    session::workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
};

pub struct WorkspaceFileSkillSource {
    discovery: SkillDiscoveryHandle,
    workspace: SessionWorkspaceContextHandle,
    config: ConfigServiceHandle,
    runtime_options: Arc<SkillCatalogRuntimeOptions>,
    on_did_change_emitter: Arc<Emitter<()>>,
    disposables: DisposableStore,
}

impl WorkspaceFileSkillSource {
    pub fn new(
        discovery: SkillDiscoveryHandle,
        workspace: SessionWorkspaceContextHandle,
        config: ConfigServiceHandle,
        runtime_options: Arc<SkillCatalogRuntimeOptions>,
    ) -> Arc<Self> {
        let emitter = Arc::new(Emitter::new());
        let service = Arc::new(Self {
            discovery,
            workspace,
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
impl SkillSourceContract for WorkspaceFileSkillSource {
    fn id(&self) -> &str {
        "workspace"
    }

    fn priority(&self) -> i32 {
        SKILL_SOURCE_PRIORITY.workspace
    }

    fn on_did_change(&self) -> Option<Event<()>> {
        Some(self.on_did_change_emitter.event())
    }

    // Original: WorkspaceFileSkillSource.load().
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
        let roots = project_roots(
            &self.workspace.work_dir(),
            SkillRootsOptions {
                merge_all_available_skills: Some(merge_all_available_skills),
            },
        )
        .await?;
        let result = self.discovery.discover(&roots).await;
        Ok(result.into())
    }
}

impl Disposable for WorkspaceFileSkillSource {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

#[derive(Clone)]
pub struct WorkspaceFileSkillSourceHandle(pub Arc<WorkspaceFileSkillSource>);

impl Deref for WorkspaceFileSkillSourceHandle {
    type Target = dyn SkillSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for WorkspaceFileSkillSourceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const WORKSPACE_FILE_SKILL_SOURCE_ID: ServiceIdentifier<WorkspaceFileSkillSourceHandle> =
    ServiceIdentifier::new("workspaceFileSkillSource");

pub fn register_workspace_file_skill_source() {
    register_scoped_service(
        LifecycleScope::Session,
        WORKSPACE_FILE_SKILL_SOURCE_ID,
        SyncDescriptor::new(|accessor| {
            let discovery = accessor.get(SKILL_DISCOVERY_SERVICE_ID)?;
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let runtime_options = accessor.get(SKILL_CATALOG_RUNTIME_OPTIONS_ID)?;
            Ok(WorkspaceFileSkillSourceHandle(
                WorkspaceFileSkillSource::new(
                    (*discovery).clone(),
                    (*workspace).clone(),
                    (*config).clone(),
                    runtime_options,
                ),
            ))
        })
        .disposable(),
        InstantiationType::Eager,
        "sessionSkillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::{Map, Value};

    use super::*;
    use crate::{
        app::{
            config::{
                ConfigChangedEvent, ConfigDiagnostic, ConfigInspectValue,
                ConfigSectionChangedEvent, ConfigServiceContract, ConfigServiceError, ConfigTarget,
                ResolvedConfig,
            },
            skill_catalog::{SkillDiscoveryContract, SkillDiscoveryResult, SkillRoot, SkillSource},
        },
        session::{
            session_context::{SessionContextInput, make_session_context},
            workspace_context::{SessionWorkspaceContextContract, SessionWorkspaceContextService},
        },
    };

    struct StubConfig {
        value: Option<Value>,
    }

    impl Disposable for StubConfig {
        fn dispose(&self) -> DisposeResult {
            Ok(())
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
            Event::none()
        }
        fn get(&self, domain: &str) -> Option<Value> {
            (domain == MERGE_ALL_AVAILABLE_SKILLS_SECTION)
                .then(|| self.value.clone())
                .flatten()
        }
        fn inspect(&self, _: &str) -> ConfigInspectValue {
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
            "kimi-workspace-skill-source-{}-{}",
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

    #[tokio::test]
    async fn discovers_project_roots_and_explicit_options_short_circuit() {
        let directory = temp_dir();
        let project = directory.join("project");
        let work = project.join("nested");
        tokio::fs::create_dir_all(project.join(".git"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.join(".kimi-code/skills"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(&work).await.unwrap();
        let discovery = Arc::new(RecordingDiscovery::default());
        let handle = SkillDiscoveryHandle(discovery.clone());
        let config = ConfigServiceHandle(Arc::new(StubConfig {
            value: Some(Value::Bool(false)),
        }));

        let source = WorkspaceFileSkillSource::new(
            handle.clone(),
            workspace(&work),
            config.clone(),
            Arc::new(SkillCatalogRuntimeOptions::default()),
        );
        let result = source.load().await.unwrap();
        assert_eq!(source.id(), "workspace");
        assert_eq!(source.priority(), 30);
        assert_eq!(result.scanned_roots.as_ref().unwrap().len(), 1);
        assert_eq!(discovery.roots.lock()[0].source, SkillSource::Project);

        let explicit = WorkspaceFileSkillSource::new(
            handle,
            workspace(&work),
            config,
            Arc::new(SkillCatalogRuntimeOptions::new(Some(vec!["only".into()]))),
        );
        assert_eq!(explicit.load().await.unwrap(), SkillContribution::default());

        source.dispose().unwrap();
        explicit.dispose().unwrap();
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
