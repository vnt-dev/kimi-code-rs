//! Explicit CLI-selected agent-file profile source.
//!
//! Original: `packages/agent-core-v2/src/session/sessionAgentProfileCatalog/explicitFileAgentSource.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::{
        agent_file_catalog::{
            AGENT_CATALOG_RUNTIME_OPTIONS_ID, AGENT_PROFILE_SOURCE_PRIORITY_EXPLICIT,
            AgentCatalogRuntimeOptions, AgentFileSource, AgentProfileContribution,
            AgentProfileSourceContract, AgentProfileSourceError, UserFileAgentSourceHandle,
            agent_profile_from_file, parse_agent_file_text, resolve_agent_path,
        },
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
    },
    os::interface::host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
    session::workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
};

pub struct ExplicitFileAgentSource {
    runtime_options: Arc<AgentCatalogRuntimeOptions>,
    workspace: SessionWorkspaceContextHandle,
    bootstrap: BootstrapServiceHandle,
    fs: HostFileSystemServiceHandle,
    user: UserFileAgentSourceHandle,
}

impl ExplicitFileAgentSource {
    pub fn new(
        runtime_options: Arc<AgentCatalogRuntimeOptions>,
        workspace: SessionWorkspaceContextHandle,
        bootstrap: BootstrapServiceHandle,
        fs: HostFileSystemServiceHandle,
        user: UserFileAgentSourceHandle,
    ) -> Self {
        Self {
            runtime_options,
            workspace,
            bootstrap,
            fs,
            user,
        }
    }
}

#[async_trait]
impl AgentProfileSourceContract for ExplicitFileAgentSource {
    fn id(&self) -> &str {
        "explicit"
    }

    fn priority(&self) -> i32 {
        AGENT_PROFILE_SOURCE_PRIORITY_EXPLICIT
    }

    fn fatal(&self) -> bool {
        true
    }

    // Original: ExplicitFileAgentSource.load(). Any read or parse failure is
    // deliberately propagated, making this source fatal to catalog readiness.
    async fn load(&self) -> Result<AgentProfileContribution, AgentProfileSourceError> {
        let files = self
            .runtime_options
            .explicit_files
            .as_deref()
            .unwrap_or_default();
        let mut profiles = Vec::with_capacity(files.len());
        let work_dir = self.workspace.work_dir();
        for file in files {
            let file_path = resolve_agent_path(file, &work_dir, self.bootstrap.os_home_dir());
            let text = self.fs.read_text(&file_path, None).await?;
            let path = file_path.to_string_lossy().replace('\\', "/");
            let definition =
                parse_agent_file_text(crate::app::agent_file_catalog::ParseAgentFileOptions {
                    path: &path,
                    source: AgentFileSource::Explicit,
                    text: &text,
                })?;
            let user = self.user.0.clone();
            profiles.push(Arc::new(agent_profile_from_file(
                definition,
                Arc::new(move |context| user.get_default_profile().render_system_prompt(context)),
            )));
        }
        Ok(AgentProfileContribution {
            profiles,
            skipped: None,
            scanned_roots: None,
        })
    }
}

#[derive(Clone)]
pub struct ExplicitFileAgentSourceHandle(pub Arc<dyn AgentProfileSourceContract>);

impl Deref for ExplicitFileAgentSourceHandle {
    type Target = dyn AgentProfileSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const EXPLICIT_FILE_AGENT_SOURCE_ID: ServiceIdentifier<ExplicitFileAgentSourceHandle> =
    ServiceIdentifier::new("explicitFileAgentSource");

pub fn register_explicit_file_agent_source() {
    register_scoped_service(
        LifecycleScope::Session,
        EXPLICIT_FILE_AGENT_SOURCE_ID,
        SyncDescriptor::new(|accessor| {
            let runtime_options = accessor.get(AGENT_CATALOG_RUNTIME_OPTIONS_ID)?;
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let user = accessor.get(crate::app::agent_file_catalog::USER_FILE_AGENT_SOURCE_ID)?;
            let source: Arc<dyn AgentProfileSourceContract> =
                Arc::new(ExplicitFileAgentSource::new(
                    runtime_options,
                    (*workspace).clone(),
                    (*bootstrap).clone(),
                    (*fs).clone(),
                    (*user).clone(),
                ));
            Ok(ExplicitFileAgentSourceHandle(source))
        }),
        InstantiationType::Eager,
        "sessionAgentProfileCatalog",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        _base::log::{LogContext, LogPayload, Logger},
        app::{
            agent_file_catalog::{UserFileAgentSource, UserFileAgentSourceHandle},
            agent_profile_catalog::{
                AgentProfile, AgentProfileCatalogContract, AgentProfileCatalogHandle,
                AgentSystemPrompt, MissingDefaultAgentProfile,
            },
            bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
        },
        os::backends::node_local::host_fs_service::HostFileSystem,
        session::{
            session_context::{SessionContextInput, make_session_context},
            workspace_context::{SessionWorkspaceContextHandle, SessionWorkspaceContextService},
        },
    };

    use super::*;

    struct StaticCatalog(Arc<AgentProfile>);

    impl AgentProfileCatalogContract for StaticCatalog {
        fn get(&self, name: &str) -> Option<Arc<AgentProfile>> {
            (name == self.0.name).then(|| Arc::clone(&self.0))
        }
        fn get_default(&self) -> Result<Arc<AgentProfile>, MissingDefaultAgentProfile> {
            Ok(Arc::clone(&self.0))
        }
        fn list(&self) -> Vec<Arc<AgentProfile>> {
            vec![Arc::clone(&self.0)]
        }
    }

    struct SilentLogger;

    impl Logger for SilentLogger {
        fn error(&self, _message: &str, _payload: Option<LogPayload>) {}
        fn warn(&self, _message: &str, _payload: Option<LogPayload>) {}
        fn info(&self, _message: &str, _payload: Option<LogPayload>) {}
        fn debug(&self, _message: &str, _payload: Option<LogPayload>) {}
        fn child(&self, _context: LogContext) -> Arc<dyn Logger> {
            Arc::new(Self)
        }
    }

    fn builtin() -> Arc<AgentProfile> {
        let prompt: AgentSystemPrompt = Arc::new(|_| "BASE".into());
        Arc::new(AgentProfile {
            name: "agent".into(),
            description: None,
            when_to_use: None,
            is_override: Some(false),
            tools: None,
            disallowed_tools: None,
            subagents: None,
            system_prompt: prompt,
            prompt_prefix: None,
            summary_policy: None,
        })
    }

    #[tokio::test]
    async fn explicit_files_resolve_from_workdir_and_are_fatal_profiles() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kimi-explicit-agent-{}-{nonce}",
            std::process::id()
        ));
        let home = root.join("home");
        let work_dir = root.join("work");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();
        tokio::fs::write(
            work_dir.join("review.md"),
            "---\nname: review\ndescription: review\n---\n${base_prompt} review",
        )
        .await
        .unwrap();

        let bootstrap: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                home_dir: home.clone(),
                config_path: home.join("config.toml"),
                os_home_dir: root.clone(),
                platform: "linux".into(),
                arch: "x64".into(),
                cwd: work_dir.clone(),
                env: HashMap::new(),
                client_version: "test".into(),
            }));
        let fs = HostFileSystemServiceHandle(Arc::new(HostFileSystem));
        let catalog = AgentProfileCatalogHandle(Arc::new(StaticCatalog(builtin())));
        let user = UserFileAgentSourceHandle(Arc::new(
            UserFileAgentSource::new(
                BootstrapServiceHandle(Arc::clone(&bootstrap)),
                fs.clone(),
                Arc::new(SilentLogger),
                catalog,
            )
            .unwrap(),
        ));
        let context = make_session_context(SessionContextInput {
            session_id: "s".into(),
            workspace_id: "w".into(),
            session_dir: root.to_string_lossy().into(),
            session_scope: "scope".into(),
            cwd: work_dir.to_string_lossy().into(),
            meta_scope: None,
        });
        let workspace = SessionWorkspaceContextHandle(Arc::new(
            SessionWorkspaceContextService::new(&context).unwrap(),
        ));
        let source = ExplicitFileAgentSource::new(
            Arc::new(AgentCatalogRuntimeOptions::new(Some(vec![
                "review.md".into(),
            ]))),
            workspace,
            BootstrapServiceHandle(bootstrap),
            fs,
            user,
        );
        let contribution = source.load().await.unwrap();
        assert_eq!(source.id(), "explicit");
        assert!(source.fatal());
        assert_eq!(contribution.profiles.len(), 1);
        assert_eq!(contribution.profiles[0].is_override, Some(true));
        assert_eq!(
            contribution.profiles[0].render_system_prompt(&Default::default()),
            "BASE review"
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
