//! Lazily initialized host environment snapshot and login-shell PATH overlay.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/hostEnvironmentService.ts`.
//!
//! Rust adaptation: Rust 2024 makes mutation of the process-global environment
//! unsafe in a multithreaded program. The enriched environment is therefore an
//! immutable snapshot exposed to this package's process factories.

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use tokio::sync::OnceCell;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::BugIndicatingError,
        exec_env::{
            environment_probe::{
                ExecFileText, HostEnvironmentInfo, HostEnvironmentProbeError, PathClass, ShellName,
                exec_file_text, probe_host_environment_from_node,
            },
            login_shell_path::apply_login_shell_path,
        },
    },
    os::interface::host_environment::{
        HOST_ENVIRONMENT_SERVICE_ID, HostEnvironment, HostEnvironmentHandle,
    },
};

#[derive(Clone)]
struct InitializedEnvironment {
    info: HostEnvironmentInfo,
    process_environment: Arc<HashMap<String, String>>,
}

#[derive(Default)]
pub struct LocalHostEnvironmentService {
    initialized: OnceCell<Result<InitializedEnvironment, HostEnvironmentProbeError>>,
}

impl LocalHostEnvironmentService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_environment(&self) -> Result<Arc<HashMap<String, String>>, BugIndicatingError> {
        self.require("processEnvironment")
            .map(|initialized| Arc::clone(&initialized.process_environment))
    }

    fn require(&self, field: &str) -> Result<&InitializedEnvironment, BugIndicatingError> {
        match self.initialized.get() {
            Some(Ok(initialized)) => Ok(initialized),
            _ => Err(BugIndicatingError::new(Some(&format!(
                "IHostEnvironment.{field} accessed before ready — await IHostEnvironment.ready first (composition root should do so before creating a Session scope)."
            )))),
        }
    }
}

/// Original: app-scope host environment service registration.
pub fn register_local_host_environment_service() {
    register_scoped_service(
        LifecycleScope::App,
        HOST_ENVIRONMENT_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn HostEnvironment> = Arc::new(LocalHostEnvironmentService::new());
            Ok(HostEnvironmentHandle(service))
        }),
        InstantiationType::Eager,
        "hostEnvironment",
    );
}

#[async_trait]
impl HostEnvironment for LocalHostEnvironmentService {
    async fn ready(&self) -> Result<(), HostEnvironmentProbeError> {
        self.initialized
            .get_or_init(|| async {
                let (info, process_environment) = tokio::join!(
                    probe_host_environment_from_node(),
                    enriched_process_environment()
                );
                info.map(|info| InitializedEnvironment {
                    info,
                    process_environment: Arc::new(process_environment),
                })
            })
            .await
            .as_ref()
            .map(|_| ())
            .map_err(Clone::clone)
    }

    fn info(&self) -> Result<HostEnvironmentInfo, BugIndicatingError> {
        Ok(self.require("info")?.info.clone())
    }

    fn os_kind(&self) -> Result<String, BugIndicatingError> {
        Ok(self.require("osKind")?.info.os_kind.clone())
    }

    fn os_arch(&self) -> Result<String, BugIndicatingError> {
        Ok(self.require("osArch")?.info.os_arch.clone())
    }

    fn os_version(&self) -> Result<String, BugIndicatingError> {
        Ok(self.require("osVersion")?.info.os_version.clone())
    }

    fn shell_name(&self) -> Result<ShellName, BugIndicatingError> {
        Ok(self.require("shellName")?.info.shell_name)
    }

    fn shell_path(&self) -> Result<String, BugIndicatingError> {
        Ok(self.require("shellPath")?.info.shell_path.clone())
    }

    fn path_class(&self) -> Result<PathClass, BugIndicatingError> {
        Ok(self.require("pathClass")?.info.path_class)
    }

    fn home_dir(&self) -> Result<String, BugIndicatingError> {
        Ok(self.require("homeDir")?.info.home_dir.clone())
    }
}

async fn enriched_process_environment() -> HashMap<String, String> {
    let mut environment = std::env::vars().collect::<HashMap<_, _>>();
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        other => other,
    };
    let user_shell = tokio::task::spawn_blocking(account_shell)
        .await
        .ok()
        .flatten();
    let exec: ExecFileText = Arc::new(|file, args, timeout| {
        Box::pin(async move { exec_file_text(file, &args, timeout).await })
    });
    apply_login_shell_path(platform, &mut environment, || user_shell, &exec).await;
    environment
}

#[cfg(unix)]
fn account_shell() -> Option<String> {
    use uzers::os::unix::UserExt;

    uzers::get_user_by_uid(uzers::get_current_uid())
        .map(|user| user.shell().to_string_lossy().into_owned())
        .filter(|shell| !shell.is_empty())
}

#[cfg(not(unix))]
fn account_shell() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_early_reads_then_memoizes_probe_and_environment() {
        let service = LocalHostEnvironmentService::new();
        let error = service.shell_path().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("IHostEnvironment.shellPath accessed before ready")
        );
        service.ready().await.unwrap();
        let first = service.info().unwrap();
        service.ready().await.unwrap();
        assert_eq!(service.info().unwrap(), first);
        assert!(!service.shell_path().unwrap().is_empty());
        let environment = service.process_environment().unwrap();
        for (key, value) in std::env::vars() {
            if key != "PATH" {
                assert_eq!(environment.get(&key), Some(&value));
            }
        }
    }
}
