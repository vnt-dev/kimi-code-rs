//! Project-directory agent-file profile source.
//!
//! Original: `packages/agent-core-v2/src/session/sessionAgentProfileCatalog/projectFileAgentSource.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        log::{LOG_SERVICE_ID, LogEntryError, LogPayload, Logger},
    },
    app::agent_file_catalog::{
        AGENT_PROFILE_SOURCE_PRIORITY_PROJECT, AgentProfileContribution,
        AgentProfileSourceContract, AgentProfileSourceError, UserFileAgentSourceHandle,
        discover_agent_files, profiles_from_discovery, project_agent_roots,
    },
    os::interface::host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
    session::workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
};

pub struct ProjectFileAgentSource {
    workspace: SessionWorkspaceContextHandle,
    fs: HostFileSystemServiceHandle,
    log: Arc<dyn Logger>,
    user: UserFileAgentSourceHandle,
}

impl ProjectFileAgentSource {
    pub fn new(
        workspace: SessionWorkspaceContextHandle,
        fs: HostFileSystemServiceHandle,
        log: Arc<dyn Logger>,
        user: UserFileAgentSourceHandle,
    ) -> Self {
        Self {
            workspace,
            fs,
            log,
            user,
        }
    }
}

#[async_trait]
impl AgentProfileSourceContract for ProjectFileAgentSource {
    fn id(&self) -> &str {
        "project"
    }

    fn priority(&self) -> i32 {
        AGENT_PROFILE_SOURCE_PRIORITY_PROJECT
    }

    // Original: ProjectFileAgentSource.load().
    async fn load(&self) -> Result<AgentProfileContribution, AgentProfileSourceError> {
        let root_warn =
            |message: &str, error: Option<&crate::os::interface::host_fs_errors::HostFsError>| {
                self.log.warn(
                    message,
                    error.map(|error| {
                        LogPayload::Error(LogEntryError {
                            message: error.to_string(),
                            stack: None,
                        })
                    }),
                );
            };
        let roots = project_agent_roots(
            self.fs.0.as_ref(),
            &self.workspace.work_dir(),
            Some(&root_warn),
        )
        .await?;
        let discovery_warn = |message: &str, error: Option<&str>| {
            self.log.warn(
                message,
                error.map(|error| {
                    LogPayload::Error(LogEntryError {
                        message: error.into(),
                        stack: None,
                    })
                }),
            );
        };
        let discovery =
            discover_agent_files(self.fs.0.as_ref(), &roots, Some(&discovery_warn)).await?;
        let user = self.user.0.clone();
        Ok(profiles_from_discovery(
            discovery,
            Arc::new(move |context| user.get_default_profile().render_system_prompt(context)),
        ))
    }
}

#[derive(Clone)]
pub struct ProjectFileAgentSourceHandle(pub Arc<dyn AgentProfileSourceContract>);

impl Deref for ProjectFileAgentSourceHandle {
    type Target = dyn AgentProfileSourceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const PROJECT_FILE_AGENT_SOURCE_ID: ServiceIdentifier<ProjectFileAgentSourceHandle> =
    ServiceIdentifier::new("projectFileAgentSource");

pub fn register_project_file_agent_source() {
    register_scoped_service(
        LifecycleScope::Session,
        PROJECT_FILE_AGENT_SOURCE_ID,
        SyncDescriptor::new(|accessor| {
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let user = accessor.get(crate::app::agent_file_catalog::USER_FILE_AGENT_SOURCE_ID)?;
            let source: Arc<dyn AgentProfileSourceContract> =
                Arc::new(ProjectFileAgentSource::new(
                    (*workspace).clone(),
                    (*fs).clone(),
                    log.0.clone(),
                    (*user).clone(),
                ));
            Ok(ProjectFileAgentSourceHandle(source))
        }),
        InstantiationType::Eager,
        "sessionAgentProfileCatalog",
    );
}
