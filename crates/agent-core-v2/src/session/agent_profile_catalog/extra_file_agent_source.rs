//! Configured extra-directory agent-file profile source.
//!
//! Original: `packages/agent-core-v2/src/session/sessionAgentProfileCatalog/extraFileAgentSource.ts`.

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
        log::{LOG_SERVICE_ID, LogEntryError, LogPayload, Logger},
    },
    app::{
        agent_file_catalog::{
            AGENT_PROFILE_SOURCE_PRIORITY_EXTRA, AgentProfileContribution,
            AgentProfileSourceContract, AgentProfileSourceError, EXTRA_AGENT_DIRS_SECTION,
            UserFileAgentSourceHandle, configured_agent_roots, discover_agent_files,
            profiles_from_discovery,
        },
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    },
    os::interface::host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
    session::workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
};

pub struct ExtraFileAgentSource {
    config: ConfigServiceHandle,
    workspace: SessionWorkspaceContextHandle,
    bootstrap: BootstrapServiceHandle,
    fs: HostFileSystemServiceHandle,
    log: Arc<dyn Logger>,
    user: UserFileAgentSourceHandle,
    on_did_change_emitter: Arc<Emitter<()>>,
    disposables: DisposableStore,
}

impl ExtraFileAgentSource {
    // Original: ExtraFileAgentSource.constructor().
    pub fn new(
        config: ConfigServiceHandle,
        workspace: SessionWorkspaceContextHandle,
        bootstrap: BootstrapServiceHandle,
        fs: HostFileSystemServiceHandle,
        log: Arc<dyn Logger>,
        user: UserFileAgentSourceHandle,
    ) -> Arc<Self> {
        let emitter = Arc::new(Emitter::new());
        let source = Arc::new(Self {
            config: config.clone(),
            workspace,
            bootstrap,
            fs,
            log,
            user,
            on_did_change_emitter: Arc::clone(&emitter),
            disposables: DisposableStore::new(),
        });
        source.disposables.add(emitter);
        let weak = Arc::downgrade(&source);
        let subscription = config.on_did_section_change().subscribe(move |event| {
            if event.domain == EXTRA_AGENT_DIRS_SECTION
                && let Some(source) = weak.upgrade()
            {
                source.on_did_change_emitter.fire(&());
            }
        });
        source.disposables.add(subscription);
        source
    }
}

#[async_trait]
impl AgentProfileSourceContract for ExtraFileAgentSource {
    fn id(&self) -> &str {
        "extra"
    }
    fn priority(&self) -> i32 {
        AGENT_PROFILE_SOURCE_PRIORITY_EXTRA
    }
    fn on_did_change(&self) -> Option<Event<()>> {
        Some(self.on_did_change_emitter.event())
    }

    // Original: ExtraFileAgentSource.load().
    async fn load(&self) -> Result<AgentProfileContribution, AgentProfileSourceError> {
        self.config.ready().await?;
        let dirs = self
            .config
            .get(EXTRA_AGENT_DIRS_SECTION)
            .and_then(|value| value.as_array().cloned())
            .map(|values| {
                values
                    .into_iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
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
        let roots = configured_agent_roots(
            self.fs.0.as_ref(),
            &dirs,
            &self.workspace.work_dir(),
            self.bootstrap.os_home_dir(),
            crate::app::agent_file_catalog::AgentFileSource::Extra,
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

impl Disposable for ExtraFileAgentSource {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

#[derive(Clone)]
pub struct ExtraFileAgentSourceHandle(pub Arc<ExtraFileAgentSource>);

impl Deref for ExtraFileAgentSourceHandle {
    type Target = dyn AgentProfileSourceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for ExtraFileAgentSourceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const EXTRA_FILE_AGENT_SOURCE_ID: ServiceIdentifier<ExtraFileAgentSourceHandle> =
    ServiceIdentifier::new("extraFileAgentSource");

pub fn register_extra_file_agent_source() {
    register_scoped_service(
        LifecycleScope::Session,
        EXTRA_FILE_AGENT_SOURCE_ID,
        SyncDescriptor::new(|accessor| {
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let workspace = accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?;
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let user = accessor.get(crate::app::agent_file_catalog::USER_FILE_AGENT_SOURCE_ID)?;
            Ok(ExtraFileAgentSourceHandle(ExtraFileAgentSource::new(
                (*config).clone(),
                (*workspace).clone(),
                (*bootstrap).clone(),
                (*fs).clone(),
                log.0.clone(),
                (*user).clone(),
            )))
        })
        .disposable(),
        InstantiationType::Eager,
        "sessionAgentProfileCatalog",
    );
}
