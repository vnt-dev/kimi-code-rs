//! Project-scoped cron task persistence contract.
//!
//! Original: `packages/agent-core-v2/src/app/cron/cronTaskPersistence.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::di::instantiation::ServiceIdentifier;

use super::CronTask;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CronTaskQuery {
    pub workspace_id: String,
}

pub type CronTaskPersistenceResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[async_trait]
pub trait CronTaskPersistenceContract: Send + Sync {
    async fn get(
        &self,
        workspace_id: &str,
        task_id: &str,
    ) -> CronTaskPersistenceResult<Option<CronTask>>;
    async fn list(&self, query: CronTaskQuery) -> CronTaskPersistenceResult<Vec<CronTask>>;
    async fn save(&self, workspace_id: &str, task: &CronTask) -> CronTaskPersistenceResult<()>;
    async fn delete(&self, workspace_id: &str, task_id: &str) -> CronTaskPersistenceResult<()>;
}

#[derive(Clone)]
pub struct CronTaskPersistenceHandle(pub Arc<dyn CronTaskPersistenceContract>);

impl Deref for CronTaskPersistenceHandle {
    type Target = dyn CronTaskPersistenceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const CRON_TASK_PERSISTENCE_SERVICE_ID: ServiceIdentifier<CronTaskPersistenceHandle> =
    ServiceIdentifier::new("cronTaskPersistence");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_service_identity_preserve_the_typescript_contract() {
        assert_eq!(
            CronTaskQuery {
                workspace_id: "wd-1".into()
            }
            .workspace_id,
            "wd-1"
        );
        assert_eq!(
            CRON_TASK_PERSISTENCE_SERVICE_ID.to_string(),
            "cronTaskPersistence"
        );
    }
}
