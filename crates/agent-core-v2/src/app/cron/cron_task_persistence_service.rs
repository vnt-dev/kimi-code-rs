//! Atomic-document implementation of cron task persistence.
//!
//! Original: `packages/agent-core-v2/src/app/cron/cronTaskPersistenceService.ts`.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use regex::Regex;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle, PersistenceScopeName},
    persistence::interface::atomic_document_store::{
        ATOMIC_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreHandle,
    },
};

use super::{
    CRON_TASK_PERSISTENCE_SERVICE_ID, CronTask, CronTaskPersistenceContract,
    CronTaskPersistenceHandle, CronTaskPersistenceResult, CronTaskQuery,
};

pub static CRON_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:[0-9a-f]{8}|[0-9a-hjkmnp-tv-z]{26})$").expect("valid cron id regex")
});
const JSON_SUFFIX: &str = ".json";

pub fn is_valid_cron_task(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(id) = object.get("id").and_then(serde_json::Value::as_str) else {
        return false;
    };
    CRON_ID_REGEX.is_match(id)
        && object.get("cron").is_some_and(serde_json::Value::is_string)
        && object
            .get("prompt")
            .is_some_and(serde_json::Value::is_string)
        && object
            .get("createdAt")
            .is_some_and(serde_json::Value::is_number)
        && object
            .get("recurring")
            .is_none_or(serde_json::Value::is_boolean)
        && object
            .get("lastFiredAt")
            .is_none_or(|value| value.as_f64().is_some_and(f64::is_finite))
        && object.get("tags").is_none_or(|tags| {
            tags.as_object()
                .is_some_and(|tags| tags.values().all(serde_json::Value::is_string))
        })
}

pub struct CronTaskPersistenceService {
    cron_scope: String,
    documents: AtomicDocumentStoreHandle,
}

impl CronTaskPersistenceService {
    pub fn new(bootstrap: BootstrapServiceHandle, documents: AtomicDocumentStoreHandle) -> Self {
        Self {
            cron_scope: bootstrap.scope(PersistenceScopeName::Cron).into(),
            documents,
        }
    }

    fn workspace_scope(&self, workspace_id: &str) -> String {
        format!("{}/{workspace_id}", self.cron_scope)
    }
}

#[async_trait]
impl CronTaskPersistenceContract for CronTaskPersistenceService {
    async fn get(
        &self,
        workspace_id: &str,
        task_id: &str,
    ) -> CronTaskPersistenceResult<Option<CronTask>> {
        let value = self
            .documents
            .0
            .get_value(
                &self.workspace_scope(workspace_id),
                &format!("{task_id}{JSON_SUFFIX}"),
            )
            .await?;
        match value.filter(is_valid_cron_task) {
            Some(value) => Ok(Some(serde_json::from_value(value)?)),
            None => Ok(None),
        }
    }

    async fn list(&self, query: CronTaskQuery) -> CronTaskPersistenceResult<Vec<CronTask>> {
        let scope = self.workspace_scope(&query.workspace_id);
        let keys = self.documents.list(&scope, None).await?;
        let mut tasks = Vec::new();
        for key in keys {
            let Some(id) = key.strip_suffix(JSON_SUFFIX) else {
                continue;
            };
            if !CRON_ID_REGEX.is_match(id) {
                continue;
            }
            if let Some(value) = self
                .documents
                .0
                .get_value(&scope, &key)
                .await?
                .filter(is_valid_cron_task)
            {
                tasks.push(serde_json::from_value(value)?);
            }
        }
        Ok(tasks)
    }

    async fn save(&self, workspace_id: &str, task: &CronTask) -> CronTaskPersistenceResult<()> {
        self.documents
            .set(
                &self.workspace_scope(workspace_id),
                &format!("{}{}", task.id, JSON_SUFFIX),
                task,
            )
            .await?;
        Ok(())
    }

    async fn delete(&self, workspace_id: &str, task_id: &str) -> CronTaskPersistenceResult<()> {
        self.documents
            .delete(
                &self.workspace_scope(workspace_id),
                &format!("{task_id}{JSON_SUFFIX}"),
            )
            .await?;
        Ok(())
    }
}

pub fn register_cron_task_persistence_service() {
    register_scoped_service(
        LifecycleScope::App,
        CRON_TASK_PERSISTENCE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service: Arc<dyn CronTaskPersistenceContract> =
                Arc::new(CronTaskPersistenceService::new(
                    (*accessor.get(BOOTSTRAP_SERVICE_ID)?).clone(),
                    (*accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?).clone(),
                ));
            Ok(CronTaskPersistenceHandle(service))
        }),
        InstantiationType::Eager,
        "cron",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_accepts_only_the_source_task_shape_and_id_formats() {
        assert!(is_valid_cron_task(
            &serde_json::json!({"id":"deadbeef","cron":"* * * * *","prompt":"x","createdAt":1})
        ));
        assert!(is_valid_cron_task(
            &serde_json::json!({"id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","cron":"* * * * *","prompt":"x","createdAt":1,"tags":{"sessionId":"s"}})
        ));
        for invalid in [
            serde_json::json!({"id":"bad","cron":"*","prompt":"x","createdAt":1}),
            serde_json::json!({"id":"deadbeef","cron":"*","prompt":1,"createdAt":1}),
            serde_json::json!({"id":"deadbeef","cron":"*","prompt":"x","createdAt":1,"tags":{"x":1}}),
        ] {
            assert!(!is_valid_cron_task(&invalid));
        }
    }
}
