//! Project-local `.kimi-code/local.toml` access contract.
//!
//! Original:
//! `packages/agent-core-v2/src/app/workspaceLocalConfig/workspaceLocalConfig.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::di::instantiation::ServiceIdentifier;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceAdditionalDirsLoadResult {
    pub project_root: String,
    pub config_path: String,
    pub additional_dirs: Vec<String>,
}

pub type WorkspaceLocalConfigError = Box<dyn Error + Send + Sync>;
pub type WorkspaceLocalConfigResult<T> = Result<T, WorkspaceLocalConfigError>;

#[async_trait]
pub trait WorkspaceLocalConfigServiceContract: Send + Sync {
    async fn read_additional_dirs(
        &self,
        work_dir: &str,
    ) -> WorkspaceLocalConfigResult<WorkspaceAdditionalDirsLoadResult>;

    async fn resolve_additional_dirs(
        &self,
        base_dir: &str,
        additional_dirs: &[String],
    ) -> WorkspaceLocalConfigResult<Vec<String>>;

    async fn append_additional_dir(
        &self,
        work_dir: &str,
        input_path: &str,
    ) -> WorkspaceLocalConfigResult<WorkspaceAdditionalDirsLoadResult>;
}

#[derive(Clone)]
pub struct WorkspaceLocalConfigServiceHandle(pub Arc<dyn WorkspaceLocalConfigServiceContract>);

impl Deref for WorkspaceLocalConfigServiceHandle {
    type Target = dyn WorkspaceLocalConfigServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const WORKSPACE_LOCAL_CONFIG_SERVICE_ID: ServiceIdentifier<WorkspaceLocalConfigServiceHandle> =
    ServiceIdentifier::new("workspaceLocalConfigService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_shape_and_service_id_match_source_contract() {
        let result = WorkspaceAdditionalDirsLoadResult {
            project_root: "/repo".into(),
            config_path: "/repo/.kimi-code/local.toml".into(),
            additional_dirs: vec!["/shared".into()],
        };
        assert_eq!(result.additional_dirs, ["/shared"]);
        assert_eq!(
            WORKSPACE_LOCAL_CONFIG_SERVICE_ID.to_string(),
            "workspaceLocalConfigService"
        );
    }
}
