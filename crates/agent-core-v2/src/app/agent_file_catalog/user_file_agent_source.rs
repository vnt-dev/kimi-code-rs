//! User agent-profile source backed by home-directory files.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/userFileAgentSource.ts`.

use std::{
    ops::Deref,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        log::{LOG_SERVICE_ID, LogEntryError, LogPayload, Logger},
    },
    app::{
        agent_profile_catalog::{
            AGENT_PROFILE_CATALOG_SERVICE_ID, AgentProfile, AgentProfileCatalogHandle,
            AgentProfileContext, AgentSystemPrompt, MissingDefaultAgentProfile,
        },
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
    },
    os::interface::host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
};

use super::{
    AGENT_PROFILE_SOURCE_PRIORITY_USER, AgentProfileContribution, AgentProfileSourceContract,
    AgentProfileSourceError, discover_agent_files, load_system_md_profile, profiles_from_discovery,
    user_agent_roots,
};

pub struct UserFileAgentSource {
    bootstrap: BootstrapServiceHandle,
    fs: HostFileSystemServiceHandle,
    log: Arc<dyn Logger>,
    builtin: AgentProfileCatalogHandle,
    // The source refreshes this on every load. The prompt closure passed to
    // other sources owns this lock, rather than a stale profile snapshot.
    default_profile: Arc<RwLock<Arc<AgentProfile>>>,
}

impl UserFileAgentSource {
    // Original: UserFileAgentSource.constructor().
    pub fn new(
        bootstrap: BootstrapServiceHandle,
        fs: HostFileSystemServiceHandle,
        log: Arc<dyn Logger>,
        builtin: AgentProfileCatalogHandle,
    ) -> Result<Self, MissingDefaultAgentProfile> {
        let default_profile = builtin.get_default()?;
        Ok(Self {
            bootstrap,
            fs,
            log,
            builtin,
            default_profile: Arc::new(RwLock::new(default_profile)),
        })
    }

    // Original: UserFileAgentSource.getDefaultProfile().
    pub fn get_default_profile(&self) -> Arc<AgentProfile> {
        Arc::clone(&self.default_profile.read().unwrap())
    }
}

#[async_trait]
impl AgentProfileSourceContract for UserFileAgentSource {
    fn id(&self) -> &str {
        "user"
    }

    fn priority(&self) -> i32 {
        AGENT_PROFILE_SOURCE_PRIORITY_USER
    }

    // Original: UserFileAgentSource.load().
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
        let roots = user_agent_roots(
            self.fs.0.as_ref(),
            self.bootstrap.home_dir(),
            self.bootstrap.os_home_dir(),
            Some(&root_warn),
        )
        .await?;

        let builtin_default = self.builtin.get_default()?;
        let system_warn = |message: &str| self.log.warn(message, None);
        let system_md = load_system_md_profile(
            self.fs.0.as_ref(),
            self.bootstrap.home_dir(),
            Arc::clone(&builtin_default),
            &system_warn,
        )
        .await?;
        *self.default_profile.write().unwrap() = system_md
            .as_ref()
            .map(Arc::clone)
            .unwrap_or(builtin_default);

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
        let default_profile = Arc::clone(&self.default_profile);
        let base_prompt: AgentSystemPrompt = Arc::new(move |context: &AgentProfileContext| {
            default_profile
                .read()
                .unwrap()
                .render_system_prompt(context)
        });
        let mut contribution = profiles_from_discovery(discovery, base_prompt);
        if let Some(system_md) = system_md {
            contribution.profiles.push(system_md);
        }
        Ok(contribution)
    }
}

#[derive(Clone)]
pub struct UserFileAgentSourceHandle(pub Arc<UserFileAgentSource>);

impl Deref for UserFileAgentSourceHandle {
    type Target = dyn AgentProfileSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const USER_FILE_AGENT_SOURCE_ID: ServiceIdentifier<UserFileAgentSourceHandle> =
    ServiceIdentifier::new("userFileAgentSource");

pub fn register_user_file_agent_source() {
    register_scoped_service(
        LifecycleScope::App,
        USER_FILE_AGENT_SOURCE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let builtin = accessor.get(AGENT_PROFILE_CATALOG_SERVICE_ID)?;
            let source = UserFileAgentSource::new(
                (*bootstrap).clone(),
                (*fs).clone(),
                log.0.clone(),
                (*builtin).clone(),
            )
            .map_err(|error| DiError::Factory(error.to_string()))?;
            Ok(UserFileAgentSourceHandle(Arc::new(source)))
        }),
        InstantiationType::Eager,
        "agentFileCatalog",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        _base::log::{LogContext, LogPayload},
        app::{
            agent_file_catalog::SYSTEM_MD_FILENAME,
            agent_profile_catalog::{AgentProfileCatalogContract, AgentSystemPrompt},
            bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
        },
        os::backends::node_local::host_fs_service::HostFileSystem,
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

    fn builtin_default() -> Arc<AgentProfile> {
        let prompt: AgentSystemPrompt = Arc::new(|_| "BUILTIN".into());
        Arc::new(AgentProfile {
            name: "agent".into(),
            description: Some("builtin".into()),
            when_to_use: None,
            is_override: Some(false),
            tools: None,
            disallowed_tools: None,
            subagents: None,
            model: None,
            system_prompt: prompt,
            prompt_prefix: None,
            summary_policy: None,
        })
    }

    #[tokio::test]
    async fn load_updates_default_prompt_and_appends_system_profile_after_discovery() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kimi-user-agent-source-{}-{nonce}",
            std::process::id()
        ));
        let home = root.join("kimi");
        let os_home = root.join("os-home");
        tokio::fs::create_dir_all(home.join("agents"))
            .await
            .unwrap();
        tokio::fs::write(
            home.join("agents/review.md"),
            "---\nname: review\ndescription: review files\n---\nReview\n",
        )
        .await
        .unwrap();
        tokio::fs::write(home.join(SYSTEM_MD_FILENAME), "SYSTEM ${base_prompt}")
            .await
            .unwrap();

        let bootstrap: Arc<dyn BootstrapServiceContract> =
            Arc::new(BootstrapService::new(BootstrapOptions {
                home_dir: home.clone(),
                config_path: home.join("config.toml"),
                os_home_dir: os_home,
                platform: "linux".into(),
                arch: "x64".into(),
                cwd: root.clone(),
                env: HashMap::new(),
                client_version: "test".into(),
            }));
        let source = UserFileAgentSource::new(
            BootstrapServiceHandle(bootstrap),
            HostFileSystemServiceHandle(Arc::new(HostFileSystem)),
            Arc::new(SilentLogger),
            AgentProfileCatalogHandle(Arc::new(StaticCatalog(builtin_default()))),
        )
        .unwrap();

        let contribution = source.load().await.unwrap();
        assert_eq!(
            contribution
                .profiles
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            ["review", "agent"]
        );
        assert_eq!(
            source
                .get_default_profile()
                .render_system_prompt(&AgentProfileContext::default()),
            "SYSTEM BUILTIN"
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
