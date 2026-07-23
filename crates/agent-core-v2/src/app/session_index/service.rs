//! Filesystem-backed persisted-session index.
//!
//! Original: `packages/agent-core-v2/src/app/sessionIndex/sessionIndexService.ts`.

use std::{
    cmp::Ordering,
    error::Error,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate};
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        log::{LOG_SERVICE_ID, LogPayload, LogServiceHandle},
    },
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle, PersistenceScopeName},
        flag::{FLAG_SERVICE_ID, FlagServiceHandle},
    },
    persistence::interface::{
        atomic_document_store::{ATOMIC_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreHandle},
        query_store::{IndexDef, Page, QUERY_STORE_SERVICE_ID, QueryStoreHandle},
        storage::{
            FILE_SYSTEM_STORAGE_SERVICE_ID, FileSystemStorageServiceHandle, STORAGE_LOCKED,
            StorageError,
        },
    },
};

use super::contract::{
    CHILD_SESSION_KIND, CHILD_SESSION_KIND_KEY, PARENT_SESSION_ID_KEY, SESSION_INDEX_SERVICE_ID,
    SessionIndexContract, SessionIndexHandle, SessionIndexResult, SessionListQuery, SessionSummary,
};

const META_SCOPE: &str = "session-meta";
const META_KEY: &str = "state.json";
const SESSION_COLLECTION: &str = "session";
const READ_MODEL_FLAG: &str = "persistence_minidb_readmodel";

pub struct FileSessionIndex {
    bootstrap: BootstrapServiceHandle,
    storage: FileSystemStorageServiceHandle,
    documents: AtomicDocumentStoreHandle,
    query_store: QueryStoreHandle,
    flags: FlagServiceHandle,
    log: LogServiceHandle,
    indexes_ensured: AtomicBool,
    read_model_disabled: AtomicBool,
}

impl FileSessionIndex {
    // Original: FileSessionIndex.constructor(). Atomic booleans are the Rust
    // representation of the source's process-lifetime boolean fields.
    pub fn new(
        bootstrap: BootstrapServiceHandle,
        storage: FileSystemStorageServiceHandle,
        documents: AtomicDocumentStoreHandle,
        query_store: QueryStoreHandle,
        flags: FlagServiceHandle,
        log: LogServiceHandle,
    ) -> Self {
        Self {
            bootstrap,
            storage,
            documents,
            query_store,
            flags,
            log,
            indexes_ensured: AtomicBool::new(false),
            read_model_disabled: AtomicBool::new(false),
        }
    }

    // Original: FileSessionIndex.withReadModelFallback(). The legacy future is
    // created only after the lock failure, preserving call order and effects.
    async fn with_read_model_fallback<T, O, L, LF>(
        &self,
        operation: O,
        legacy: L,
    ) -> SessionIndexResult<T>
    where
        O: Future<Output = SessionIndexResult<T>>,
        L: FnOnce() -> LF,
        LF: Future<Output = SessionIndexResult<T>>,
    {
        if self.read_model_disabled.load(AtomicOrdering::Acquire) {
            return legacy().await;
        }
        match operation.await {
            Ok(value) => Ok(value),
            Err(error) if is_storage_locked(error.as_ref()) => {
                self.read_model_disabled
                    .store(true, AtomicOrdering::Release);
                self.log.0.warn(
                    "query-store locked by another process; disabling read model",
                    Some(LogPayload::Context(Map::from_iter([(
                        "error".into(),
                        Value::String(error.to_string()),
                    )]))),
                );
                legacy().await
            }
            Err(error) => Err(error),
        }
    }

    // Original: FileSessionIndex.listFromReadModel().
    async fn list_from_read_model(
        &self,
        query: &SessionListQuery,
    ) -> SessionIndexResult<Page<SessionSummary>> {
        self.ensure_indexes().await?;
        if let Some(session_id) = query.session_id.as_deref() {
            let summary = self.get_from_read_model(session_id).await?;
            return Ok(single_session_page(summary, query));
        }

        let workspace_ids = match &query.workspace_ids {
            Some(ids) => ids.clone(),
            None => self.list_workspace_ids().await,
        };
        let mut items = Vec::new();
        for workspace_id in workspace_ids {
            for session_id in self.list_session_ids(&workspace_id).await {
                let Some(summary) = self.get_cached_summary(&workspace_id, &session_id).await?
                else {
                    continue;
                };
                if summary.archived && query.include_archived != Some(true) {
                    continue;
                }
                if !matches_child_of(&summary, query.child_of.as_deref()) {
                    continue;
                }
                items.push(summary);
            }
        }
        Ok(session_page(items, query.limit))
    }

    // Original: FileSessionIndex.getFromReadModel().
    async fn get_from_read_model(&self, id: &str) -> SessionIndexResult<Option<SessionSummary>> {
        if let Some(summary) = self.query_store.get(SESSION_COLLECTION, id).await? {
            return Ok(Some(summary));
        }
        for workspace_id in self.list_workspace_ids().await {
            if !self.has_session(&workspace_id, id).await {
                continue;
            }
            return self.get_cached_summary(&workspace_id, id).await;
        }
        Ok(None)
    }

    // Original: FileSessionIndex.countActiveFromReadModel().
    async fn count_active_from_read_model(
        &self,
        workspace_ids: &[String],
    ) -> SessionIndexResult<usize> {
        let mut count = 0;
        for workspace_id in workspace_ids {
            for session_id in self.list_session_ids(workspace_id).await {
                if self
                    .get_cached_summary(workspace_id, &session_id)
                    .await?
                    .is_some_and(|summary| !summary.archived)
                {
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    fn read_model_enabled(&self) -> bool {
        self.flags.enabled(READ_MODEL_FLAG)
    }

    // Original: FileSessionIndex.ensureIndexes(). State is committed only
    // after both index definitions succeed.
    async fn ensure_indexes(&self) -> SessionIndexResult<()> {
        if self.indexes_ensured.load(AtomicOrdering::Acquire) {
            return Ok(());
        }
        self.query_store
            .0
            .ensure_index(
                SESSION_COLLECTION,
                &IndexDef::Value {
                    name: "byWorkspace".into(),
                    field: "workspaceId".into(),
                    unique: None,
                },
            )
            .await?;
        self.query_store
            .0
            .ensure_index(
                SESSION_COLLECTION,
                &IndexDef::Compound {
                    name: "byWsUpdated".into(),
                    group_by: "workspaceId".into(),
                    order_by: "updatedAt".into(),
                },
            )
            .await?;
        self.indexes_ensured.store(true, AtomicOrdering::Release);
        Ok(())
    }

    // Original: FileSessionIndex.getCachedSummary().
    async fn get_cached_summary(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> SessionIndexResult<Option<SessionSummary>> {
        if let Some(summary) = self.query_store.get(SESSION_COLLECTION, session_id).await? {
            return Ok(Some(summary));
        }
        let summary = self.read_summary(workspace_id, session_id).await;
        if let Some(summary) = &summary {
            self.query_store
                .put(SESSION_COLLECTION, session_id, summary)
                .await?;
        }
        Ok(summary)
    }

    // Original: FileSessionIndex.listLegacy().
    async fn list_legacy(
        &self,
        query: &SessionListQuery,
    ) -> SessionIndexResult<Page<SessionSummary>> {
        if let Some(session_id) = query.session_id.as_deref() {
            let summary = self.get_legacy(session_id).await;
            return Ok(single_session_page(summary, query));
        }

        let workspace_ids = match &query.workspace_ids {
            Some(ids) => ids.clone(),
            None => self.list_workspace_ids().await,
        };
        let mut items = Vec::new();
        for workspace_id in workspace_ids {
            for session_id in self.list_session_ids(&workspace_id).await {
                let Some(summary) = self.read_summary(&workspace_id, &session_id).await else {
                    continue;
                };
                if summary.archived && query.include_archived != Some(true) {
                    continue;
                }
                if !matches_child_of(&summary, query.child_of.as_deref()) {
                    continue;
                }
                items.push(summary);
            }
        }
        Ok(session_page(items, query.limit))
    }

    // Original: FileSessionIndex.getLegacy().
    async fn get_legacy(&self, id: &str) -> Option<SessionSummary> {
        for workspace_id in self.list_workspace_ids().await {
            if !self.has_session(&workspace_id, id).await {
                continue;
            }
            if let Some(summary) = self.read_summary(&workspace_id, id).await {
                return Some(summary);
            }
        }
        None
    }

    // Original: FileSessionIndex.countActiveLegacy().
    async fn count_active_legacy(&self, workspace_ids: &[String]) -> usize {
        let mut count = 0;
        for workspace_id in workspace_ids {
            for session_id in self.list_session_ids(workspace_id).await {
                if self
                    .read_summary(workspace_id, &session_id)
                    .await
                    .is_some_and(|summary| !summary.archived)
                {
                    count += 1;
                }
            }
        }
        count
    }

    fn sessions_scope(&self) -> &str {
        self.bootstrap.scope(PersistenceScopeName::Sessions)
    }

    // Original: FileSessionIndex.listWorkspaceIds(). All storage errors are
    // intentionally treated as an empty directory.
    async fn list_workspace_ids(&self) -> Vec<String> {
        self.storage
            .0
            .list(self.sessions_scope(), None)
            .await
            .unwrap_or_default()
    }

    // Original: FileSessionIndex.listSessionIds().
    async fn list_session_ids(&self, workspace_id: &str) -> Vec<String> {
        self.storage
            .0
            .list(&format!("{}/{workspace_id}", self.sessions_scope()), None)
            .await
            .unwrap_or_default()
    }

    // Original: FileSessionIndex.hasSession().
    async fn has_session(&self, workspace_id: &str, session_id: &str) -> bool {
        self.list_session_ids(workspace_id)
            .await
            .iter()
            .any(|id| id == session_id)
    }

    // Original: FileSessionIndex.readSummary(). The unified state location is
    // attempted before the historical `session-meta` location.
    async fn read_summary(&self, workspace_id: &str, session_id: &str) -> Option<SessionSummary> {
        let base = format!("{}/{workspace_id}/{session_id}", self.sessions_scope());
        let metadata = match self.read_meta(&base).await {
            Some(metadata) => metadata,
            None => self.read_meta(&format!("{base}/{META_SCOPE}")).await?,
        };
        let custom = metadata.get("custom").and_then(Value::as_object).cloned();
        Some(SessionSummary {
            id: session_id.into(),
            workspace_id: workspace_id.into(),
            cwd: recover_cwd(&metadata),
            title: metadata
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned),
            last_prompt: metadata
                .get("lastPrompt")
                .and_then(Value::as_str)
                .map(str::to_owned),
            created_at: parse_time(metadata.get("createdAt")),
            updated_at: parse_time(metadata.get("updatedAt")),
            archived: metadata.get("archived") == Some(&Value::Bool(true)),
            custom,
        })
    }

    // Original: FileSessionIndex.readMeta(). Decode and I/O failures both
    // make this candidate location absent.
    async fn read_meta(&self, scope: &str) -> Option<Map<String, Value>> {
        self.documents
            .get(scope, META_KEY)
            .await
            .unwrap_or_default()
    }
}

#[async_trait]
impl SessionIndexContract for FileSessionIndex {
    // Original: FileSessionIndex.list().
    async fn list(&self, query: SessionListQuery) -> SessionIndexResult<Page<SessionSummary>> {
        if !self.read_model_enabled() {
            return self.list_legacy(&query).await;
        }
        self.with_read_model_fallback(self.list_from_read_model(&query), || {
            self.list_legacy(&query)
        })
        .await
    }

    // Original: FileSessionIndex.get().
    async fn get(&self, id: &str) -> SessionIndexResult<Option<SessionSummary>> {
        if !self.read_model_enabled() {
            return Ok(self.get_legacy(id).await);
        }
        self.with_read_model_fallback(self.get_from_read_model(id), || async {
            Ok(self.get_legacy(id).await)
        })
        .await
    }

    // Original: FileSessionIndex.countActive().
    async fn count_active(&self, workspace_ids: &[String]) -> SessionIndexResult<usize> {
        if !self.read_model_enabled() {
            return Ok(self.count_active_legacy(workspace_ids).await);
        }
        self.with_read_model_fallback(self.count_active_from_read_model(workspace_ids), || async {
            Ok(self.count_active_legacy(workspace_ids).await)
        })
        .await
    }
}

// Original: parseTime(). Persisted v1 timestamps are ISO strings; numeric v2
// timestamps retain their finite floating-point value.
fn parse_time(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().filter(|value| value.is_finite()),
        Some(Value::String(value)) => parse_date(value),
        _ => None,
    }
    .unwrap_or(0.0)
}

fn parse_date(value: &str) -> Option<f64> {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.timestamp_millis() as f64)
        .or_else(|_| DateTime::parse_from_rfc2822(value).map(|date| date.timestamp_millis() as f64))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc().timestamp_millis() as f64)
        })
}

// Original: recoverCwd().
fn recover_cwd(metadata: &Map<String, Value>) -> Option<String> {
    for value in [metadata.get("cwd"), metadata.get("workDir")] {
        if let Some(value) = value
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return Some(value.into());
        }
    }
    metadata
        .get("custom")
        .and_then(Value::as_object)
        .and_then(|custom| custom.get("cwd"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

// Original: matchesChildOf().
fn matches_child_of(summary: &SessionSummary, parent_id: Option<&str>) -> bool {
    let Some(parent_id) = parent_id else {
        return true;
    };
    summary.custom.as_ref().is_some_and(|custom| {
        custom.get(PARENT_SESSION_ID_KEY).and_then(Value::as_str) == Some(parent_id)
            && custom.get(CHILD_SESSION_KIND_KEY).and_then(Value::as_str)
                == Some(CHILD_SESSION_KIND)
    })
}

fn single_session_page(
    summary: Option<SessionSummary>,
    query: &SessionListQuery,
) -> Page<SessionSummary> {
    let mut items = summary
        .filter(|summary| !summary.archived || query.include_archived == Some(true))
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(limit) = query.limit {
        items.truncate(limit);
    }
    Page {
        items,
        next_cursor: None,
    }
}

fn session_page(mut items: Vec<SessionSummary>, limit: Option<usize>) -> Page<SessionSummary> {
    items.sort_by(|left, right| {
        right
            .updated_at
            .partial_cmp(&left.updated_at)
            .unwrap_or(Ordering::Equal)
    });
    if let Some(limit) = limit {
        items.truncate(limit);
    }
    Page {
        items,
        next_cursor: None,
    }
}

fn is_storage_locked(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error
            .downcast_ref::<StorageError>()
            .is_some_and(|error| error.code() == STORAGE_LOCKED)
        {
            return true;
        }
        current = error.source();
    }
    false
}

pub fn register_session_index_service() {
    register_scoped_service(
        LifecycleScope::App,
        SESSION_INDEX_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let storage = accessor.get(FILE_SYSTEM_STORAGE_SERVICE_ID)?;
            let documents = accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?;
            let query_store = accessor.get(QUERY_STORE_SERVICE_ID)?;
            let flags = accessor.get(FLAG_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let service: Arc<dyn SessionIndexContract> = Arc::new(FileSessionIndex::new(
                (*bootstrap).clone(),
                (*storage).clone(),
                (*documents).clone(),
                (*query_store).clone(),
                (*flags).clone(),
                (*log).clone(),
            ));
            Ok(SessionIndexHandle(service))
        }),
        InstantiationType::Eager,
        "sessionIndex",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicUsize, Ordering as TestOrdering},
    };

    use futures_util::future::{BoxFuture, ready};
    use serde_json::json;

    use crate::{
        _base::{
            di::lifecycle::{Disposable, DisposableHandle, DisposeResult, disposable_none},
            log::{LogContext, LogLevel, LogService, Logger},
        },
        app::{
            bootstrap::{BootstrapOptions, BootstrapService, BootstrapServiceContract},
            flag::{
                ExperimentalFeatureState, ExperimentalFlagConfig, ExperimentalFlagMap,
                FlagDefinitionInput, FlagId, FlagRegistry, FlagRegistryError,
            },
        },
        persistence::{
            backends::{
                minidb::mini_db_query_store::MiniDbQueryStore,
                node_fs::{
                    atomic_document_store::JsonAtomicDocumentStore,
                    file_storage_service::FileStorageService,
                },
            },
            interface::{
                atomic_document_store::AtomicDocumentStoreService,
                query_store::{QueryStoreError, QueryStoreService},
                storage::FileSystemStorageService,
            },
        },
    };

    use super::*;

    #[derive(Default)]
    struct EmptyFlagRegistry;

    impl FlagRegistry for EmptyFlagRegistry {
        fn register(&self, _: FlagDefinitionInput) -> Result<DisposableHandle, FlagRegistryError> {
            Ok(disposable_none())
        }

        fn get(&self, _: &str) -> Option<FlagDefinitionInput> {
            None
        }

        fn list(&self) -> Vec<FlagDefinitionInput> {
            Vec::new()
        }
    }

    struct TestFlags {
        read_model: bool,
    }

    impl Disposable for TestFlags {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    impl crate::app::flag::FlagServiceContract for TestFlags {
        fn registry(&self) -> Arc<dyn FlagRegistry> {
            Arc::new(EmptyFlagRegistry)
        }

        fn enabled(&self, id: &str) -> bool {
            self.read_model && id == READ_MODEL_FLAG
        }

        fn snapshot(&self) -> ExperimentalFlagMap {
            ExperimentalFlagMap::new()
        }

        fn enabled_ids(&self) -> Vec<FlagId> {
            self.read_model
                .then(|| READ_MODEL_FLAG.into())
                .into_iter()
                .collect()
        }

        fn explain(&self, _: &str) -> Option<ExperimentalFeatureState> {
            None
        }

        fn explain_all(&self) -> Vec<ExperimentalFeatureState> {
            Vec::new()
        }

        fn set_config_overrides(&self, _: Option<ExperimentalFlagConfig>) {}
    }

    #[derive(Clone, Default)]
    struct TestLog {
        warnings: Arc<AtomicUsize>,
    }

    impl Logger for TestLog {
        fn error(&self, _: &str, _: Option<LogPayload>) {}

        fn warn(&self, _: &str, _: Option<LogPayload>) {
            self.warnings.fetch_add(1, TestOrdering::Relaxed);
        }

        fn info(&self, _: &str, _: Option<LogPayload>) {}

        fn debug(&self, _: &str, _: Option<LogPayload>) {}

        fn child(&self, _: LogContext) -> Arc<dyn Logger> {
            Arc::new(self.clone())
        }
    }

    impl Disposable for TestLog {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    impl LogService for TestLog {
        fn level(&self) -> LogLevel {
            LogLevel::Off
        }

        fn set_level(&self, _: LogLevel) {}

        fn flush(&self) -> BoxFuture<'_, std::io::Result<()>> {
            Box::pin(ready(Ok(())))
        }
    }

    struct Fixture {
        service: FileSessionIndex,
        documents: AtomicDocumentStoreHandle,
        query_store: QueryStoreHandle,
        log: TestLog,
        root: std::path::PathBuf,
    }

    impl Fixture {
        fn new(read_model: bool) -> Self {
            let root =
                std::env::temp_dir().join(format!("kimi-session-index-{}", uuid::Uuid::new_v4()));
            let bootstrap: Arc<dyn BootstrapServiceContract> =
                Arc::new(BootstrapService::new(BootstrapOptions {
                    home_dir: root.clone(),
                    config_path: root.join("config.toml"),
                    os_home_dir: root.clone(),
                    platform: "test".into(),
                    arch: "test".into(),
                    cwd: root.clone(),
                    env: HashMap::new(),
                    client_version: "test".into(),
                }));
            let storage: Arc<dyn FileSystemStorageService> =
                Arc::new(FileStorageService::with_default_modes(&root));
            let documents: Arc<dyn AtomicDocumentStoreService> =
                Arc::new(JsonAtomicDocumentStore::new(Arc::clone(&storage)));
            let query_backend = MiniDbQueryStore::new(root.join("cache"));
            let query_service: Arc<dyn QueryStoreService> = Arc::new(query_backend);
            let query_store = QueryStoreHandle(query_service);
            let log = TestLog::default();
            let service = FileSessionIndex::new(
                BootstrapServiceHandle(bootstrap),
                FileSystemStorageServiceHandle(storage),
                AtomicDocumentStoreHandle(Arc::clone(&documents)),
                query_store.clone(),
                FlagServiceHandle(Arc::new(TestFlags { read_model })),
                LogServiceHandle(Arc::new(log.clone())),
            );
            Self {
                service,
                documents: AtomicDocumentStoreHandle(documents),
                query_store,
                log,
                root,
            }
        }

        async fn write(&self, scope: &str, metadata: Value) {
            self.documents
                .set(scope, META_KEY, &metadata)
                .await
                .unwrap();
        }

        async fn cleanup(self) {
            self.query_store.0.close().await.unwrap();
            match tokio::fs::remove_dir_all(self.root).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("failed to clean session-index fixture: {error}"),
            }
        }
    }

    #[test]
    fn pure_metadata_helpers_preserve_legacy_compatibility() {
        assert_eq!(parse_time(Some(&json!(12.5))), 12.5);
        assert_eq!(
            parse_time(Some(&json!("2024-01-02T03:04:05.006Z"))),
            1_704_164_645_006.0
        );
        assert_eq!(parse_time(Some(&json!("invalid"))), 0.0);

        let metadata = json!({
            "cwd": "",
            "workDir": "/legacy",
            "custom": {"cwd": "/custom"}
        });
        assert_eq!(
            recover_cwd(metadata.as_object().unwrap()).as_deref(),
            Some("/legacy")
        );
    }

    #[test]
    fn child_filter_requires_both_original_custom_fields() {
        let mut summary = SessionSummary {
            id: "child-1".into(),
            workspace_id: "wd".into(),
            cwd: None,
            title: None,
            last_prompt: None,
            created_at: 0.0,
            updated_at: 0.0,
            archived: false,
            custom: Some(Map::from_iter([
                (PARENT_SESSION_ID_KEY.into(), json!("parent-1")),
                (CHILD_SESSION_KIND_KEY.into(), json!(CHILD_SESSION_KIND)),
            ])),
        };
        assert!(matches_child_of(&summary, None));
        assert!(matches_child_of(&summary, Some("parent-1")));
        summary
            .custom
            .as_mut()
            .unwrap()
            .insert(CHILD_SESSION_KIND_KEY.into(), json!("agent"));
        assert!(!matches_child_of(&summary, Some("parent-1")));
    }

    #[test]
    fn pages_sort_by_recency_then_apply_limit() {
        let make = |id: &str, updated_at| SessionSummary {
            id: id.into(),
            workspace_id: "wd".into(),
            cwd: None,
            title: None,
            last_prompt: None,
            created_at: 0.0,
            updated_at,
            archived: false,
            custom: None,
        };
        let page = session_page(
            vec![make("old", 1.0), make("new", 3.0), make("middle", 2.0)],
            Some(2),
        );
        assert_eq!(
            page.items
                .iter()
                .map(|summary| summary.id.as_str())
                .collect::<Vec<_>>(),
            ["new", "middle"]
        );
        assert_eq!(page.next_cursor, None);
    }

    #[tokio::test]
    async fn legacy_index_merges_buckets_recovers_both_layouts_and_filters() {
        let fixture = Fixture::new(false);
        fixture
            .write(
                "sessions/wd-a/child",
                json!({
                    "workDir": "/repo-a",
                    "title": "child",
                    "lastPrompt": "hello",
                    "createdAt": "2024-01-01T00:00:00.000Z",
                    "updatedAt": 30,
                    "custom": {
                        "parent_session_id": "parent",
                        "child_session_kind": "child"
                    }
                }),
            )
            .await;
        fixture
            .write(
                "sessions/wd-b/archived/session-meta",
                json!({
                    "cwd": "/repo-b",
                    "createdAt": 10,
                    "updatedAt": 40,
                    "archived": true
                }),
            )
            .await;
        fixture
            .write(
                "sessions/wd-b/active",
                json!({"cwd": "/repo-b", "createdAt": 20, "updatedAt": 20}),
            )
            .await;

        let child_page = fixture
            .service
            .list(SessionListQuery {
                workspace_ids: Some(vec!["wd-a".into(), "wd-b".into()]),
                child_of: Some("parent".into()),
                ..SessionListQuery::default()
            })
            .await
            .unwrap();
        assert_eq!(child_page.items.len(), 1);
        assert_eq!(child_page.items[0].id, "child");
        assert_eq!(child_page.items[0].cwd.as_deref(), Some("/repo-a"));

        let all = fixture
            .service
            .list(SessionListQuery {
                workspace_ids: Some(vec!["wd-a".into(), "wd-b".into()]),
                include_archived: Some(true),
                ..SessionListQuery::default()
            })
            .await
            .unwrap();
        assert_eq!(
            all.items
                .iter()
                .map(|summary| summary.id.as_str())
                .collect::<Vec<_>>(),
            ["archived", "child", "active"]
        );
        assert_eq!(
            fixture
                .service
                .count_active(&["wd-a".into(), "wd-b".into()])
                .await
                .unwrap(),
            2
        );
        assert!(
            fixture
                .service
                .get("archived")
                .await
                .unwrap()
                .unwrap()
                .archived
        );

        fixture.cleanup().await;
    }

    #[tokio::test]
    async fn read_model_cold_miss_backfills_and_survives_missing_metadata() {
        let fixture = Fixture::new(true);
        fixture
            .write(
                "sessions/wd/session-1",
                json!({"cwd": "/repo", "createdAt": 1, "updatedAt": 2}),
            )
            .await;

        let first = fixture.service.get("session-1").await.unwrap().unwrap();
        assert_eq!(first.cwd.as_deref(), Some("/repo"));
        assert_eq!(
            fixture
                .query_store
                .get::<SessionSummary>(SESSION_COLLECTION, "session-1")
                .await
                .unwrap(),
            Some(first.clone())
        );
        fixture
            .documents
            .delete("sessions/wd/session-1", META_KEY)
            .await
            .unwrap();
        assert_eq!(fixture.service.get("session-1").await.unwrap(), Some(first));
        assert_eq!(fixture.log.warnings.load(TestOrdering::Relaxed), 0);

        fixture.cleanup().await;
    }

    #[tokio::test]
    async fn storage_lock_warns_once_and_disables_the_read_model() {
        let fixture = Fixture::new(true);
        let locked: SessionIndexResult<usize> = Err(Box::new(QueryStoreError::backend(
            StorageError::new(STORAGE_LOCKED, "locked"),
        )));

        let first = fixture
            .service
            .with_read_model_fallback(async { locked }, || async { Ok(41) })
            .await
            .unwrap();
        assert_eq!(first, 41);
        assert_eq!(fixture.log.warnings.load(TestOrdering::Relaxed), 1);

        let second = fixture
            .service
            .with_read_model_fallback(async { Ok(99) }, || async { Ok(42) })
            .await
            .unwrap();
        assert_eq!(second, 42);
        assert_eq!(fixture.log.warnings.load(TestOrdering::Relaxed), 1);

        fixture.cleanup().await;
    }
}
