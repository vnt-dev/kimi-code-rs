//! Session-scoped durable metadata with optional query read-model mirroring.
//!
//! Original:
//! `packages/agent-core-v2/src/session/sessionMetadata/sessionMetadataService.ts`.

use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};
use std::sync::{Arc};
use parking_lot::Mutex;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::{Emitter, Event},
        log::{LOG_SERVICE_ID, LogPayload, LogServiceHandle},
    },
    app::flag::{FLAG_SERVICE_ID, FlagServiceHandle},
    persistence::interface::{
        atomic_document_store::{ATOMIC_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreHandle},
        query_store::{QUERY_STORE_SERVICE_ID, QueryStoreHandle},
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
};

use super::{
    AgentMeta, SESSION_META_VERSION, SESSION_METADATA_ID, SessionMeta, SessionMetaPatch,
    SessionMetadataChangedEvent, SessionMetadataContract, SessionMetadataError,
    SessionMetadataHandle,
};

const META_KEY: &str = "state.json";
const SESSION_COLLECTION: &str = "session";
const READ_MODEL_FLAG: &str = "persistence_minidb_readmodel";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionReadModel<'a> {
    id: &'a str,
    workspace_id: &'a str,
    cwd: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_prompt: Option<&'a str>,
    created_at: i64,
    updated_at: i64,
    archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom: Option<&'a BTreeMap<String, Value>>,
}

pub struct SessionMetadataService {
    context: SessionContext,
    store: AtomicDocumentStoreHandle,
    log: LogServiceHandle,
    query_store: QueryStoreHandle,
    flags: FlagServiceHandle,
    data: Mutex<Option<SessionMeta>>,
    load_lock: AsyncMutex<()>,
    update_lock: AsyncMutex<()>,
    changed: Arc<Emitter<SessionMetadataChangedEvent>>,
}

impl SessionMetadataService {
    pub fn new(
        context: &SessionContext,
        store: AtomicDocumentStoreHandle,
        log: LogServiceHandle,
        query_store: QueryStoreHandle,
        flags: FlagServiceHandle,
    ) -> Self {
        Self {
            context: context.clone(),
            store,
            log,
            query_store,
            flags,
            data: Mutex::new(None),
            load_lock: AsyncMutex::new(()),
            update_lock: AsyncMutex::new(()),
            changed: Arc::new(Emitter::new()),
        }
    }
    pub fn on_did_change_metadata(&self) -> Event<SessionMetadataChangedEvent> {
        self.changed.event()
    }
    pub async fn ready(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_loaded().await
    }
    pub async fn read(&self) -> Result<SessionMeta, Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_loaded().await?;
        Ok(self.data.lock().clone().expect("loaded metadata"))
    }
    pub async fn update(
        &self,
        patch: SessionMetaPatch,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _lock = self.update_lock.lock().await;
        self.apply_update(patch).await
    }
    async fn apply_update(
        &self,
        patch: SessionMetaPatch,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_loaded().await?;
        let mut data = self.data.lock().clone().expect("loaded metadata");
        let changed = patch_keys(&patch);
        apply_patch(&mut data, patch);
        data.updated_at = now_ms();
        // The source updates its in-memory model before awaiting persistence.
        // Preserve that observation order for concurrent readers.
        *self.data.lock() = Some(data.clone());
        self.store
            .set(&self.context.meta_scope, META_KEY, &data)
            .await?;
        self.mirror_to_read_model(&data).await;
        self.changed.fire(&SessionMetadataChangedEvent { changed });
        Ok(())
    }
    pub async fn set_title(
        &self,
        title: String,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.update(SessionMetaPatch {
            title: Some(title),
            is_custom_title: Some(true),
            ..Default::default()
        })
        .await
    }
    pub async fn set_archived(
        &self,
        archived: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.update(SessionMetaPatch {
            archived: Some(archived),
            ..Default::default()
        })
        .await
    }
    pub async fn register_agent(
        &self,
        agent_id: String,
        meta: AgentMeta,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _lock = self.update_lock.lock().await;
        self.ensure_loaded().await?;
        let data = self.data.lock().clone().expect("loaded metadata");
        if data
            .agents
            .as_ref()
            .and_then(|agents| agents.get(&agent_id))
            .is_some_and(|existing| agent_meta_equals(existing, &meta))
        {
            return Ok(());
        }
        let mut agents = data.agents.clone().unwrap_or_default();
        agents.insert(agent_id, meta);
        self.apply_update(SessionMetaPatch {
            agents: Some(agents),
            ..SessionMetaPatch::default()
        })
        .await
    }
    async fn mirror_to_read_model(&self, data: &SessionMeta) {
        if !self.flags.enabled(READ_MODEL_FLAG) {
            return;
        }
        let summary = SessionReadModel {
            id: &data.id,
            workspace_id: &self.context.workspace_id,
            cwd: &self.context.cwd,
            title: data.title.as_deref(),
            last_prompt: data.last_prompt.as_deref(),
            created_at: data.created_at,
            updated_at: data.updated_at,
            archived: data.archived,
            custom: data.custom.as_ref(),
        };
        if let Err(error) = self
            .query_store
            .put(SESSION_COLLECTION, &self.context.session_id, &summary)
            .await
        {
            self.log.0.warn(
                "failed to mirror session metadata to read model",
                Some(LogPayload::Context(Map::from_iter([
                    (
                        "sessionId".into(),
                        Value::String(self.context.session_id.clone()),
                    ),
                    ("error".into(), Value::String(error.to_string())),
                ]))),
            );
        }
    }
    async fn ensure_loaded(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.data.lock().is_some() {
            return Ok(());
        }
        let _lock = self.load_lock.lock().await;
        if self.data.lock().is_some() {
            return Ok(());
        }
        let loaded = self
            .store
            .get::<serde_json::Value>(&self.context.meta_scope, META_KEY)
            .await?;
        let created = loaded.is_none();
        let mut data = match loaded {
            Some(data) => normalize_session_meta(
                session_meta_from_value(data, &self.context.session_id)?,
                &self.context.session_id,
            ),
            None => SessionMeta {
                id: self.context.session_id.clone(),
                version: Some(SESSION_META_VERSION),
                title: None,
                is_custom_title: None,
                last_prompt: None,
                created_at: now_ms(),
                updated_at: now_ms(),
                archived: false,
                cwd: Some(self.context.cwd.clone()),
                forked_from: None,
                agents: Some(BTreeMap::new()),
                custom: Some(BTreeMap::new()),
            },
        };
        let heal = data.agents.is_none() || data.custom.is_none();
        if data.agents.is_none() {
            data.agents = Some(BTreeMap::new());
        }
        if data.custom.is_none() {
            data.custom = Some(BTreeMap::new());
        }
        if created || heal {
            self.store
                .set(&self.context.meta_scope, META_KEY, &data)
                .await?;
        }
        if created {
            self.log.0.debug(
                "session metadata created",
                Some(LogPayload::Context(Map::from_iter([(
                    "sessionId".into(),
                    Value::String(self.context.session_id.clone()),
                )]))),
            );
        }
        *self.data.lock() = Some(data);
        Ok(())
    }
}

#[async_trait]
impl SessionMetadataContract for SessionMetadataService {
    async fn ready(&self) -> Result<(), SessionMetadataError> {
        Self::ready(self).await
    }

    fn on_did_change_metadata(&self) -> Event<SessionMetadataChangedEvent> {
        Self::on_did_change_metadata(self)
    }

    async fn read(&self) -> Result<SessionMeta, SessionMetadataError> {
        Self::read(self).await
    }

    async fn update(&self, patch: SessionMetaPatch) -> Result<(), SessionMetadataError> {
        Self::update(self, patch).await
    }

    async fn set_title(&self, title: String) -> Result<(), SessionMetadataError> {
        Self::set_title(self, title).await
    }

    async fn set_archived(&self, archived: bool) -> Result<(), SessionMetadataError> {
        Self::set_archived(self, archived).await
    }

    async fn register_agent(
        &self,
        agent_id: String,
        meta: AgentMeta,
    ) -> Result<(), SessionMetadataError> {
        Self::register_agent(self, agent_id, meta).await
    }
}

/// Register the eager Session-scope metadata service.
///
/// Original: `registerScopedService(LifecycleScope.Session, ISessionMetadata, ...)`.
pub fn register_session_metadata() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_METADATA_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(SESSION_CONTEXT_ID)?;
            let store = accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let query_store = accessor.get(QUERY_STORE_SERVICE_ID)?;
            let flags = accessor.get(FLAG_SERVICE_ID)?;
            let service: Arc<dyn SessionMetadataContract> = Arc::new(SessionMetadataService::new(
                &context,
                (*store).clone(),
                (*log).clone(),
                (*query_store).clone(),
                (*flags).clone(),
            ));
            Ok(SessionMetadataHandle(service))
        }),
        InstantiationType::Eager,
        "sessionMetadata",
    );
}

pub fn normalize_session_meta(mut raw: SessionMeta, session_id: &str) -> SessionMeta {
    if raw.version == Some(SESSION_META_VERSION) {
        return raw;
    }
    raw.id = session_id.into();
    raw.version = Some(SESSION_META_VERSION);
    raw
}
fn session_meta_from_value(
    mut value: serde_json::Value,
    session_id: &str,
) -> Result<SessionMeta, serde_json::Error> {
    if let Some(object) = value.as_object_mut() {
        // Legacy TypeScript sessions predate the persisted `id` field. The
        // TypeScript loader normalizes those untyped objects before using the
        // field, so provide the directory-derived id before Rust's typed
        // deserialization enforces it.
        if !object.get("id").is_some_and(Value::is_string) {
            object.insert("id".into(), Value::String(session_id.into()));
        }
        let legacy_cwd = object
            .get("workDir")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if object.get("cwd").is_none_or(Value::is_null)
            && let Some(cwd) = legacy_cwd
        {
            object.insert("cwd".into(), Value::String(cwd));
        }
        object.remove("workDir");
        for key in ["createdAt", "updatedAt"] {
            if let Some(serde_json::Value::String(text)) = object.get(key) {
                let parsed = parse_epoch_ms(text);
                object.insert(key.into(), serde_json::Value::from(parsed));
            }
        }
    }
    serde_json::from_value(value)
}
fn parse_epoch_ms(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(value))
        .map(|date| date.timestamp_millis())
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map(|date| {
                    date.and_hms_opt(0, 0, 0)
                        .expect("midnight is valid")
                        .and_utc()
                })
                .map(|date| date.timestamp_millis())
        })
        .unwrap_or_default()
}
fn agent_meta_equals(left: &AgentMeta, right: &AgentMeta) -> bool {
    left.homedir == right.homedir
        && left.r#type == right.r#type
        && left.parent_agent_id == right.parent_agent_id
        && left.forked_from == right.forked_from
        && left.swarm_item == right.swarm_item
        && left.labels.as_ref().filter(|labels| !labels.is_empty())
            == right.labels.as_ref().filter(|labels| !labels.is_empty())
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default()
}
fn apply_patch(data: &mut SessionMeta, patch: SessionMetaPatch) {
    if let Some(value) = patch.version {
        data.version = Some(value)
    }
    if let Some(value) = patch.title {
        data.title = Some(value)
    }
    if let Some(value) = patch.is_custom_title {
        data.is_custom_title = Some(value)
    }
    if let Some(value) = patch.last_prompt {
        data.last_prompt = Some(value)
    }
    if let Some(value) = patch.archived {
        data.archived = value
    }
    if let Some(value) = patch.cwd {
        data.cwd = Some(value)
    }
    if let Some(value) = patch.forked_from {
        data.forked_from = Some(value)
    }
    if let Some(value) = patch.agents {
        data.agents = Some(value)
    }
    if let Some(value) = patch.custom {
        data.custom = Some(value)
    }
}
fn patch_keys(patch: &SessionMetaPatch) -> Vec<String> {
    let mut keys = Vec::new();
    if patch.version.is_some() {
        keys.push("version".into())
    }
    if patch.title.is_some() {
        keys.push("title".into())
    }
    if patch.is_custom_title.is_some() {
        keys.push("isCustomTitle".into())
    }
    if patch.last_prompt.is_some() {
        keys.push("lastPrompt".into())
    }
    if patch.archived.is_some() {
        keys.push("archived".into())
    }
    if patch.cwd.is_some() {
        keys.push("cwd".into())
    }
    if patch.forked_from.is_some() {
        keys.push("forkedFrom".into())
    }
    if patch.agents.is_some() {
        keys.push("agents".into())
    }
    if patch.custom.is_some() {
        keys.push("custom".into())
    }
    keys
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use futures_util::future::{BoxFuture, ready};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        _base::{
            di::lifecycle::{Disposable, DisposableHandle, DisposeResult, disposable_none},
            event::Event,
            log::{LogContext, LogLevel, LogService, Logger},
        },
        app::flag::{
            ExperimentalFeatureState, ExperimentalFlagConfig, ExperimentalFlagMap,
            FlagDefinitionInput, FlagId, FlagRegistry, FlagRegistryError,
        },
        persistence::interface::{
            atomic_document_store::AtomicDocumentStoreService,
            query_store::{
                Checkpoint, IndexDef, Page, QueryBuilderService, QueryFilter, QueryStoreError,
                QueryStoreService, SortDir, WriteOp,
            },
            storage::StorageError,
        },
        session::session_context::{SessionContextInput, make_session_context},
    };

    #[derive(Default)]
    struct Store {
        values: Mutex<HashMap<(String, String), Value>>,
        writes: AtomicUsize,
        fail_next_write: AtomicBool,
    }

    #[async_trait]
    impl AtomicDocumentStoreService for Store {
        async fn get_value(&self, scope: &str, key: &str) -> Result<Option<Value>, StorageError> {
            Ok(self
                .values
                .lock()
                .get(&(scope.into(), key.into()))
                .cloned())
        }
        async fn set_value(
            &self,
            scope: &str,
            key: &str,
            value: Value,
        ) -> Result<(), StorageError> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            if self.fail_next_write.swap(false, Ordering::AcqRel) {
                return Err(StorageError::new(
                    crate::persistence::interface::storage::STORAGE_IO_FAILED,
                    "test write failed",
                ));
            }
            self.values
                .lock()
                .insert((scope.into(), key.into()), value);
            Ok(())
        }
        async fn delete(&self, _: &str, _: &str) -> Result<(), StorageError> {
            Ok(())
        }
        async fn list(&self, _: &str, _: Option<&str>) -> Result<Vec<String>, StorageError> {
            Ok(vec![])
        }
        fn watch(&self, _: &str, _: &str) -> Event<()> {
            Event::none()
        }
        fn acquire(&self, _: &str, _: &str) -> DisposableHandle {
            disposable_none()
        }
    }

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

    type LogEntries = Vec<(String, Option<LogPayload>)>;

    #[derive(Clone, Default)]
    struct TestLog {
        entries: Arc<Mutex<LogEntries>>,
    }

    impl Logger for TestLog {
        fn error(&self, message: &str, payload: Option<LogPayload>) {
            self.entries.lock().push((message.into(), payload));
        }

        fn warn(&self, message: &str, payload: Option<LogPayload>) {
            self.entries.lock().push((message.into(), payload));
        }

        fn info(&self, message: &str, payload: Option<LogPayload>) {
            self.entries.lock().push((message.into(), payload));
        }

        fn debug(&self, message: &str, payload: Option<LogPayload>) {
            self.entries.lock().push((message.into(), payload));
        }

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

    struct EmptyQuery;

    #[async_trait]
    impl QueryBuilderService for EmptyQuery {
        fn where_filter(&mut self, _: QueryFilter) {}
        fn order_by(&mut self, _: String, _: SortDir) {}
        fn limit(&mut self, _: u64) {}
        fn cursor(&mut self, _: Option<String>) {}

        async fn execute_values(&self) -> Result<Page<Value>, QueryStoreError> {
            Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            })
        }
    }

    #[derive(Default)]
    struct TestQueryStore {
        values: Mutex<HashMap<(String, String), Value>>,
        puts: AtomicUsize,
        fail_put: AtomicBool,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("query put failed")]
    struct QueryPutFailed;

    #[async_trait]
    impl QueryStoreService for TestQueryStore {
        async fn put_value(
            &self,
            collection: &str,
            key: &str,
            value: Value,
        ) -> Result<(), QueryStoreError> {
            self.puts.fetch_add(1, Ordering::Relaxed);
            if self.fail_put.load(Ordering::Acquire) {
                return Err(QueryStoreError::backend(QueryPutFailed));
            }
            self.values
                .lock()
                .insert((collection.into(), key.into()), value);
            Ok(())
        }

        async fn batch(&self, _: &[WriteOp]) -> Result<(), QueryStoreError> {
            Ok(())
        }

        async fn delete(&self, collection: &str, key: &str) -> Result<(), QueryStoreError> {
            self.values
                .lock()
                .remove(&(collection.into(), key.into()));
            Ok(())
        }

        async fn get_value(
            &self,
            collection: &str,
            key: &str,
        ) -> Result<Option<Value>, QueryStoreError> {
            Ok(self
                .values
                .lock()
                .get(&(collection.into(), key.into()))
                .cloned())
        }

        fn query_values(&self, _: &str) -> Box<dyn QueryBuilderService> {
            Box::new(EmptyQuery)
        }

        async fn ensure_index(&self, _: &str, _: &IndexDef) -> Result<(), QueryStoreError> {
            Ok(())
        }

        async fn get_checkpoint(&self, _: &str) -> Result<Option<Checkpoint>, QueryStoreError> {
            Ok(None)
        }

        async fn set_checkpoint(&self, _: &str, _: Checkpoint) -> Result<(), QueryStoreError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), QueryStoreError> {
            Ok(())
        }
    }

    struct Fixture {
        metadata: SessionMetadataService,
        store: Arc<Store>,
        query_store: Arc<TestQueryStore>,
        log: TestLog,
    }

    fn fixture(read_model: bool) -> Fixture {
        let store = Arc::new(Store::default());
        fixture_with_store(store, read_model)
    }

    fn fixture_with_store(store: Arc<Store>, read_model: bool) -> Fixture {
        let context = make_session_context(SessionContextInput {
            session_id: "s".into(),
            workspace_id: "w".into(),
            session_dir: "/s".into(),
            session_scope: "sessions/w/s".into(),
            cwd: "/repo".into(),
            meta_scope: None,
        });
        let query_store = Arc::new(TestQueryStore::default());
        let log = TestLog::default();
        let metadata = SessionMetadataService::new(
            &context,
            AtomicDocumentStoreHandle(store.clone()),
            LogServiceHandle(Arc::new(log.clone())),
            QueryStoreHandle(query_store.clone()),
            FlagServiceHandle(Arc::new(TestFlags { read_model })),
        );
        Fixture {
            metadata,
            store,
            query_store,
            log,
        }
    }

    fn captured_events(
        metadata: &SessionMetadataService,
    ) -> (
        Arc<Mutex<Vec<SessionMetadataChangedEvent>>>,
        DisposableHandle,
    ) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&events);
        let subscription = metadata
            .on_did_change_metadata()
            .subscribe(move |event| target.lock().push(event.clone()));
        (events, subscription)
    }

    fn parse_meta(value: Value) -> Result<SessionMeta, serde_json::Error> {
        session_meta_from_value(value, "s")
    }

    #[tokio::test]
    async fn creates_seeded_document_without_initial_read_model_mirror() {
        let fixture = fixture(true);

        let metadata = fixture.metadata.read().await.unwrap();

        assert_eq!(metadata.id, "s");
        assert_eq!(metadata.version, Some(SESSION_META_VERSION));
        assert_eq!(metadata.cwd.as_deref(), Some("/repo"));
        assert_eq!(metadata.agents, Some(BTreeMap::new()));
        assert_eq!(metadata.custom, Some(BTreeMap::new()));
        assert_eq!(fixture.store.writes.load(Ordering::Relaxed), 1);
        assert_eq!(fixture.query_store.puts.load(Ordering::Relaxed), 0);

        let entries = fixture.log.entries.lock();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "session metadata created");
        let Some(LogPayload::Context(context)) = &entries[0].1 else {
            panic!("creation log payload must be structured context");
        };
        assert_eq!(context["sessionId"], "s");
    }

    #[tokio::test]
    async fn update_persists_mirrors_exact_summary_and_fires_patch_keys() {
        let fixture = fixture(true);
        fixture.metadata.ready().await.unwrap();
        let (events, _subscription) = captured_events(&fixture.metadata);
        let custom = BTreeMap::from([("source".into(), json!("test"))]);

        fixture
            .metadata
            .update(SessionMetaPatch {
                title: Some("Title".into()),
                last_prompt: Some("Prompt".into()),
                archived: Some(true),
                custom: Some(custom.clone()),
                ..Default::default()
            })
            .await
            .unwrap();

        let metadata = fixture.metadata.read().await.unwrap();
        assert_eq!(fixture.store.writes.load(Ordering::Relaxed), 2);
        assert_eq!(fixture.query_store.puts.load(Ordering::Relaxed), 1);
        assert_eq!(
            fixture
                .query_store
                .values
                .lock()
                .get(&("session".into(), "s".into()))
                .cloned(),
            Some(json!({
                "id": "s",
                "workspaceId": "w",
                "cwd": "/repo",
                "title": "Title",
                "lastPrompt": "Prompt",
                "createdAt": metadata.created_at,
                "updatedAt": metadata.updated_at,
                "archived": true,
                "custom": custom,
            }))
        );
        assert_eq!(
            *events.lock(),
            vec![SessionMetadataChangedEvent {
                changed: vec![
                    "title".into(),
                    "lastPrompt".into(),
                    "archived".into(),
                    "custom".into(),
                ],
            }]
        );
    }

    #[tokio::test]
    async fn disabled_read_model_flag_skips_mirroring() {
        let fixture = fixture(false);

        fixture.metadata.set_archived(true).await.unwrap();

        assert_eq!(fixture.query_store.puts.load(Ordering::Relaxed), 0);
        assert!(fixture.query_store.values.lock().is_empty());
    }

    #[tokio::test]
    async fn mirror_failure_is_logged_without_failing_the_durable_update() {
        let fixture = fixture(true);
        fixture.metadata.ready().await.unwrap();
        fixture.query_store.fail_put.store(true, Ordering::Release);
        let (events, _subscription) = captured_events(&fixture.metadata);

        fixture.metadata.set_archived(true).await.unwrap();

        assert!(fixture.metadata.read().await.unwrap().archived);
        let persisted = fixture
            .store
            .values
            .lock()
            .get(&("sessions/w/s".into(), META_KEY.into()))
            .cloned()
            .unwrap();
        assert_eq!(persisted["archived"], true);
        assert_eq!(events.lock().len(), 1);

        let entries = fixture.log.entries.lock();
        let (_, payload) = entries
            .iter()
            .find(|(message, _)| message == "failed to mirror session metadata to read model")
            .expect("mirror failure must be logged");
        let Some(LogPayload::Context(context)) = payload else {
            panic!("mirror warning payload must be structured context");
        };
        assert_eq!(context["sessionId"], "s");
        assert!(
            context["error"]
                .as_str()
                .is_some_and(|error| error.contains("query put failed"))
        );
    }

    #[tokio::test]
    async fn failed_write_keeps_memory_update_and_does_not_poison_update_queue() {
        let fixture = fixture(true);
        fixture.metadata.ready().await.unwrap();
        let (events, _subscription) = captured_events(&fixture.metadata);
        fixture.store.fail_next_write.store(true, Ordering::Release);

        assert!(
            fixture
                .metadata
                .update(SessionMetaPatch {
                    title: Some("Retained in memory".into()),
                    ..Default::default()
                })
                .await
                .is_err()
        );
        assert_eq!(
            fixture.metadata.read().await.unwrap().title.as_deref(),
            Some("Retained in memory")
        );
        assert!(events.lock().is_empty());
        assert_eq!(fixture.query_store.puts.load(Ordering::Relaxed), 0);

        fixture.metadata.set_archived(true).await.unwrap();

        let persisted = fixture
            .store
            .values
            .lock()
            .get(&("sessions/w/s".into(), META_KEY.into()))
            .cloned()
            .unwrap();
        assert_eq!(persisted["title"], "Retained in memory");
        assert_eq!(persisted["archived"], true);
        assert_eq!(
            *events.lock(),
            vec![SessionMetadataChangedEvent {
                changed: vec!["archived".into()],
            }]
        );
    }

    #[tokio::test]
    async fn register_agent_serializes_concurrent_updates_and_equivalent_reregistration_is_noop() {
        let fixture = fixture(true);
        fixture.metadata.ready().await.unwrap();
        let (events, _subscription) = captured_events(&fixture.metadata);
        let main = AgentMeta {
            homedir: Some("/agents/main".into()),
            ..Default::default()
        };
        let sub = AgentMeta {
            parent_agent_id: Some("main".into()),
            ..Default::default()
        };

        let (main_result, sub_result) = tokio::join!(
            fixture.metadata.register_agent("main".into(), main.clone()),
            fixture.metadata.register_agent("sub".into(), sub),
        );
        main_result.unwrap();
        sub_result.unwrap();

        let metadata = fixture.metadata.read().await.unwrap();
        let agents = metadata.agents.unwrap();
        assert_eq!(agents.len(), 2);
        assert!(agents.contains_key("main"));
        assert!(agents.contains_key("sub"));
        assert_eq!(fixture.store.writes.load(Ordering::Relaxed), 3);
        assert_eq!(fixture.query_store.puts.load(Ordering::Relaxed), 2);
        assert_eq!(events.lock().len(), 2);

        let writes = fixture.store.writes.load(Ordering::Relaxed);
        let puts = fixture.query_store.puts.load(Ordering::Relaxed);
        let event_count = events.lock().len();
        fixture
            .metadata
            .register_agent(
                "main".into(),
                AgentMeta {
                    labels: Some(BTreeMap::new()),
                    ..main
                },
            )
            .await
            .unwrap();
        assert_eq!(fixture.store.writes.load(Ordering::Relaxed), writes);
        assert_eq!(fixture.query_store.puts.load(Ordering::Relaxed), puts);
        assert_eq!(events.lock().len(), event_count);
    }

    #[tokio::test]
    async fn load_heals_missing_maps_without_bumping_time_or_mirroring() {
        let store = Arc::new(Store::default());
        store.values.lock().insert(
            ("sessions/w/s".into(), META_KEY.into()),
            json!({
                "id": "s",
                "version": 2,
                "cwd": "/repo",
                "createdAt": 10,
                "updatedAt": 20,
                "archived": false,
            }),
        );
        let fixture = fixture_with_store(store, true);

        let metadata = fixture.metadata.read().await.unwrap();

        assert_eq!(metadata.updated_at, 20);
        assert_eq!(metadata.agents, Some(BTreeMap::new()));
        assert_eq!(metadata.custom, Some(BTreeMap::new()));
        assert_eq!(fixture.store.writes.load(Ordering::Relaxed), 1);
        assert_eq!(fixture.query_store.puts.load(Ordering::Relaxed), 0);
        assert!(fixture.log.entries.lock().is_empty());
    }

    #[tokio::test]
    async fn load_accepts_legacy_typescript_metadata_without_id() {
        let store = Arc::new(Store::default());
        store.values.lock().insert(
            ("sessions/w/s".into(), META_KEY.into()),
            json!({
                "title": "Legacy TypeScript session",
                "createdAt": "2024-01-02T03:04:05Z",
                "updatedAt": "2024-01-03T03:04:05Z",
                "archived": false,
                "workDir": "/legacy",
                "agents": {},
                "custom": {},
            }),
        );
        let fixture = fixture_with_store(store, false);

        let metadata = fixture.metadata.read().await.unwrap();

        assert_eq!(metadata.id, "s");
        assert_eq!(metadata.version, Some(SESSION_META_VERSION));
        assert_eq!(metadata.cwd.as_deref(), Some("/legacy"));
        assert_eq!(metadata.created_at, 1_704_164_645_000);
        assert_eq!(metadata.updated_at, 1_704_251_045_000);
        assert_eq!(fixture.store.writes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn normalizes_legacy_version_dates_and_work_dir() {
        let raw = parse_meta(json!({
            "id": "old",
            "createdAt": "2024-01-02T03:04:05Z",
            "updatedAt": "invalid",
            "archived": false,
            "workDir": "/legacy",
        }))
        .unwrap();
        let normalized = normalize_session_meta(raw, "new");
        assert_eq!(normalized.id, "new");
        assert_eq!(normalized.version, Some(2));
        assert_eq!(normalized.cwd.as_deref(), Some("/legacy"));
        assert_eq!(normalized.created_at, 1_704_164_645_000);
        assert_eq!(normalized.updated_at, 0);
    }

    #[test]
    fn current_cwd_wins_over_legacy_work_dir_and_empty_legacy_value_is_ignored() {
        let current = parse_meta(json!({
            "id": "s",
            "version": 2,
            "createdAt": 1,
            "updatedAt": 2,
            "archived": false,
            "cwd": "/current",
            "workDir": "/legacy",
        }))
        .unwrap();
        assert_eq!(current.cwd.as_deref(), Some("/current"));

        let null_current = parse_meta(json!({
            "id": "s",
            "version": 2,
            "createdAt": 1,
            "updatedAt": 2,
            "archived": false,
            "cwd": null,
            "workDir": "/legacy",
        }))
        .unwrap();
        assert_eq!(null_current.cwd.as_deref(), Some("/legacy"));

        let empty_legacy = parse_meta(json!({
            "id": "s",
            "version": 2,
            "createdAt": 1,
            "updatedAt": 2,
            "archived": false,
            "workDir": "",
        }))
        .unwrap();
        assert_eq!(empty_legacy.cwd, None);
    }
}
