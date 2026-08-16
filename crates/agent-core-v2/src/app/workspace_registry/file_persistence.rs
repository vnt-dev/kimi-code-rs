//! V1-compatible `workspaces.json` persistence adapter.
//!
//! Original:
//! `packages/agent-core-v2/src/app/workspaceRegistry/fileWorkspacePersistence.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde_json::{Map, Value};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    _base::utils::iso_date_time::now_millis,
    persistence::interface::atomic_document_store::{
        ATOMIC_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreService,
    },
};

use super::{
    contract::Workspace,
    persistence::{
        PersistedWorkspaceEntry, WORKSPACE_PERSISTENCE_SERVICE_ID, WorkspaceCatalog,
        WorkspacePersistenceContract, WorkspacePersistenceHandle, WorkspacePersistenceResult,
    },
};

const WORKSPACE_REGISTRY_VERSION: i64 = 1;
const WORKSPACE_REGISTRY_SCOPE: &str = "";
const WORKSPACE_REGISTRY_KEY: &str = "workspaces.json";

pub struct FileWorkspacePersistence {
    documents: Arc<dyn AtomicDocumentStoreService>,
    now_millis: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl FileWorkspacePersistence {
    pub fn new(documents: Arc<dyn AtomicDocumentStoreService>) -> Self {
        Self::with_clock(documents, Arc::new(current_time_millis))
    }

    fn with_clock(
        documents: Arc<dyn AtomicDocumentStoreService>,
        now_millis: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            documents,
            now_millis,
        }
    }
}

#[async_trait]
impl WorkspacePersistenceContract for FileWorkspacePersistence {
    // Original: FileWorkspacePersistence.load(). The document is inspected as
    // untyped JSON so dirty optional fields and individual invalid entries do
    // not invalidate an otherwise usable catalog.
    async fn load(&self) -> WorkspacePersistenceResult<Option<WorkspaceCatalog>> {
        let Some(file) = self
            .documents
            .get_value(WORKSPACE_REGISTRY_SCOPE, WORKSPACE_REGISTRY_KEY)
            .await?
        else {
            return Ok(None);
        };
        let Value::Object(file) = file else {
            return Ok(None);
        };
        let Some(workspaces) = file.get("workspaces").and_then(object_entries) else {
            return Ok(None);
        };

        let now = (self.now_millis)();
        let mut loaded = Vec::new();
        for (id, raw) in workspaces {
            let Some(entry) = sanitize_entry(raw) else {
                continue;
            };
            loaded.push(Workspace {
                id,
                root: entry.root,
                name: entry.name,
                created_at_millis: parse_time(&entry.created_at, now),
                last_opened_at_millis: parse_time(&entry.last_opened_at, now),
            });
        }
        let deleted_ids = file
            .get("deleted_workspace_ids")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Some(WorkspaceCatalog {
            workspaces: loaded,
            deleted_ids,
        }))
    }

    // Original: FileWorkspacePersistence.save().
    async fn save(&self, catalog: &WorkspaceCatalog) -> WorkspacePersistenceResult<()> {
        let mut workspaces = Map::new();
        for workspace in &catalog.workspaces {
            workspaces.insert(
                workspace.id.clone(),
                serde_json::to_value(PersistedWorkspaceEntry {
                    root: workspace.root.clone(),
                    name: workspace.name.clone(),
                    created_at: timestamp_to_iso(workspace.created_at_millis)?,
                    last_opened_at: timestamp_to_iso(workspace.last_opened_at_millis)?,
                })?,
            );
        }
        let file = Value::Object(Map::from_iter([
            ("version".into(), Value::from(WORKSPACE_REGISTRY_VERSION)),
            ("workspaces".into(), Value::Object(workspaces)),
            (
                "deleted_workspace_ids".into(),
                Value::Array(
                    catalog
                        .deleted_ids
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
        ]));
        self.documents
            .set_value(WORKSPACE_REGISTRY_SCOPE, WORKSPACE_REGISTRY_KEY, file)
            .await?;
        Ok(())
    }
}

fn object_entries(value: &Value) -> Option<Vec<(String, &Value)>> {
    match value {
        Value::Object(entries) => Some(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), value))
                .collect(),
        ),
        // JavaScript `typeof [] === 'object'`; Object.entries(array) yields
        // numeric string keys, so preserve that unusual accepted shape.
        Value::Array(entries) => Some(
            entries
                .iter()
                .enumerate()
                .map(|(index, value)| (index.to_string(), value))
                .collect(),
        ),
        _ => None,
    }
}

// Original: sanitizeEntry(). Empty strings and non-ISO date strings remain
// accepted here; parseTime applies the original fallback later.
fn sanitize_entry(value: &Value) -> Option<PersistedWorkspaceEntry> {
    let value = value.as_object()?;
    Some(PersistedWorkspaceEntry {
        root: value.get("root")?.as_str()?.into(),
        name: value.get("name")?.as_str()?.into(),
        created_at: value.get("created_at")?.as_str()?.into(),
        last_opened_at: value.get("last_opened_at")?.as_str()?.into(),
    })
}

// Original: parseTime(). Generated files use RFC 3339; the extra date-only
// and RFC 2822 branches cover other stable Date.parse inputs used by legacy
// files before falling back to the single load-time `now` value.
fn parse_time(value: &str, fallback: i64) -> i64 {
    if let Ok(date) = DateTime::parse_from_rfc3339(value) {
        return date.timestamp_millis();
    }
    if let Ok(date) = DateTime::parse_from_rfc2822(value) {
        return date.timestamp_millis();
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        && let Some(date) = date.and_hms_opt(0, 0, 0)
    {
        return DateTime::<Utc>::from_naive_utc_and_offset(date, Utc).timestamp_millis();
    }
    fallback
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid time value")]
struct InvalidWorkspaceTimestamp;

fn timestamp_to_iso(millis: i64) -> Result<String, InvalidWorkspaceTimestamp> {
    DateTime::<Utc>::from_timestamp_millis(millis)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or(InvalidWorkspaceTimestamp)
}

fn current_time_millis() -> i64 {
    now_millis()
}

pub fn register_workspace_persistence() {
    register_scoped_service(
        LifecycleScope::App,
        WORKSPACE_PERSISTENCE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let documents = accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?;
            let service: Arc<dyn WorkspacePersistenceContract> =
                Arc::new(FileWorkspacePersistence::new(Arc::clone(&documents.0)));
            Ok(WorkspacePersistenceHandle(service))
        }),
        InstantiationType::Eager,
        "workspaceRegistry",
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use crate::{
        _base::{di::lifecycle::DisposableHandle, event::Event},
        persistence::interface::{
            atomic_document_store::AtomicDocumentStoreService, storage::StorageError,
        },
    };

    use super::*;

    #[derive(Default)]
    struct StubDocuments {
        value: Mutex<Option<Value>>,
    }

    #[async_trait]
    impl AtomicDocumentStoreService for StubDocuments {
        async fn get_value(&self, _scope: &str, _key: &str) -> Result<Option<Value>, StorageError> {
            Ok(self.value.lock().clone())
        }

        async fn set_value(
            &self,
            _scope: &str,
            _key: &str,
            value: Value,
        ) -> Result<(), StorageError> {
            *self.value.lock() = Some(value);
            Ok(())
        }

        async fn delete(&self, _scope: &str, _key: &str) -> Result<(), StorageError> {
            Ok(())
        }

        async fn list(
            &self,
            _scope: &str,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, StorageError> {
            Ok(Vec::new())
        }

        fn watch(&self, _scope: &str, _key: &str) -> Event<()> {
            Event::none()
        }

        fn acquire(&self, _scope: &str, _key: &str) -> DisposableHandle {
            crate::_base::di::lifecycle::disposable_none()
        }
    }

    fn persistence(value: Option<Value>) -> (FileWorkspacePersistence, Arc<StubDocuments>) {
        let documents = Arc::new(StubDocuments {
            value: Mutex::new(value),
        });
        (
            FileWorkspacePersistence::with_clock(
                Arc::clone(&documents) as Arc<dyn AtomicDocumentStoreService>,
                Arc::new(|| 1234),
            ),
            documents,
        )
    }

    #[tokio::test]
    async fn missing_or_unusable_documents_return_none() {
        assert_eq!(persistence(None).0.load().await.unwrap(), None);
        assert_eq!(
            persistence(Some(serde_json::json!({"version": 1})))
                .0
                .load()
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            persistence(Some(serde_json::json!({"workspaces": null})))
                .0
                .load()
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn load_sanitizes_entries_times_and_dirty_tombstones() {
        let (persistence, _) = persistence(Some(serde_json::json!({
            "version": "ignored",
            "workspaces": {
                "valid": {
                    "root": "/repo",
                    "name": "repo",
                    "created_at": "1970-01-01T00:00:01.000Z",
                    "last_opened_at": "invalid"
                },
                "bad": {"root": "/bad"}
            },
            "deleted_workspace_ids": ["gone", 42, null]
        })));

        let catalog = persistence.load().await.unwrap().unwrap();
        assert_eq!(catalog.workspaces.len(), 1);
        assert_eq!(catalog.workspaces[0].created_at_millis, 1000);
        assert_eq!(catalog.workspaces[0].last_opened_at_millis, 1234);
        assert_eq!(catalog.deleted_ids, ["gone"]);
    }

    #[tokio::test]
    async fn load_accepts_array_workspaces_like_object_entries() {
        let (persistence, _) = persistence(Some(serde_json::json!({
            "workspaces": [{
                "root": "/repo", "name": "repo",
                "created_at": "1970-01-01", "last_opened_at": "1970-01-02"
            }]
        })));
        let catalog = persistence.load().await.unwrap().unwrap();
        assert_eq!(catalog.workspaces[0].id, "0");
        assert_eq!(catalog.workspaces[0].created_at_millis, 0);
        assert_eq!(catalog.workspaces[0].last_opened_at_millis, 86_400_000);
    }

    #[tokio::test]
    async fn save_writes_exact_v1_shape_and_round_trips() {
        let (persistence, documents) = persistence(None);
        let catalog = WorkspaceCatalog {
            workspaces: vec![Workspace {
                id: "wd_repo_hash".into(),
                root: "/repo".into(),
                name: "repo".into(),
                created_at_millis: 0,
                last_opened_at_millis: 1000,
            }],
            deleted_ids: vec!["wd_old_hash".into()],
        };
        persistence.save(&catalog).await.unwrap();
        assert_eq!(
            documents.value.lock().clone().unwrap(),
            serde_json::json!({
                "version": 1,
                "workspaces": {
                    "wd_repo_hash": {
                        "root": "/repo",
                        "name": "repo",
                        "created_at": "1970-01-01T00:00:00.000Z",
                        "last_opened_at": "1970-01-01T00:00:01.000Z"
                    }
                },
                "deleted_workspace_ids": ["wd_old_hash"]
            })
        );
        assert_eq!(persistence.load().await.unwrap(), Some(catalog));
    }

    #[test]
    fn time_parser_uses_supported_date_parse_forms_and_shared_fallback() {
        assert_eq!(parse_time("invalid", 77), 77);
        assert_eq!(parse_time("Thu, 01 Jan 1970 00:00:01 +0000", 77), 1000);
        assert_eq!(timestamp_to_iso(0).unwrap(), "1970-01-01T00:00:00.000Z");
    }
}
