//! Immutable host OS, shell, path-style and home-directory facts.
//!
//! Original: `packages/agent-core-v2/src/os/interface/hostEnvironment.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::{
    di::instantiation::ServiceIdentifier, errors::errors::BugIndicatingError,
    exec_env::environment_probe::HostEnvironmentProbeError,
};

pub use crate::_base::exec_env::environment_probe::{HostEnvironmentInfo, PathClass, ShellName};

#[async_trait]
pub trait HostEnvironment: Send + Sync {
    async fn ready(&self) -> Result<(), HostEnvironmentProbeError>;
    fn info(&self) -> Result<HostEnvironmentInfo, BugIndicatingError>;

    fn os_kind(&self) -> Result<String, BugIndicatingError> {
        Ok(self.info()?.os_kind)
    }
    fn os_arch(&self) -> Result<String, BugIndicatingError> {
        Ok(self.info()?.os_arch)
    }
    fn os_version(&self) -> Result<String, BugIndicatingError> {
        Ok(self.info()?.os_version)
    }
    fn shell_name(&self) -> Result<ShellName, BugIndicatingError> {
        Ok(self.info()?.shell_name)
    }
    fn shell_path(&self) -> Result<String, BugIndicatingError> {
        Ok(self.info()?.shell_path)
    }
    fn path_class(&self) -> Result<PathClass, BugIndicatingError> {
        Ok(self.info()?.path_class)
    }
    fn home_dir(&self) -> Result<String, BugIndicatingError> {
        Ok(self.info()?.home_dir)
    }
}

#[derive(Clone)]
pub struct HostEnvironmentHandle(pub Arc<dyn HostEnvironment>);

impl Deref for HostEnvironmentHandle {
    type Target = dyn HostEnvironment;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const HOST_ENVIRONMENT_SERVICE_ID: ServiceIdentifier<HostEnvironmentHandle> =
    ServiceIdentifier::new("hostEnvironment");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_matches_source() {
        assert_eq!(HOST_ENVIRONMENT_SERVICE_ID.to_string(), "hostEnvironment");
    }
}
