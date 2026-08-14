use std::{
    cmp::Ordering,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    compound_index::{CompoundIndexDef, CompoundIndexInfo},
    index_manager::{IndexDef, IndexInfo, NumericRangeOptions},
    lockfile::{LockError, LockFile},
    minidb::{
        BatchInputOp, CodecName, DocumentRecord, IndexedDocumentRecord, MiniDb, MiniDbError,
        QueryOptions, SearchDocumentRecord, SetOptions, ValueCodec,
    },
    skiplist::RangeOptions,
    text_index::SearchOptions,
};

use super::{
    coordinator::Coordinator,
    lock_pool::{LockPoolError, LockPoolOptions, ShardLockPool},
    router::Router,
    topology::{Topology, TopologyError},
    types::{
        ClusterIndexRegistry, ClusterOpenOptions, ClusterStats, CompactResult, CrossShardMode,
        NamedCompoundDefinition, NamedIndexDefinition, NamedTextDefinition, ScanOptions,
    },
    utils::{CLUSTER_INDEX_FILE, InvalidShardCount},
};

#[derive(Debug, Error)]
pub enum ClusterError {
    #[error("ClusterDb is closed")]
    Closed,
    #[error("ClusterDb is open in read-only mode")]
    ReadOnly,
    #[error("cross_shard: '2pc' is reserved for a future release and is not implemented yet")]
    TwoPhaseCommit,
    #[error("{0}")]
    Coordination(String),
    #[error("{operation} failed on {failed}/{total} shard(s); partial writes possible: {errors:?}")]
    PartialWrites {
        operation: &'static str,
        failed: usize,
        total: usize,
        errors: Vec<String>,
    },
    #[error("cluster index registry update timed out")]
    RegistryTimeout,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Topology(#[from] TopologyError),
    #[error(transparent)]
    Pool(#[from] LockPoolError),
    #[error(transparent)]
    Database(#[from] MiniDbError),
    #[error(transparent)]
    ShardCount(#[from] InvalidShardCount),
    #[error(transparent)]
    Lock(#[from] LockError),
}

pub struct ClusterDb<V> {
    topology: Topology,
    router: Router,
    pool: Arc<ShardLockPool<V>>,
    coordinator: Coordinator,
    index_path: PathBuf,
    codec: Arc<dyn ValueCodec<V>>,
    read_only: bool,
    registry_timeout: Duration,
    closed: std::sync::atomic::AtomicBool,
}

impl<V: Send + Sync + 'static> ClusterDb<V> {
    // Original: packages/minidb/src/cluster/index.ts, ClusterDb.open().
    pub async fn open(options: ClusterOpenOptions<V>) -> Result<Self, ClusterError> {
        if options.cross_shard == CrossShardMode::TwoPhaseCommit {
            return Err(ClusterError::TwoPhaseCommit);
        }
        let topology = Topology::open(
            &options.base.directory,
            options.shard_count,
            options.base.codec.name(),
            options.base.fsync_policy,
        )
        .await?;
        topology.ensure_shard_directories().await?;
        let router = Router::new(&topology.directory, topology.meta.clone());
        let shard_options = super::shard::ShardOpenOptions::from_open(&options.base);
        let pool = Arc::new(ShardLockPool::new(LockPoolOptions {
            writer_options: shard_options.clone(),
            reader_options: shard_options,
            lock_renew_millis: options.lock_renew_millis,
            lock_acquire_timeout_millis: options.lock_acquire_timeout_millis,
            lock_hold_millis: options.lock_hold_millis,
            max_writers: options.max_writers,
            max_readers: options.max_readers.unwrap_or(topology.shard_count()),
            read_only: options.read_only,
        }));
        Ok(Self {
            index_path: topology.directory.join(CLUSTER_INDEX_FILE),
            router,
            coordinator: Coordinator::new(options.cross_shard),
            codec: Arc::clone(&options.base.codec),
            topology,
            pool,
            read_only: options.read_only,
            registry_timeout: Duration::from_millis(options.lock_acquire_timeout_millis),
            closed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn ensure_open(&self) -> Result<(), ClusterError> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            Err(ClusterError::Closed)
        } else {
            Ok(())
        }
    }
    fn ensure_writable(&self) -> Result<(), ClusterError> {
        if self.read_only {
            Err(ClusterError::ReadOnly)
        } else {
            Ok(())
        }
    }
    fn require_json_codec(&self, kind: &'static str) -> Result<(), ClusterError> {
        if self.topology.meta.value_codec == CodecName::Json {
            Ok(())
        } else {
            Err(ClusterError::Database(MiniDbError::JsonCodecRequired(kind)))
        }
    }

    pub fn directory(&self) -> &Path {
        &self.topology.directory
    }
    pub fn shard_count(&self) -> usize {
        self.router.shard_count()
    }
    pub fn shard_of(&self, key: &str) -> Result<usize, ClusterError> {
        Ok(self.router.shard_for(key)?)
    }

    async fn writer(
        &self,
        shard_id: usize,
    ) -> Result<Arc<super::shard::ShardHandle<V>>, ClusterError> {
        self.ensure_open()?;
        self.ensure_writable()?;
        let handle = self
            .pool
            .writer(shard_id, self.router.shard_directory(shard_id))
            .await?;
        if let Err(error) = self.apply_registry(&handle.database).await {
            self.pool.invalidate_writer(shard_id).await?;
            return Err(error);
        }
        Ok(handle)
    }

    async fn reader(
        &self,
        shard_id: usize,
    ) -> Result<Arc<super::shard::ShardHandle<V>>, ClusterError> {
        self.ensure_open()?;
        Ok(self
            .pool
            .reader(shard_id, self.router.shard_directory(shard_id))
            .await?)
    }

    async fn apply_registry(&self, database: &MiniDb<V>) -> Result<(), ClusterError> {
        let registry = load_registry(&self.index_path).await?;
        for definition in registry.indexes {
            if !database
                .list_indexes()?
                .iter()
                .any(|index| index.name == definition.name)
            {
                database
                    .create_index(&definition.name, definition.def)
                    .await?;
            }
        }
        for definition in registry.compound_indexes {
            if !database
                .list_compound_indexes()?
                .iter()
                .any(|index| index.name == definition.name)
            {
                database
                    .create_compound_index(&definition.name, definition.def)
                    .await?;
            }
        }
        for definition in registry.text_indexes {
            if !database
                .list_text_indexes()?
                .iter()
                .any(|(name, _)| name == &definition.name)
            {
                database
                    .create_text_index(&definition.name, definition.fields)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Option<V>, ClusterError> {
        Ok(self
            .reader(self.shard_of(key)?)
            .await?
            .database
            .get(key.as_bytes())?)
    }
    pub async fn set(&self, key: &str, value: V, options: SetOptions) -> Result<(), ClusterError> {
        self.writer(self.shard_of(key)?)
            .await?
            .database
            .set(key.as_bytes(), value, options)
            .await?;
        Ok(())
    }
    pub async fn del(&self, key: &str) -> Result<bool, ClusterError> {
        Ok(self
            .writer(self.shard_of(key)?)
            .await?
            .database
            .del(key.as_bytes())
            .await?)
    }
    pub async fn has(&self, key: &str) -> Result<bool, ClusterError> {
        Ok(self
            .reader(self.shard_of(key)?)
            .await?
            .database
            .has(key.as_bytes())?)
    }
    pub async fn ttl(&self, key: &str) -> Result<i64, ClusterError> {
        Ok(self
            .reader(self.shard_of(key)?)
            .await?
            .database
            .ttl(key.as_bytes())?)
    }
    pub async fn expire(&self, key: &str, ttl_millis: u64) -> Result<bool, ClusterError> {
        Ok(self
            .writer(self.shard_of(key)?)
            .await?
            .database
            .expire(key.as_bytes(), ttl_millis)
            .await?)
    }

    pub async fn mget(&self, keys: &[String]) -> Result<Vec<Option<V>>, ClusterError> {
        let mut output = (0..keys.len()).map(|_| None).collect::<Vec<_>>();
        let mut groups = std::collections::BTreeMap::<usize, Vec<(usize, &String)>>::new();
        for (index, key) in keys.iter().enumerate() {
            groups
                .entry(self.shard_of(key)?)
                .or_default()
                .push((index, key));
        }
        for (shard, entries) in groups {
            let handle = self.reader(shard).await?;
            for (index, key) in entries {
                output[index] = handle.database.get(key.as_bytes())?;
            }
        }
        Ok(output)
    }

    pub async fn mset(&self, entries: Vec<(String, V)>) -> Result<(), ClusterError> {
        self.ensure_writable()?;
        let groups = self.coordinator.group_entries(&self.router, entries)?;
        self.coordinator
            .check_mode(groups.len())
            .map_err(ClusterError::Coordination)?;
        let total = groups.len();
        let mut errors = Vec::new();
        for (shard, entries) in groups {
            match self.writer(shard).await {
                Ok(handle) => {
                    if let Err(error) = handle.database.mset(entries).await {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        partial_result("mset", total, errors)
    }

    pub async fn mdel(&self, keys: Vec<String>) -> Result<usize, ClusterError> {
        self.ensure_writable()?;
        let groups = self.coordinator.group_keys(&self.router, keys)?;
        self.coordinator
            .check_mode(groups.len())
            .map_err(ClusterError::Coordination)?;
        let total = groups.len();
        let mut errors = Vec::new();
        let mut removed = 0;
        for (shard, keys) in groups {
            match self.writer(shard).await {
                Ok(handle) => {
                    let mut operations = Vec::new();
                    for key in keys {
                        if handle.database.has(key.as_bytes())? {
                            operations.push(BatchInputOp::Del { key });
                            removed += 1;
                        }
                    }
                    if let Err(error) = handle.database.batch(operations).await {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        partial_result("mdel", total, errors)?;
        Ok(removed)
    }

    pub async fn batch(&self, operations: Vec<BatchInputOp<V>>) -> Result<(), ClusterError> {
        self.ensure_writable()?;
        let groups = self
            .coordinator
            .group_operations(&self.router, operations)?;
        self.coordinator
            .check_mode(groups.len())
            .map_err(ClusterError::Coordination)?;
        let total = groups.len();
        let mut errors = Vec::new();
        for (shard, operations) in groups {
            match self.writer(shard).await {
                Ok(handle) => {
                    if let Err(error) = handle.database.batch(operations).await {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        partial_result("batch", total, errors)
    }

    pub async fn scan(
        &self,
        options: &ScanOptions,
    ) -> Result<Vec<DocumentRecord<V>>, ClusterError> {
        let mut all = Vec::new();
        let per_shard = if options.reverse { None } else { options.limit };
        for shard in self.router.shard_ids() {
            let handle = self.reader(shard).await?;
            let mut rows = if let Some(prefix) = &options.prefix {
                handle
                    .database
                    .prefix(prefix.as_bytes(), per_shard.unwrap_or(usize::MAX))?
            } else {
                handle.database.scan(&RangeOptions {
                    gte: options.gte.as_ref().map(|v| v.as_bytes().to_vec()),
                    gt: options.gt.as_ref().map(|v| v.as_bytes().to_vec()),
                    lte: options.lte.as_ref().map(|v| v.as_bytes().to_vec()),
                    lt: options.lt.as_ref().map(|v| v.as_bytes().to_vec()),
                    count: per_shard,
                    reverse: false,
                    offset: 0,
                })?
            };
            all.append(&mut rows);
        }
        all.sort_by(|left, right| left.key.as_bytes().cmp(right.key.as_bytes()));
        if options.reverse {
            all.reverse();
        }
        if let Some(limit) = options.limit {
            all.truncate(limit);
        }
        Ok(all)
    }

    pub async fn prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<DocumentRecord<V>>, ClusterError> {
        self.scan(&ScanOptions {
            prefix: Some(prefix.into()),
            limit: Some(limit),
            ..Default::default()
        })
        .await
    }

    pub async fn query(
        &self,
        options: &QueryOptions,
    ) -> Result<Vec<DocumentRecord<V>>, ClusterError> {
        let needed = options
            .limit
            .map(|limit| options.skip.saturating_add(limit));
        let mut local = options.clone();
        local.skip = 0;
        local.limit = needed;
        let mut all = Vec::new();
        for shard in self.router.shard_ids() {
            all.extend(self.reader(shard).await?.database.query(&local)?);
        }
        if options.sort.is_empty() {
            all.sort_by(|left, right| left.key.as_bytes().cmp(right.key.as_bytes()));
        } else {
            all.sort_by(|left, right| compare_documents(&*self.codec, left, right, &options.sort));
        }
        let start = options.skip.min(all.len());
        let end = options.limit.map_or(all.len(), |limit| {
            start.saturating_add(limit).min(all.len())
        });
        Ok(all.drain(start..end).collect())
    }

    pub async fn create_index(&self, name: &str, definition: IndexDef) -> Result<(), ClusterError> {
        self.require_json_codec("secondary")?;
        self.ensure_writable()?;
        if load_registry(&self.index_path)
            .await?
            .indexes
            .iter()
            .any(|item| item.name == name)
        {
            return Err(index_already("index", name));
        }
        let mut created = Vec::new();
        for shard in self.router.shard_ids() {
            let handle = self.writer(shard).await?;
            if !handle
                .database
                .list_indexes()?
                .iter()
                .any(|index| index.name == name)
            {
                if let Err(error) = handle.database.create_index(name, definition.clone()).await {
                    for shard in created {
                        if let Ok(handle) = self.writer(shard).await {
                            let _ = handle.database.drop_index(name).await;
                        }
                    }
                    return Err(error.into());
                }
                created.push(shard);
            }
        }
        self.mutate_registry(|registry| {
            if let Some(existing) = registry.indexes.iter().find(|item| item.name == name) {
                return if existing.def == definition {
                    Ok(())
                } else {
                    Err(index_already("index", name))
                };
            }
            registry.indexes.push(NamedIndexDefinition {
                name: name.into(),
                def: definition,
            });
            Ok(())
        })
        .await
    }

    pub async fn drop_index(&self, name: &str) -> Result<bool, ClusterError> {
        self.ensure_writable()?;
        let existed = load_registry(&self.index_path)
            .await?
            .indexes
            .iter()
            .any(|item| item.name == name);
        for shard in self.router.shard_ids() {
            let handle = self.writer(shard).await?;
            if handle
                .database
                .list_indexes()?
                .iter()
                .any(|index| index.name == name)
            {
                handle.database.drop_index(name).await?;
            }
        }
        if existed {
            self.mutate_registry(|registry| {
                registry.indexes.retain(|item| item.name != name);
                Ok(())
            })
            .await?;
        }
        Ok(existed)
    }
    pub async fn list_indexes(&self) -> Result<Vec<IndexInfo>, ClusterError> {
        Ok(load_registry(&self.index_path)
            .await?
            .indexes
            .into_iter()
            .map(|item| IndexInfo {
                name: item.name,
                field: item.def.field,
                index_type: item.def.index_type,
                unique: item.def.unique,
                sparse: item.def.sparse,
            })
            .collect())
    }
    pub async fn find_eq(
        &self,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<Vec<DocumentRecord<V>>, ClusterError> {
        if !load_registry(&self.index_path)
            .await?
            .indexes
            .iter()
            .any(|item| item.name == name)
        {
            return Err(index_missing("index", name));
        }
        let mut rows = Vec::new();
        for shard in self.router.shard_ids() {
            rows.extend(self.reader(shard).await?.database.find_eq(name, value)?);
        }
        rows.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
        Ok(rows)
    }

    pub async fn find_range(
        &self,
        name: &str,
        options: &NumericRangeOptions,
    ) -> Result<Vec<IndexedDocumentRecord<V>>, ClusterError> {
        if !load_registry(&self.index_path)
            .await?
            .indexes
            .iter()
            .any(|item| item.name == name)
        {
            return Err(index_missing("index", name));
        }
        let bounds = NumericRangeOptions {
            min: options.min,
            max: options.max,
            min_exclusive: options.min_exclusive,
            max_exclusive: options.max_exclusive,
            ..Default::default()
        };
        let mut rows = Vec::new();
        for shard in self.router.shard_ids() {
            rows.extend(
                self.reader(shard)
                    .await?
                    .database
                    .find_range(name, &bounds)?,
            );
        }
        rows.sort_by(|a, b| {
            a.field
                .partial_cmp(&b.field)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.key.as_bytes().cmp(b.key.as_bytes()))
        });
        if options.reverse {
            rows.reverse();
        }
        let start = options.offset.min(rows.len());
        let end = options.count.map_or(rows.len(), |count| {
            start.saturating_add(count).min(rows.len())
        });
        Ok(rows.drain(start..end).collect())
    }

    pub async fn create_compound_index(
        &self,
        name: &str,
        definition: CompoundIndexDef,
    ) -> Result<(), ClusterError> {
        self.require_json_codec("compound")?;
        self.ensure_writable()?;
        if load_registry(&self.index_path)
            .await?
            .compound_indexes
            .iter()
            .any(|item| item.name == name)
        {
            return Err(index_already("compound index", name));
        }
        let mut created = Vec::new();
        for shard in self.router.shard_ids() {
            let handle = self.writer(shard).await?;
            if !handle
                .database
                .list_compound_indexes()?
                .iter()
                .any(|index| index.name == name)
            {
                if let Err(error) = handle
                    .database
                    .create_compound_index(name, definition.clone())
                    .await
                {
                    for shard in created {
                        if let Ok(handle) = self.writer(shard).await {
                            let _ = handle.database.drop_compound_index(name).await;
                        }
                    }
                    return Err(error.into());
                }
                created.push(shard);
            }
        }
        self.mutate_registry(|registry| {
            if let Some(existing) = registry
                .compound_indexes
                .iter()
                .find(|item| item.name == name)
            {
                return if existing.def == definition {
                    Ok(())
                } else {
                    Err(index_already("compound index", name))
                };
            }
            registry.compound_indexes.push(NamedCompoundDefinition {
                name: name.into(),
                def: definition,
            });
            Ok(())
        })
        .await
    }
    pub async fn drop_compound_index(&self, name: &str) -> Result<bool, ClusterError> {
        self.ensure_writable()?;
        let existed = load_registry(&self.index_path)
            .await?
            .compound_indexes
            .iter()
            .any(|item| item.name == name);
        for shard in self.router.shard_ids() {
            let handle = self.writer(shard).await?;
            if handle
                .database
                .list_compound_indexes()?
                .iter()
                .any(|index| index.name == name)
            {
                handle.database.drop_compound_index(name).await?;
            }
        }
        if existed {
            self.mutate_registry(|registry| {
                registry.compound_indexes.retain(|item| item.name != name);
                Ok(())
            })
            .await?;
        }
        Ok(existed)
    }
    pub async fn list_compound_indexes(&self) -> Result<Vec<CompoundIndexInfo>, ClusterError> {
        Ok(load_registry(&self.index_path)
            .await?
            .compound_indexes
            .into_iter()
            .map(|item| CompoundIndexInfo {
                name: item.name,
                group_by: item.def.group_by,
                order_by: item.def.order_by,
                order_type: item.def.order_type,
            })
            .collect())
    }

    pub async fn create_text_index(
        &self,
        name: &str,
        fields: Option<Vec<String>>,
    ) -> Result<(), ClusterError> {
        self.require_json_codec("text")?;
        self.ensure_writable()?;
        if load_registry(&self.index_path)
            .await?
            .text_indexes
            .iter()
            .any(|item| item.name == name)
        {
            return Err(index_already("text index", name));
        }
        let mut created = Vec::new();
        for shard in self.router.shard_ids() {
            let handle = self.writer(shard).await?;
            if !handle
                .database
                .list_text_indexes()?
                .iter()
                .any(|(index, _)| index == name)
            {
                if let Err(error) = handle
                    .database
                    .create_text_index(name, fields.clone())
                    .await
                {
                    for shard in created {
                        if let Ok(handle) = self.writer(shard).await {
                            let _ = handle.database.drop_text_index(name).await;
                        }
                    }
                    return Err(error.into());
                }
                created.push(shard);
            }
        }
        self.mutate_registry(|registry| {
            if let Some(existing) = registry.text_indexes.iter().find(|item| item.name == name) {
                return if existing.fields == fields {
                    Ok(())
                } else {
                    Err(index_already("text index", name))
                };
            }
            registry.text_indexes.push(NamedTextDefinition {
                name: name.into(),
                fields,
            });
            Ok(())
        })
        .await
    }
    pub async fn drop_text_index(&self, name: &str) -> Result<bool, ClusterError> {
        self.ensure_writable()?;
        let existed = load_registry(&self.index_path)
            .await?
            .text_indexes
            .iter()
            .any(|item| item.name == name);
        for shard in self.router.shard_ids() {
            let handle = self.writer(shard).await?;
            if handle
                .database
                .list_text_indexes()?
                .iter()
                .any(|(index, _)| index == name)
            {
                handle.database.drop_text_index(name).await?;
            }
        }
        if existed {
            self.mutate_registry(|registry| {
                registry.text_indexes.retain(|item| item.name != name);
                Ok(())
            })
            .await?;
        }
        Ok(existed)
    }
    pub async fn search(
        &self,
        name: &str,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchDocumentRecord<V>>, ClusterError> {
        if !load_registry(&self.index_path)
            .await?
            .text_indexes
            .iter()
            .any(|item| item.name == name)
        {
            return Err(index_missing("text index", name));
        }
        let mut rows = Vec::new();
        for shard in self.router.shard_ids() {
            rows.extend(
                self.reader(shard)
                    .await?
                    .database
                    .search(name, query, options)?,
            );
        }
        rows.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.key.as_bytes().cmp(b.key.as_bytes()))
        });
        rows.truncate(options.limit);
        Ok(rows)
    }

    pub async fn compact(&self) -> Result<CompactResult, ClusterError> {
        self.ensure_writable()?;
        let mut result = CompactResult::default();
        for shard in self.router.shard_ids() {
            match self.writer(shard).await {
                Ok(handle) => {
                    handle.database.compact().await?;
                    result.compacted.push(shard);
                }
                Err(error) if cluster_error_is_locked(&error) => result.skipped.push(shard),
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }
    pub async fn stats(&self) -> ClusterStats {
        let (writers_cached, readers_cached) = self.pool.cached_counts().await;
        let stats = self.pool.stats().await;
        ClusterStats {
            shard_count: self.shard_count(),
            writers_cached,
            readers_cached,
            writer_opens: stats.writer_opens,
            reader_opens: stats.reader_opens,
            reader_reopens: stats.reader_reopens,
            incremental_catchups: stats.incremental_catchups,
            catchup_frames_applied: stats.catchup_frames_applied,
            lock_waits: stats.lock_waits,
            evictions: stats.evictions,
        }
    }
    pub async fn close(&self) -> Result<(), ClusterError> {
        if self.closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return Ok(());
        }
        self.pool.close_all().await?;
        Ok(())
    }

    async fn mutate_registry(
        &self,
        mutation: impl FnOnce(&mut ClusterIndexRegistry) -> Result<(), ClusterError>,
    ) -> Result<(), ClusterError> {
        let lock = LockFile::new(PathBuf::from(format!("{}.lock", self.index_path.display())));
        let deadline = Instant::now() + self.registry_timeout;
        let mut delay = Duration::from_millis(10);
        loop {
            if lock.acquire().await? {
                break;
            }
            if Instant::now() + delay > deadline {
                return Err(ClusterError::RegistryTimeout);
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(250));
        }
        let result = async {
            let mut registry = load_registry(&self.index_path).await?;
            mutation(&mut registry)?;
            save_registry(&self.index_path, &registry).await
        }
        .await;
        let _ = lock.release().await;
        result
    }
}

fn index_already(kind: &str, name: &str) -> ClusterError {
    ClusterError::Coordination(format!("{kind} \"{name}\" already exists"))
}
fn index_missing(kind: &str, name: &str) -> ClusterError {
    ClusterError::Coordination(format!("no such {kind}: {name}"))
}
fn partial_result(
    operation: &'static str,
    total: usize,
    errors: Vec<String>,
) -> Result<(), ClusterError> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ClusterError::PartialWrites {
            operation,
            failed: errors.len(),
            total,
            errors,
        })
    }
}
fn cluster_error_is_locked(error: &ClusterError) -> bool {
    matches!(
        error,
        ClusterError::Pool(LockPoolError::Database(MiniDbError::Lock(
            LockError::Locked(_)
        ))) | ClusterError::Database(MiniDbError::Lock(LockError::Locked(_)))
            | ClusterError::Lock(LockError::Locked(_))
    )
}
async fn load_registry(path: &Path) -> Result<ClusterIndexRegistry, ClusterError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(ClusterIndexRegistry::default())
        }
        Err(error) => Err(error.into()),
    }
}
async fn save_registry(path: &Path, registry: &ClusterIndexRegistry) -> Result<(), ClusterError> {
    let temporary = PathBuf::from(format!("{}.tmp-{}", path.display(), std::process::id()));
    tokio::fs::write(&temporary, serde_json::to_vec_pretty(registry)?).await?;
    tokio::fs::rename(temporary, path).await?;
    Ok(())
}
fn compare_documents<V>(
    codec: &dyn ValueCodec<V>,
    left: &DocumentRecord<V>,
    right: &DocumentRecord<V>,
    sort: &[(String, i8)],
) -> Ordering {
    let left = codec
        .index_value(&left.value)
        .unwrap_or(serde_json::Value::Null);
    let right = codec
        .index_value(&right.value)
        .unwrap_or(serde_json::Value::Null);
    for (path, direction) in sort {
        let order = compare_values(
            crate::query::get_path(&left, path),
            crate::query::get_path(&right, path),
        );
        if order != Ordering::Equal {
            return if *direction < 0 {
                order.reverse()
            } else {
                order
            };
        }
    }
    Ordering::Equal
}
fn compare_values(left: Option<&serde_json::Value>, right: Option<&serde_json::Value>) -> Ordering {
    match (left, right) {
        (Some(serde_json::Value::Number(a)), Some(serde_json::Value::Number(b))) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(Ordering::Equal),
        (Some(serde_json::Value::String(a)), Some(serde_json::Value::String(b))) => a.cmp(b),
        (Some(a), Some(b)) => a.to_string().cmp(&b.to_string()),
        (None, None) => Ordering::Equal,
        (None, _) => Ordering::Less,
        (_, None) => Ordering::Greater,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[tokio::test]
    async fn routes_persists_and_globally_scans_shards() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = ClusterOpenOptions::new(MiniDb::<Value>::json_options(directory.path()));
        options.shard_count = Some(4);
        options.lock_hold_millis = 0;
        let cluster = ClusterDb::open(options).await.unwrap();
        cluster
            .set("a", serde_json::json!({"n":1}), SetOptions::default())
            .await
            .unwrap();
        cluster
            .set("z", serde_json::json!({"n":2}), SetOptions::default())
            .await
            .unwrap();
        assert_eq!(
            cluster
                .scan(&ScanOptions::default())
                .await
                .unwrap()
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        cluster.close().await.unwrap();
    }

    #[tokio::test]
    async fn readonly_reader_incrementally_observes_writer_wal() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer_options =
            ClusterOpenOptions::new(MiniDb::<Value>::json_options(directory.path()));
        writer_options.shard_count = Some(2);
        writer_options.lock_hold_millis = 0;
        let writer = ClusterDb::open(writer_options).await.unwrap();
        writer
            .set("key", Value::from(1), SetOptions::default())
            .await
            .unwrap();

        let mut reader_options =
            ClusterOpenOptions::new(MiniDb::<Value>::json_options(directory.path()));
        reader_options.shard_count = Some(2);
        reader_options.read_only = true;
        let reader = ClusterDb::open(reader_options).await.unwrap();
        assert_eq!(reader.get("key").await.unwrap(), Some(Value::from(1)));
        writer
            .set("key", Value::from(2), SetOptions::default())
            .await
            .unwrap();
        assert_eq!(reader.get("key").await.unwrap(), Some(Value::from(2)));
        assert!(reader.stats().await.incremental_catchups >= 1);
        reader.close().await.unwrap();
        writer.close().await.unwrap();
    }

    #[tokio::test]
    async fn fans_out_indexes_and_rejects_cross_shard_none() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = ClusterOpenOptions::new(MiniDb::<Value>::json_options(directory.path()));
        options.shard_count = Some(4);
        options.lock_hold_millis = 0;
        options.cross_shard = CrossShardMode::None;
        let cluster = ClusterDb::open(options).await.unwrap();
        cluster
            .create_index("kind", IndexDef::equality("kind"))
            .await
            .unwrap();
        cluster
            .set("a", serde_json::json!({"kind":"x"}), SetOptions::default())
            .await
            .unwrap();
        assert_eq!(
            cluster.find_eq("kind", &Value::from("x")).await.unwrap()[0].key,
            "a"
        );
        let first = "a".to_owned();
        let second = (0..100)
            .map(|number| format!("key-{number}"))
            .find(|key| cluster.shard_of(key).unwrap() != cluster.shard_of(&first).unwrap())
            .unwrap();
        assert!(matches!(
            cluster
                .mset(vec![(first, Value::Null), (second, Value::Null)])
                .await,
            Err(ClusterError::Coordination(_))
        ));
        cluster.close().await.unwrap();
    }
}
