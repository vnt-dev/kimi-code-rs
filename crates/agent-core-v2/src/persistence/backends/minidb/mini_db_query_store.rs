//! `kimi-code-minidb` implementation of the derived query store.
//!
//! Original: `packages/agent-core-v2/src/persistence/backends/minidb/miniDbQueryStore.ts`.

use parking_lot::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{
    collections::HashSet,
    future::Future,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use kimi_code_minidb::{
    MiniDb, MiniDbError,
    cluster::{
        index::{ClusterDb, ClusterError},
        topology::TopologyError,
        types::ClusterOpenOptions,
    },
    codec::CodecError,
    compound_index::{CompoundIndexDef as MiniDbCompoundIndexDef, CompoundIndexError, OrderType},
    index_manager::{IndexDef as MiniDbIndexDef, IndexError, IndexType},
    minidb::{BatchInputOp, KeyQuery, QueryOptions, SetOptions, ValueModeSetting},
    recovery::RecoveryError,
};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        log::{LOG_SERVICE_ID, LogPayload},
    },
    app::bootstrap::BOOTSTRAP_SERVICE_ID,
    persistence::interface::query_store::{
        Checkpoint, IndexDef, Page, QUERY_STORE_SERVICE_ID, QueryBuilderService, QueryFilter,
        QueryStoreError, QueryStoreHandle, QueryStoreService, SortDir, WriteOp,
    },
};

const SEPARATOR: char = '\0';
const CHECKPOINT_COLLECTION: &str = "__checkpoint__";
const STORE_SUBDIRECTORY: &str = "query-store";
const SHARD_COUNT: usize = 16;
const LOCK_ACQUIRE_TIMEOUT_MILLIS: u64 = 1_000;

type RebuildLogger = Arc<dyn Fn(&Path, &str) + Send + Sync>;

#[derive(Clone)]
pub struct MiniDbQueryStore {
    inner: Arc<MiniDbQueryStoreInner>,
}

struct MiniDbQueryStoreInner {
    directory: PathBuf,
    database: AsyncMutex<Option<Arc<ClusterDb<Value>>>>,
    rebuild_gate: AsyncMutex<()>,
    rebuilt: AtomicBool,
    ensured_indexes: Mutex<HashSet<String>>,
    rebuild_logger: Option<RebuildLogger>,
}

impl MiniDbQueryStore {
    pub fn new(cache_directory: impl AsRef<Path>) -> Self {
        Self::new_with_rebuild_logger(cache_directory, None)
    }

    pub fn new_with_rebuild_logger(
        cache_directory: impl AsRef<Path>,
        rebuild_logger: Option<RebuildLogger>,
    ) -> Self {
        Self {
            inner: Arc::new(MiniDbQueryStoreInner {
                directory: cache_directory.as_ref().join(STORE_SUBDIRECTORY),
                database: AsyncMutex::new(None),
                rebuild_gate: AsyncMutex::new(()),
                rebuilt: AtomicBool::new(false),
                ensured_indexes: Mutex::new(HashSet::new()),
                rebuild_logger,
            }),
        }
    }

    // Original: openDb() + openFresh(). Construction performs no filesystem I/O.
    async fn open_database(&self) -> Result<Arc<ClusterDb<Value>>, ClusterError> {
        let _rebuild = self.inner.rebuild_gate.lock().await;
        let mut database = self.inner.database.lock().await;
        if let Some(database) = database.as_ref() {
            return Ok(Arc::clone(database));
        }
        let mut base = MiniDb::json_options(&self.inner.directory);
        base.value_mode = ValueModeSetting::Memory;
        let mut options = ClusterOpenOptions::new(base);
        options.shard_count = Some(SHARD_COUNT);
        options.lock_acquire_timeout_millis = LOCK_ACQUIRE_TIMEOUT_MILLIS;
        let opened = Arc::new(ClusterDb::open(options).await?);
        *database = Some(Arc::clone(&opened));
        Ok(opened)
    }

    // Original: rebuild(). Concurrent callers share the same gate. A caller
    // whose failed instance has already been replaced retries without wiping.
    async fn rebuild_if_current(
        &self,
        failed_database: &Arc<ClusterDb<Value>>,
        cause: &ClusterError,
    ) -> Result<bool, ClusterError> {
        let _rebuild = self.inner.rebuild_gate.lock().await;
        let previous = {
            let mut database = self.inner.database.lock().await;
            match database.as_ref() {
                Some(current) if !Arc::ptr_eq(current, failed_database) => return Ok(true),
                _ if self.inner.rebuilt.swap(true, Ordering::AcqRel) => return Ok(false),
                _ => database.take(),
            }
        };
        if let Some(logger) = &self.inner.rebuild_logger {
            logger(&self.inner.directory, &cause.to_string());
        }
        self.inner.ensured_indexes.lock().clear();
        if let Some(previous) = previous {
            let _ = previous.close().await;
        }
        match tokio::fs::remove_dir_all(&self.inner.directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ClusterError::Io(error)),
        }
        Ok(true)
    }

    // Original: withDb<T>(). Only JSON/frame corruption is rebuildable; lock
    // and all other operational errors propagate unchanged.
    async fn with_database<T, F, Fut>(&self, operation: F) -> Result<T, QueryStoreError>
    where
        F: Fn(Arc<ClusterDb<Value>>) -> Fut,
        Fut: Future<Output = Result<T, ClusterError>>,
    {
        let database = self
            .open_database()
            .await
            .map_err(QueryStoreError::backend)?;
        match operation(Arc::clone(&database)).await {
            Ok(value) => Ok(value),
            Err(error) if is_rebuildable(&error) => {
                if !self
                    .rebuild_if_current(&database, &error)
                    .await
                    .map_err(QueryStoreError::backend)?
                {
                    return Err(QueryStoreError::backend(error));
                }
                let database = self
                    .open_database()
                    .await
                    .map_err(QueryStoreError::backend)?;
                operation(database).await.map_err(QueryStoreError::backend)
            }
            Err(error) => Err(QueryStoreError::backend(error)),
        }
    }
}

/// Registers the App-scoped MiniDB derived query store.
///
/// The rebuild callback preserves the TypeScript service's structured warning
/// through the injected application logger.
pub fn register_mini_db_query_store() {
    super::flag::register_persistence_minidb_read_model_flag();
    register_scoped_service(
        LifecycleScope::App,
        QUERY_STORE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let log = log.clone();
            let logger: RebuildLogger = Arc::new(move |directory, error| {
                log.0.warn(
                    "minidb query-store rebuilt after corruption",
                    Some(LogPayload::Context(serde_json::Map::from_iter([
                        (
                            "dir".into(),
                            Value::String(directory.to_string_lossy().into_owned()),
                        ),
                        ("error".into(), Value::String(error.into())),
                    ]))),
                );
            });
            let service: Arc<dyn QueryStoreService> = Arc::new(
                MiniDbQueryStore::new_with_rebuild_logger(bootstrap.cache_dir(), Some(logger)),
            );
            Ok(QueryStoreHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "storage",
    );
}

#[async_trait]
impl QueryStoreService for MiniDbQueryStore {
    async fn put_value(
        &self,
        collection: &str,
        key: &str,
        value: Value,
    ) -> Result<(), QueryStoreError> {
        let key = physical_key(collection, key);
        self.with_database(move |database| {
            let key = key.clone();
            let value = value.clone();
            async move { database.set(&key, value, SetOptions::default()).await }
        })
        .await
    }

    async fn batch(&self, operations: &[WriteOp]) -> Result<(), QueryStoreError> {
        if operations.is_empty() {
            return Ok(());
        }
        let operations = operations.to_vec();
        self.with_database(move |database| {
            let operations = operations
                .iter()
                .map(|operation| match operation {
                    WriteOp::Put {
                        collection,
                        key,
                        value,
                    } => BatchInputOp::Set {
                        key: physical_key(collection, key),
                        value: value.clone(),
                        options: SetOptions::default(),
                    },
                    WriteOp::Delete { collection, key } => BatchInputOp::Del {
                        key: physical_key(collection, key),
                    },
                })
                .collect();
            async move { database.batch(operations).await }
        })
        .await
    }

    async fn delete(&self, collection: &str, key: &str) -> Result<(), QueryStoreError> {
        let key = physical_key(collection, key);
        self.with_database(move |database| {
            let key = key.clone();
            async move { database.del(&key).await.map(|_| ()) }
        })
        .await
    }

    async fn get_value(
        &self,
        collection: &str,
        key: &str,
    ) -> Result<Option<Value>, QueryStoreError> {
        let key = physical_key(collection, key);
        self.with_database(move |database| {
            let key = key.clone();
            async move { database.get(&key).await }
        })
        .await
    }

    fn query_values(&self, collection: &str) -> Box<dyn QueryBuilderService> {
        Box::new(MiniDbQuery {
            store: self.clone(),
            collection: collection.into(),
            filter: QueryFilter::new(),
            sort_field: None,
            sort_direction: SortDir::Asc,
            limit: None,
            skip: 0,
        })
    }

    async fn ensure_index(
        &self,
        collection: &str,
        definition: &IndexDef,
    ) -> Result<(), QueryStoreError> {
        let (kind, unprefixed_name) = match definition {
            IndexDef::Value { name, .. } => ("value", name),
            IndexDef::Compound { name, .. } => ("compound", name),
            IndexDef::Text { name, .. } => ("text", name),
        };
        let guard = format!("{collection}:{kind}:{unprefixed_name}");
        if self.inner.ensured_indexes.lock().contains(&guard) {
            return Ok(());
        }
        let name = index_name(collection, unprefixed_name);
        let definition = definition.clone();
        self.with_database(move |database| {
            let name = name.clone();
            let definition = definition.clone();
            async move {
                let result = match definition {
                    IndexDef::Value { field, unique, .. } => {
                        database
                            .create_index(
                                &name,
                                MiniDbIndexDef {
                                    field,
                                    index_type: IndexType::Equality,
                                    unique: unique.unwrap_or(false),
                                    sparse: true,
                                },
                            )
                            .await
                    }
                    IndexDef::Compound {
                        group_by, order_by, ..
                    } => {
                        database
                            .create_compound_index(
                                &name,
                                MiniDbCompoundIndexDef {
                                    group_by,
                                    order_by,
                                    order_type: OrderType::Number,
                                },
                            )
                            .await
                    }
                    IndexDef::Text { fields, .. } => {
                        database.create_text_index(&name, fields).await
                    }
                };
                match result {
                    Err(error) if index_already_exists(&error) => Ok(()),
                    other => other,
                }
            }
        })
        .await?;
        self.inner.ensured_indexes.lock().insert(guard);
        Ok(())
    }

    async fn get_checkpoint(&self, source: &str) -> Result<Option<Checkpoint>, QueryStoreError> {
        self.get_value(CHECKPOINT_COLLECTION, source)
            .await?
            .map(serde_json::from_value)
            .transpose()
            .map_err(QueryStoreError::Codec)
    }

    async fn set_checkpoint(
        &self,
        source: &str,
        checkpoint: Checkpoint,
    ) -> Result<(), QueryStoreError> {
        self.put_value(
            CHECKPOINT_COLLECTION,
            source,
            serde_json::to_value(checkpoint).map_err(QueryStoreError::Codec)?,
        )
        .await
    }

    async fn close(&self) -> Result<(), QueryStoreError> {
        let database = self.inner.database.lock().await.clone();
        if let Some(database) = database {
            database.close().await.map_err(QueryStoreError::backend)?;
        }
        Ok(())
    }
}

fn index_already_exists(error: &ClusterError) -> bool {
    matches!(
        error,
        ClusterError::Database(MiniDbError::Index(IndexError::AlreadyExists(_)))
            | ClusterError::Database(MiniDbError::Compound(CompoundIndexError::AlreadyExists(_)))
            | ClusterError::Database(MiniDbError::TextIndexExists(_))
    )
}

struct MiniDbQuery {
    store: MiniDbQueryStore,
    collection: String,
    filter: QueryFilter,
    sort_field: Option<String>,
    sort_direction: SortDir,
    limit: Option<usize>,
    skip: usize,
}

#[async_trait]
impl QueryBuilderService for MiniDbQuery {
    fn where_filter(&mut self, filter: QueryFilter) {
        self.filter.extend(filter);
    }

    fn order_by(&mut self, field: String, direction: SortDir) {
        self.sort_field = Some(field);
        self.sort_direction = direction;
    }

    fn limit(&mut self, limit: u64) {
        self.limit = Some(usize::try_from(limit).unwrap_or(usize::MAX));
    }

    fn cursor(&mut self, cursor: Option<String>) {
        self.skip = cursor
            .filter(|cursor| !cursor.is_empty())
            .and_then(|cursor| cursor.parse().ok())
            .unwrap_or(0);
    }

    async fn execute_values(&self) -> Result<Page<Value>, QueryStoreError> {
        let prefix = format!("{}{SEPARATOR}", self.collection);
        let mut query = QueryOptions {
            key: Some(KeyQuery {
                prefix: Some(prefix),
                ..KeyQuery::default()
            }),
            skip: self.skip,
            ..QueryOptions::default()
        };
        if !self.filter.is_empty() {
            query.filter = Some(Value::Object(self.filter.clone()));
        }
        if let Some(field) = &self.sort_field {
            query.sort.push((
                field.clone(),
                if self.sort_direction == SortDir::Desc {
                    -1
                } else {
                    1
                },
            ));
        }
        if let Some(limit) = self.limit {
            query.limit = Some(limit.saturating_add(1));
        }
        let query = Arc::new(query);
        let rows = self
            .store
            .with_database(move |database| {
                let query = Arc::clone(&query);
                async move { database.query(&query).await }
            })
            .await?;
        let mut items = rows.into_iter().map(|row| row.value).collect::<Vec<_>>();
        let next_cursor = self.limit.and_then(|limit| {
            (items.len() > limit).then(|| {
                items.truncate(limit);
                self.skip.saturating_add(limit).to_string()
            })
        });
        Ok(Page { items, next_cursor })
    }
}

fn physical_key(collection: &str, key: &str) -> String {
    format!("{collection}{SEPARATOR}{key}")
}

fn index_name(collection: &str, name: &str) -> String {
    format!("{collection}:{name}")
}

fn is_rebuildable(error: &ClusterError) -> bool {
    matches!(
        error,
        ClusterError::Json(_)
            | ClusterError::Topology(TopologyError::Json(_))
            | ClusterError::Database(MiniDbError::Json(_))
            | ClusterError::Database(MiniDbError::FrameCodec(CodecError::CorruptFrame { .. }))
            | ClusterError::Database(MiniDbError::Recovery(RecoveryError::Metadata(_)))
            | ClusterError::Database(MiniDbError::Recovery(RecoveryError::Codec(
                CodecError::CorruptFrame { .. }
            )))
    )
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    use super::*;
    use crate::persistence::interface::query_store::{QueryStoreHandle, WriteOp};

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Row {
        name: String,
        score: u8,
    }

    fn temporary_cache() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-query-store-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn opens_lazily_and_preserves_crud_batch_collection_and_checkpoint_contracts() {
        let cache = temporary_cache();
        let backend = MiniDbQueryStore::new(&cache);
        assert!(!cache.join(STORE_SUBDIRECTORY).exists());
        let store = QueryStoreHandle(Arc::new(backend.clone()));

        store
            .put(
                "sessions",
                "one",
                &Row {
                    name: "one".into(),
                    score: 1,
                },
            )
            .await
            .unwrap();
        assert!(cache.join(STORE_SUBDIRECTORY).exists());
        assert_eq!(
            store.get::<Row>("sessions", "one").await.unwrap(),
            Some(Row {
                name: "one".into(),
                score: 1
            })
        );
        assert_eq!(store.get::<Row>("other", "one").await.unwrap(), None);

        backend
            .batch(&[
                WriteOp::Put {
                    collection: "sessions".into(),
                    key: "two".into(),
                    value: serde_json::json!({"name": "two", "score": 2}),
                },
                WriteOp::Delete {
                    collection: "sessions".into(),
                    key: "one".into(),
                },
            ])
            .await
            .unwrap();
        assert_eq!(store.get::<Row>("sessions", "one").await.unwrap(), None);
        backend
            .set_checkpoint("wire", Checkpoint { seq: 42 })
            .await
            .unwrap();
        assert_eq!(
            backend.get_checkpoint("wire").await.unwrap(),
            Some(Checkpoint { seq: 42 })
        );
        backend.close().await.unwrap();
        tokio::fs::remove_dir_all(cache).await.unwrap();
    }

    #[tokio::test]
    async fn queries_filter_sort_and_page_and_indexes_are_idempotent() {
        let cache = temporary_cache();
        let backend = MiniDbQueryStore::new(&cache);
        let store = QueryStoreHandle(Arc::new(backend.clone()));
        for (key, name, score) in [("a", "alpha", 1), ("b", "beta", 3), ("c", "gamma", 2)] {
            store
                .put(
                    "rows",
                    key,
                    &Row {
                        name: name.into(),
                        score,
                    },
                )
                .await
                .unwrap();
        }
        let index = IndexDef::Value {
            name: "by_name".into(),
            field: "name".into(),
            unique: None,
        };
        backend.ensure_index("rows", &index).await.unwrap();
        backend.ensure_index("rows", &index).await.unwrap();
        backend
            .ensure_index(
                "rows",
                &IndexDef::Compound {
                    name: "by_name_score".into(),
                    group_by: "name".into(),
                    order_by: "score".into(),
                },
            )
            .await
            .unwrap();
        let text_index = IndexDef::Text {
            name: "name_text".into(),
            fields: Some(vec!["name".into()]),
        };
        backend.ensure_index("rows", &text_index).await.unwrap();
        backend.ensure_index("rows", &text_index).await.unwrap();

        let mut query = store.query::<Row>("rows");
        query.order_by("score", SortDir::Desc).limit(2);
        let first = query.execute().await.unwrap();
        assert_eq!(
            first.items.iter().map(|row| row.score).collect::<Vec<_>>(),
            [3, 2]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("2"));

        let mut next = store.query::<Row>("rows");
        next.order_by("score", SortDir::Desc)
            .limit(2)
            .cursor(first.next_cursor);
        assert_eq!(
            next.execute().await.unwrap().items,
            [Row {
                name: "alpha".into(),
                score: 1
            }]
        );

        let mut filtered = store.query::<Row>("rows");
        filtered.where_filter(QueryFilter::from_iter([(
            "score".into(),
            serde_json::json!({"$gte": 2}),
        )]));
        assert_eq!(filtered.execute().await.unwrap().items.len(), 2);
        backend.close().await.unwrap();
        tokio::fs::remove_dir_all(cache).await.unwrap();
    }

    #[tokio::test]
    async fn corrupt_registry_is_wiped_once_and_operation_retries_on_fresh_cluster() {
        let cache = temporary_cache();
        let first = MiniDbQueryStore::new(&cache);
        first
            .put_value("rows", "old", serde_json::json!({"value": 1}))
            .await
            .unwrap();
        let index = IndexDef::Value {
            name: "by_value".into(),
            field: "value".into(),
            unique: None,
        };
        first.ensure_index("rows", &index).await.unwrap();
        first.close().await.unwrap();

        tokio::fs::write(
            cache.join(STORE_SUBDIRECTORY).join("cluster.indexes.json"),
            b"{ definitely not valid json",
        )
        .await
        .unwrap();
        let rebuilt = MiniDbQueryStore::new(&cache);
        rebuilt.ensure_index("rows", &index).await.unwrap();
        assert_eq!(rebuilt.get_value("rows", "old").await.unwrap(), None);
        rebuilt
            .put_value("rows", "new", serde_json::json!({"value": 2}))
            .await
            .unwrap();
        assert_eq!(
            rebuilt.get_value("rows", "new").await.unwrap(),
            Some(serde_json::json!({"value": 2}))
        );
        rebuilt.close().await.unwrap();
        tokio::fs::remove_dir_all(cache).await.unwrap();
    }
}
