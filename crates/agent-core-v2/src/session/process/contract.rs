//! Session process runner contract.
//!
//! Original: `packages/agent-core-v2/src/session/process/processRunner.ts`.

use std::{collections::HashMap, error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::instantiation::ServiceIdentifier, os::interface::host_process::HostProcess,
};

/// The session-level process handle is behaviorally identical to the host
/// process handle; the type alias keeps the original domain name visible.
pub type SessionProcess = Arc<dyn HostProcess>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessExecOptions {
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

pub type SessionProcessRunnerError = Box<dyn Error + Send + Sync>;
pub type SessionProcessRunnerResult<T> = Result<T, SessionProcessRunnerError>;

#[async_trait]
pub trait SessionProcessRunnerContract: Send + Sync {
    async fn exec(
        &self,
        args: &[String],
        options: Option<ProcessExecOptions>,
    ) -> SessionProcessRunnerResult<SessionProcess>;
}

#[derive(Clone)]
pub struct SessionProcessRunnerHandle(pub Arc<dyn SessionProcessRunnerContract>);

impl Deref for SessionProcessRunnerHandle {
    type Target = dyn SessionProcessRunnerContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_PROCESS_RUNNER_SERVICE_ID: ServiceIdentifier<SessionProcessRunnerHandle> =
    ServiceIdentifier::new("sessionProcessRunner");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_to_inherited_environment_and_seeded_cwd() {
        assert_eq!(
            ProcessExecOptions::default(),
            ProcessExecOptions {
                cwd: None,
                env: None,
            }
        );
        assert_eq!(
            SESSION_PROCESS_RUNNER_SERVICE_ID.to_string(),
            "sessionProcessRunner"
        );
    }
}
