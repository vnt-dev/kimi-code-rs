//! Frozen startup snapshot service and semantic persistence scopes.
//!
//! Original: `packages/agent-core-v2/src/app/bootstrap/bootstrapService.ts`.

use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::_base::di::{
    descriptors::SyncDescriptor,
    instantiation::ServiceIdentifier,
    instantiation::ServicesAccessorExt,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

use super::options::{BOOTSTRAP_OPTIONS_ID, BootstrapOptions};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PersistenceScopeName {
    Config,
    Sessions,
    Blobs,
    Store,
    Logs,
    Cache,
    Credentials,
    Cron,
}

pub trait BootstrapServiceContract: Send + Sync {
    fn platform(&self) -> &str;
    fn arch(&self) -> &str;
    fn cwd(&self) -> &Path;
    fn os_home_dir(&self) -> &Path;
    fn home_dir(&self) -> &Path;
    fn config_path(&self) -> &Path;
    fn client_version(&self) -> &str;
    fn sessions_dir(&self) -> &Path;
    fn blobs_dir(&self) -> &Path;
    fn store_dir(&self) -> &Path;
    fn cache_dir(&self) -> &Path;
    fn logs_dir(&self) -> &Path;
    fn config_key(&self) -> &str;
    fn get_env(&self, name: &str) -> Option<&str>;
    fn scope(&self, name: PersistenceScopeName) -> &str;
    fn session_scope(&self, workspace_id: &str, session_id: &str) -> String;
    fn agent_scope(&self, workspace_id: &str, session_id: &str, agent_id: &str) -> String;
    fn session_dir(&self, workspace_id: &str, session_id: &str) -> PathBuf;
    fn agent_homedir(&self, workspace_id: &str, session_id: &str, agent_id: &str) -> PathBuf;
}

#[derive(Clone)]
pub struct BootstrapServiceHandle(pub Arc<dyn BootstrapServiceContract>);

impl Deref for BootstrapServiceHandle {
    type Target = dyn BootstrapServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const BOOTSTRAP_SERVICE_ID: ServiceIdentifier<BootstrapServiceHandle> =
    ServiceIdentifier::new("bootstrapService");

pub struct BootstrapService {
    options: BootstrapOptions,
    sessions_dir: PathBuf,
    blobs_dir: PathBuf,
    store_dir: PathBuf,
    cache_dir: PathBuf,
    logs_dir: PathBuf,
    config_key: String,
}

impl BootstrapService {
    // Original: BootstrapService.constructor().
    pub fn new(options: BootstrapOptions) -> Self {
        let sessions_dir = options.home_dir.join("sessions");
        let blobs_dir = options.home_dir.join("blobs");
        let store_dir = options.home_dir.join("store");
        let cache_dir = options.home_dir.join("cache");
        let logs_dir = options.home_dir.join("logs");
        let config_key = options
            .config_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            options,
            sessions_dir,
            blobs_dir,
            store_dir,
            cache_dir,
            logs_dir,
            config_key,
        }
    }
}

impl BootstrapServiceContract for BootstrapService {
    fn platform(&self) -> &str {
        &self.options.platform
    }

    fn arch(&self) -> &str {
        &self.options.arch
    }

    fn cwd(&self) -> &Path {
        &self.options.cwd
    }

    fn os_home_dir(&self) -> &Path {
        &self.options.os_home_dir
    }

    fn home_dir(&self) -> &Path {
        &self.options.home_dir
    }

    fn config_path(&self) -> &Path {
        &self.options.config_path
    }

    fn client_version(&self) -> &str {
        &self.options.client_version
    }

    fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    fn blobs_dir(&self) -> &Path {
        &self.blobs_dir
    }

    fn store_dir(&self) -> &Path {
        &self.store_dir
    }

    fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    fn logs_dir(&self) -> &Path {
        &self.logs_dir
    }

    fn config_key(&self) -> &str {
        &self.config_key
    }

    // Original: BootstrapService.getEnv().
    fn get_env(&self, name: &str) -> Option<&str> {
        self.options.env.get(name).map(String::as_str)
    }

    // Original: BootstrapService.scope().
    fn scope(&self, name: PersistenceScopeName) -> &str {
        match name {
            PersistenceScopeName::Config => "",
            PersistenceScopeName::Sessions => "sessions",
            PersistenceScopeName::Blobs => "blobs",
            PersistenceScopeName::Store => "store",
            PersistenceScopeName::Logs => "logs",
            PersistenceScopeName::Cache => "cache",
            PersistenceScopeName::Credentials => "credentials",
            PersistenceScopeName::Cron => "cron",
        }
    }

    // Original: BootstrapService.sessionScope(). Scope keys intentionally use
    // forward slashes on every platform, matching `pathe.join`.
    fn session_scope(&self, workspace_id: &str, session_id: &str) -> String {
        join_scope(&[
            self.scope(PersistenceScopeName::Sessions),
            workspace_id,
            session_id,
        ])
    }

    // Original: BootstrapService.agentScope().
    fn agent_scope(&self, workspace_id: &str, session_id: &str, agent_id: &str) -> String {
        join_scope(&[
            &self.session_scope(workspace_id, session_id),
            "agents",
            agent_id,
        ])
    }

    // Original: BootstrapService.sessionDir().
    fn session_dir(&self, workspace_id: &str, session_id: &str) -> PathBuf {
        self.options
            .home_dir
            .join(self.session_scope(workspace_id, session_id))
    }

    // Original: BootstrapService.agentHomedir().
    fn agent_homedir(&self, workspace_id: &str, session_id: &str, agent_id: &str) -> PathBuf {
        self.options
            .home_dir
            .join(self.agent_scope(workspace_id, session_id, agent_id))
    }
}

fn join_scope(parts: &[&str]) -> String {
    parts
        .iter()
        .flat_map(|part| part.split('/'))
        .filter(|part| !part.is_empty() && *part != ".")
        .fold(Vec::<&str>::new(), |mut output, part| {
            if part == ".." {
                output.pop();
            } else {
                output.push(part);
            }
            output
        })
        .join("/")
}

// Original: registerScopedService(... BootstrapService ...).
pub fn register_bootstrap_service() {
    register_scoped_service(
        LifecycleScope::App,
        BOOTSTRAP_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let options = accessor.get(BOOTSTRAP_OPTIONS_ID)?;
            let service: Arc<dyn BootstrapServiceContract> =
                Arc::new(BootstrapService::new((*options).clone()));
            Ok(BootstrapServiceHandle(service))
        }),
        InstantiationType::Eager,
        "bootstrap",
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn service() -> BootstrapService {
        BootstrapService::new(BootstrapOptions {
            home_dir: "/tmp/kimi-home".into(),
            config_path: "/tmp/kimi-home/config.toml".into(),
            os_home_dir: "/home/test".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            cwd: "/tmp".into(),
            env: HashMap::from([("FOO".into(), "bar".into())]),
            client_version: "0.0.0-test".into(),
        })
    }

    #[test]
    fn constructor_materializes_paths_and_frozen_environment() {
        let service = service();
        assert_eq!(service.home_dir(), Path::new("/tmp/kimi-home"));
        assert_eq!(service.sessions_dir(), Path::new("/tmp/kimi-home/sessions"));
        assert_eq!(service.config_key(), "config.toml");
        assert_eq!(service.get_env("FOO"), Some("bar"));
        assert_eq!(service.get_env("MISSING"), None);
    }

    #[test]
    fn maps_semantic_session_and_agent_scopes() {
        let service = service();
        assert_eq!(service.scope(PersistenceScopeName::Config), "");
        assert_eq!(
            service.session_scope("workspace", "session"),
            "sessions/workspace/session"
        );
        assert_eq!(
            service.agent_scope("workspace", "session", "agent"),
            "sessions/workspace/session/agents/agent"
        );
        assert_eq!(
            service.agent_homedir("workspace", "session", "agent"),
            Path::new("/tmp/kimi-home/sessions/workspace/session/agents/agent")
        );
    }
}
