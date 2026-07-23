//! Explicit-directory skill contribution source.
//!
//! Original: `packages/agent-core-v2/src/session/sessionSkillCatalog/explicitFileSkillSource.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        skill_catalog::{
            SKILL_CATALOG_RUNTIME_OPTIONS_ID, SKILL_DISCOVERY_SERVICE_ID, SKILL_SOURCE_PRIORITY,
            SkillCatalogRuntimeOptions, SkillContribution, SkillDiscoveryHandle,
            SkillSource as SkillDefinitionSource, SkillSourceContract, SkillSourceResult,
            configured_roots,
        },
    },
    session::workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
};

pub struct ExplicitFileSkillSource {
    discovery: SkillDiscoveryHandle,
    runtime_options: Arc<SkillCatalogRuntimeOptions>,
    workspace: SessionWorkspaceContextHandle,
    bootstrap: BootstrapServiceHandle,
}

impl ExplicitFileSkillSource {
    pub fn new(
        discovery: SkillDiscoveryHandle,
        runtime_options: Arc<SkillCatalogRuntimeOptions>,
        workspace: SessionWorkspaceContextHandle,
        bootstrap: BootstrapServiceHandle,
    ) -> Self {
        Self {
            discovery,
            runtime_options,
            workspace,
            bootstrap,
        }
    }
}

#[async_trait]
impl SkillSourceContract for ExplicitFileSkillSource {
    fn id(&self) -> &str {
        "explicit"
    }

    fn priority(&self) -> i32 {
        SKILL_SOURCE_PRIORITY.user
    }

    // Original: ExplicitFileSkillSource.load().
    async fn load(&self) -> SkillSourceResult<SkillContribution> {
        let Some(directories) = self
            .runtime_options
            .explicit_dirs
            .as_deref()
            .filter(|directories| !directories.is_empty())
        else {
            return Ok(SkillContribution::default());
        };
        let roots = configured_roots(
            directories,
            &self.workspace.work_dir(),
            self.bootstrap.os_home_dir(),
            SkillDefinitionSource::User,
        )
        .await?;
        let result = self.discovery.discover(&roots).await;
        Ok(SkillContribution {
            skills: result.skills,
            skipped: Some(result.skipped),
            scanned_roots: Some(result.scanned_roots),
        })
    }
}

#[derive(Clone)]
pub struct ExplicitFileSkillSourceHandle(pub Arc<dyn SkillSourceContract>);

impl Deref for ExplicitFileSkillSourceHandle {
    type Target = dyn SkillSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const EXPLICIT_FILE_SKILL_SOURCE_ID: ServiceIdentifier<ExplicitFileSkillSourceHandle> =
    ServiceIdentifier::new("explicitFileSkillSource");

pub fn register_explicit_file_skill_source() {
    register_scoped_service(
        LifecycleScope::Session,
        EXPLICIT_FILE_SKILL_SOURCE_ID,
        SyncDescriptor::new(|accessor| {
            let discovery = accessor.get(SKILL_DISCOVERY_SERVICE_ID)?;
            let runtime_options = accessor.get(SKILL_CATALOG_RUNTIME_OPTIONS_ID)?;
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let source: Arc<dyn SkillSourceContract> = Arc::new(ExplicitFileSkillSource::new(
                (*discovery).clone(),
                runtime_options,
                (*workspace).clone(),
                (*bootstrap).clone(),
            ));
            Ok(ExplicitFileSkillSourceHandle(source))
        }),
        InstantiationType::Eager,
        "sessionSkillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::{
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };

    use super::*;
    use crate::{
        app::{
            bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
            skill_catalog::{SkillDiscoveryContract, SkillDiscoveryResult, SkillRoot},
        },
        session::{
            session_context::{SessionContextInput, make_session_context},
            workspace_context::{SessionWorkspaceContextContract, SessionWorkspaceContextService},
        },
    };

    #[derive(Default)]
    struct RecordingDiscovery {
        roots: Mutex<Vec<SkillRoot>>,
    }

    #[async_trait]
    impl SkillDiscoveryContract for RecordingDiscovery {
        async fn discover(&self, roots: &[SkillRoot]) -> SkillDiscoveryResult {
            *self.roots.lock().unwrap() = roots.to_vec();
            SkillDiscoveryResult {
                scanned_roots: roots.iter().map(|root| root.path.clone()).collect(),
                ..SkillDiscoveryResult::default()
            }
        }
    }

    fn temp_dir() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "kimi-explicit-skill-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn workspace(work_dir: &Path) -> SessionWorkspaceContextHandle {
        let context = make_session_context(SessionContextInput {
            session_id: "s".into(),
            workspace_id: "w".into(),
            session_dir: "/sessions/s".into(),
            session_scope: "sessions/s".into(),
            cwd: work_dir.to_string_lossy().into_owned(),
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
    async fn empty_options_skip_discovery_and_nonempty_options_resolve_user_roots() {
        let directory = temp_dir();
        let project = directory.join("project");
        tokio::fs::create_dir_all(project.join(".git"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.join("relative-skills"))
            .await
            .unwrap();
        let discovery = Arc::new(RecordingDiscovery::default());
        let handle = SkillDiscoveryHandle(discovery.clone());

        let empty = ExplicitFileSkillSource::new(
            handle.clone(),
            Arc::new(SkillCatalogRuntimeOptions::default()),
            workspace(&project),
            bootstrap(&directory),
        );
        assert_eq!(empty.load().await.unwrap(), SkillContribution::default());
        assert!(discovery.roots.lock().unwrap().is_empty());

        let source = ExplicitFileSkillSource::new(
            handle,
            Arc::new(SkillCatalogRuntimeOptions::new(Some(vec![
                "relative-skills".into(),
            ]))),
            workspace(&project),
            bootstrap(&directory),
        );
        let contribution = source.load().await.unwrap();
        assert_eq!(source.id(), "explicit");
        assert_eq!(source.priority(), 20);
        assert_eq!(contribution.scanned_roots.as_ref().unwrap().len(), 1);
        assert_eq!(
            discovery.roots.lock().unwrap()[0].source,
            SkillDefinitionSource::User
        );

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
