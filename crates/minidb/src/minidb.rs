use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, RwLock};

use crate::{
    codec::{
        BatchOp, CodecError, Frame, HEADER_SIZE, TYPE_BATCH, TYPE_DEL, TYPE_SET, encode_batch_ops,
        encode_frame, scan_batch_op_refs,
    },
    compaction::{CompactionError, CompactionTarget, compact},
    compound_index::{
        CompoundIndexDef, CompoundIndexError, CompoundIndexInfo, CompoundIndexManager, OrderValue,
    },
    dt_index::DateTimeIndex,
    index_manager::{
        BatchIndexOp, BatchIndexOpType, IndexDef, IndexError, IndexInfo, IndexManager,
        NumericRangeOptions,
    },
    lockfile::{LockError, LockFile},
    query::{matches_filter, project},
    recovery::{
        CatchUpResult, RecoveredOp, RecoveryError, RecoveryInfo, RecoveryOptions, ValueMode,
        WalAnchor, catch_up_wal, frame_to_ops, recover,
    },
    skiplist::RangeOptions,
    store::{
        Store, StoreEntry, StoreError, StoreOptions, StoreRecord, ValueFile, ValueLoc, ValueRef,
        now_millis,
    },
    text_index::{SearchOptions, TextIndex, TextIndexError, TextIndexOptions},
    value_reader::{PositionedValueReader, ValueReaderError},
    wal::{FsyncPolicy, Wal, WalError, WalOptions, WalStats},
};

const MAX_KEY_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecName {
    Buffer,
    String,
    Json,
    Custom,
}

pub trait ValueCodec<V>: Send + Sync {
    fn encode(&self, value: &V) -> Result<Vec<u8>, String>;
    fn decode(&self, bytes: &[u8]) -> Result<V, String>;
    fn index_value(&self, _value: &V) -> Option<Value> {
        None
    }
    fn name(&self) -> CodecName {
        CodecName::Custom
    }
}

pub struct BufferCodec;
impl ValueCodec<Vec<u8>> for BufferCodec {
    fn encode(&self, value: &Vec<u8>) -> Result<Vec<u8>, String> {
        Ok(value.clone())
    }
    fn decode(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        Ok(bytes.to_vec())
    }
    fn name(&self) -> CodecName {
        CodecName::Buffer
    }
}

pub struct StringCodec;
impl ValueCodec<String> for StringCodec {
    fn encode(&self, value: &String) -> Result<Vec<u8>, String> {
        Ok(value.as_bytes().to_vec())
    }
    fn decode(&self, bytes: &[u8]) -> Result<String, String> {
        String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())
    }
    fn name(&self) -> CodecName {
        CodecName::String
    }
}

pub struct JsonCodec;
impl ValueCodec<Value> for JsonCodec {
    fn encode(&self, value: &Value) -> Result<Vec<u8>, String> {
        serde_json::to_vec(value).map_err(|error| error.to_string())
    }
    fn decode(&self, bytes: &[u8]) -> Result<Value, String> {
        serde_json::from_slice(bytes).map_err(|error| error.to_string())
    }
    fn index_value(&self, value: &Value) -> Option<Value> {
        Some(value.clone())
    }
    fn name(&self) -> CodecName {
        CodecName::Json
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueModeSetting {
    Memory,
    Disk,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPolicy {
    Reject,
    EvictLru,
}

pub struct OpenOptions<V> {
    pub directory: PathBuf,
    pub codec: Arc<dyn ValueCodec<V>>,
    pub fsync_policy: FsyncPolicy,
    pub compact_threshold_bytes: u64,
    pub auto_compact: bool,
    pub active_expire_interval: Duration,
    pub recovery_mode: crate::codec::CorruptionMode,
    pub read_only: bool,
    pub readonly_on_lock_fail: bool,
    pub value_mode: ValueModeSetting,
    pub max_memory_bytes: Option<usize>,
    pub max_memory_policy: MemoryPolicy,
}

impl<V> OpenOptions<V> {
    pub fn new(directory: impl Into<PathBuf>, codec: Arc<dyn ValueCodec<V>>) -> Self {
        Self {
            directory: directory.into(),
            codec,
            fsync_policy: FsyncPolicy::EverySecond,
            compact_threshold_bytes: 64 * 1024 * 1024,
            auto_compact: true,
            active_expire_interval: Duration::from_millis(100),
            recovery_mode: crate::codec::CorruptionMode::Resync,
            read_only: false,
            readonly_on_lock_fail: false,
            value_mode: ValueModeSetting::Memory,
            max_memory_bytes: None,
            max_memory_policy: MemoryPolicy::Reject,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SetOptions {
    pub ttl_millis: Option<f64>,
    pub datetimes: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone)]
pub enum BatchInputOp<V> {
    Set {
        key: String,
        value: V,
        options: SetOptions,
    },
    Del {
        key: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocumentRecord<V> {
    pub key: String,
    pub value: V,
    pub datetimes: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DateTimeDocumentRecord<V> {
    pub record: DocumentRecord<V>,
    pub datetime_value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexedDocumentRecord<V> {
    pub key: String,
    pub value: V,
    pub field: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompoundDocumentRecord<V> {
    pub key: String,
    pub value: V,
    pub order_value: OrderValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchDocumentRecord<V> {
    pub key: String,
    pub value: V,
    pub score: f64,
}

#[derive(Debug, Clone, Default)]
pub struct KeyQuery {
    pub exact: Option<String>,
    pub prefix: Option<String>,
    pub range: RangeOptions<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct TextQuery {
    pub index: String,
    pub query: String,
    pub options: SearchOptions,
}

#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
    pub key: Option<KeyQuery>,
    pub datetimes: HashMap<String, RangeOptions<f64>>,
    pub text: Option<TextQuery>,
    pub filter: Option<Value>,
    pub project: Option<Vec<String>>,
    pub sort: Vec<(String, i8)>,
    pub skip: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MiniDbStats {
    pub evictions: u64,
    pub max_memory_rejections: u64,
    pub query_index_hits: u64,
}

#[derive(Debug, Error)]
pub enum MiniDbError {
    #[error("MiniDb is closed")]
    Closed,
    #[error("MiniDb is open in read-only mode")]
    ReadOnly,
    #[error("key must be non-empty")]
    EmptyKey,
    #[error("key too long (>{MAX_KEY_LEN})")]
    KeyTooLong,
    #[error("ttl must be a finite number of milliseconds")]
    InvalidTtl,
    #[error("{0} indexes require the JSON codec")]
    JsonCodecRequired(&'static str),
    #[error("text index \"{0}\" already exists")]
    TextIndexExists(String),
    #[error("no such text index: {0}")]
    TextIndexNotFound(String),
    #[error("maxMemory exceeded: projected {projected} bytes > {maximum} bytes")]
    MaxMemory { projected: usize, maximum: usize },
    #[error("codec failed: {0}")]
    Codec(String),
    #[error("state lock is poisoned")]
    StatePoisoned,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    FrameCodec(#[from] CodecError),
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Compound(#[from] CompoundIndexError),
    #[error(transparent)]
    Text(#[from] TextIndexError),
    #[error(transparent)]
    ValueReader(#[from] ValueReaderError),
    #[error(transparent)]
    Compaction(#[from] CompactionError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TextIndexDefinition {
    name: String,
    fields: Option<Vec<String>>,
}

#[derive(Default)]
struct DerivedState {
    indexes: IndexManager,
    datetimes: DateTimeIndex,
    compound: CompoundIndexManager,
    text: HashMap<String, TextIndex>,
    text_definitions: Vec<TextIndexDefinition>,
    access: Vec<Vec<u8>>,
}

type LiveJsonRecord = (String, Value, Option<BTreeMap<String, f64>>);
struct WalTail {
    anchor: WalAnchor,
    offset: u64,
}

struct MiniDbInner<V> {
    directory: PathBuf,
    wal_path: PathBuf,
    index_path: PathBuf,
    text_index_path: PathBuf,
    compound_index_path: PathBuf,
    codec: Arc<dyn ValueCodec<V>>,
    store: Arc<Mutex<Store>>,
    derived: Mutex<DerivedState>,
    wal: Arc<RwLock<Wal>>,
    rotation_gate: Arc<RwLock<()>>,
    write_lock: AsyncMutex<()>,
    value_reader: Option<Arc<PositionedValueReader>>,
    value_mode: ValueMode,
    read_only: bool,
    lock: Option<Arc<LockFile>>,
    closed: std::sync::atomic::AtomicBool,
    recovery_info: RecoveryInfo,
    wal_tail: Mutex<Option<WalTail>>,
    max_memory_bytes: Option<usize>,
    max_memory_policy: MemoryPolicy,
    auto_compact: bool,
    compaction: OnceLock<Arc<CompactionTarget>>,
    expire_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    stats: Mutex<MiniDbStats>,
}

#[derive(Clone)]
pub struct MiniDb<V> {
    inner: Arc<MiniDbInner<V>>,
}

struct PreparedOp {
    op_type: u8,
    key: Vec<u8>,
    value: Option<Vec<u8>>,
    meta: Option<Vec<u8>>,
    expire_at: i64,
    datetimes: Option<BTreeMap<String, f64>>,
    pk: Vec<u8>,
    index_value: Option<Value>,
}

impl MiniDb<Vec<u8>> {
    pub fn buffer_options(directory: impl Into<PathBuf>) -> OpenOptions<Vec<u8>> {
        OpenOptions::new(directory, Arc::new(BufferCodec))
    }
}
impl MiniDb<String> {
    pub fn string_options(directory: impl Into<PathBuf>) -> OpenOptions<String> {
        OpenOptions::new(directory, Arc::new(StringCodec))
    }
}
impl MiniDb<Value> {
    pub fn json_options(directory: impl Into<PathBuf>) -> OpenOptions<Value> {
        OpenOptions::new(directory, Arc::new(JsonCodec))
    }
}

impl<V: Send + Sync + 'static> MiniDb<V> {
    // Original: packages/minidb/src/index.ts, MiniDb.open().
    pub async fn open(mut options: OpenOptions<V>) -> Result<Self, MiniDbError> {
        if let Some(maximum) = options.max_memory_bytes
            && maximum == 0
        {
            return Err(MiniDbError::MaxMemory {
                projected: 0,
                maximum,
            });
        }
        tokio::fs::create_dir_all(&options.directory).await?;
        let lock = if options.read_only {
            None
        } else {
            let lock = Arc::new(LockFile::new(options.directory.join("db.lock")));
            if lock.acquire().await? {
                Some(lock)
            } else if options.readonly_on_lock_fail {
                options.read_only = true;
                None
            } else {
                return Err(LockError::Locked(options.directory.clone()).into());
            }
        };
        if !options.read_only {
            cleanup_temporary_files(&options.directory).await?;
        }
        let wal_path = options.directory.join("db.wal");
        let wal_stats = Arc::new(Mutex::new(WalStats::default()));
        let wal = Wal::new(
            &wal_path,
            WalOptions {
                fsync_policy: options.fsync_policy,
                stats: Some(Arc::clone(&wal_stats)),
                ..Default::default()
            },
        );
        if !options.read_only {
            wal.open().await?;
        }
        let value_mode = resolve_value_mode(
            options.value_mode,
            &options.directory,
            options.max_memory_bytes,
        )
        .await?;
        let reader = (value_mode == ValueMode::Disk)
            .then(|| Arc::new(PositionedValueReader::new(&options.directory)));
        let reader_for_store = reader.clone();
        let mut store = Store::new(StoreOptions {
            read_value: reader_for_store.map(|reader| {
                Arc::new(move |location| {
                    reader
                        .read(location)
                        .map_err(|error| StoreError::Read(error.to_string()))
                }) as _
            }),
        });
        let mut recovery_options = RecoveryOptions::new(&options.directory);
        recovery_options.mode = options.recovery_mode;
        recovery_options.truncate = !options.read_only;
        recovery_options.value_mode = value_mode;
        let recovery_info = recover(&mut store, recovery_options).await?;
        if recovery_info.truncated_wal && !options.read_only {
            wal.refresh_size().await?;
        }
        if let Some(reader) = &reader {
            reader.open()?;
        }

        let mut derived = DerivedState::default();
        load_definitions(&options.directory, options.read_only, &mut derived).await?;
        rebuild_derived(&store, &options.codec, &mut derived)?;
        derived.access = store.map().keys().cloned().collect();
        let store = Arc::new(Mutex::new(store));
        let wal = Arc::new(RwLock::new(wal));
        let rotation_gate = Arc::new(RwLock::new(()));
        let inner = Arc::new(MiniDbInner {
            directory: options.directory.clone(),
            wal_path: wal_path.clone(),
            index_path: options.directory.join("db.indexes.json"),
            text_index_path: options.directory.join("db.textindexes.json"),
            compound_index_path: options.directory.join("db.compound-indexes.json"),
            codec: options.codec,
            store: Arc::clone(&store),
            derived: Mutex::new(derived),
            wal: Arc::clone(&wal),
            rotation_gate: Arc::clone(&rotation_gate),
            write_lock: AsyncMutex::new(()),
            value_reader: reader,
            value_mode,
            read_only: options.read_only,
            lock,
            closed: std::sync::atomic::AtomicBool::new(false),
            recovery_info,
            wal_tail: Mutex::new(None),
            max_memory_bytes: options.max_memory_bytes,
            max_memory_policy: options.max_memory_policy,
            auto_compact: options.auto_compact,
            compaction: OnceLock::new(),
            expire_task: Mutex::new(None),
            stats: Mutex::new(MiniDbStats::default()),
        });
        let weak = Arc::downgrade(&inner);
        let mut target = CompactionTarget::new(
            &options.directory,
            &wal_path,
            options.fsync_policy,
            store,
            wal,
            wal_stats,
            options.compact_threshold_bytes,
            rotation_gate,
        );
        target.value_reader = inner.value_reader.clone();
        target.on_compacted = Some(Arc::new(move || {
            weak.upgrade()
                .ok_or_else(|| "database closed".into())
                .and_then(|inner| rebuild_text_indexes(&inner).map_err(|error| error.to_string()))
        }));
        inner.compaction.set(Arc::new(target)).ok();
        let database = Self { inner };
        database.start_expiration_task(options.active_expire_interval);
        if !database.inner.read_only
            && database.inner.auto_compact
            && database.compaction().should_compact().await
        {
            database.compact().await?;
        }
        Ok(database)
    }

    fn compaction(&self) -> &Arc<CompactionTarget> {
        self.inner.compaction.get().expect("compaction initialized")
    }

    fn ensure_open(&self) -> Result<(), MiniDbError> {
        if self.inner.closed.load(std::sync::atomic::Ordering::Acquire) {
            Err(MiniDbError::Closed)
        } else {
            Ok(())
        }
    }

    fn ensure_writable(&self) -> Result<(), MiniDbError> {
        if self.inner.read_only {
            Err(MiniDbError::ReadOnly)
        } else {
            Ok(())
        }
    }

    fn start_expiration_task(&self, interval: Duration) {
        if interval.is_zero() {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = Weak::upgrade(&weak) else {
                    break;
                };
                if inner.closed.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                if let (Ok(mut store), Ok(mut derived)) = (inner.store.lock(), inner.derived.lock())
                {
                    store.reap_expired(None);
                    drain_expired(&mut store, &mut derived);
                }
            }
        });
        *self
            .inner
            .expire_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
    }

    fn prepare_set(
        &self,
        key: &[u8],
        value: V,
        options: SetOptions,
    ) -> Result<PreparedOp, MiniDbError> {
        check_key(key)?;
        let ttl = options.ttl_millis;
        if ttl.is_some_and(|ttl| !ttl.is_finite()) {
            return Err(MiniDbError::InvalidTtl);
        }
        let expire_at = ttl
            .filter(|ttl| *ttl != 0.0)
            .map_or(0, |ttl| now_millis().saturating_add(ttl.floor() as i64));
        let encoded = self
            .inner
            .codec
            .encode(&value)
            .map_err(MiniDbError::Codec)?;
        let meta = options
            .datetimes
            .as_ref()
            .map(|dt| serde_json::to_vec(&serde_json::json!({ "dt": dt })))
            .transpose()?;
        let index_value = self.inner.codec.index_value(&value);
        Ok(PreparedOp {
            op_type: TYPE_SET,
            key: key.to_vec(),
            value: Some(encoded),
            meta,
            expire_at,
            datetimes: options.datetimes,
            pk: key.to_vec(),
            index_value,
        })
    }

    fn prepare_del(&self, key: &[u8]) -> Result<PreparedOp, MiniDbError> {
        check_key(key)?;
        Ok(PreparedOp {
            op_type: TYPE_DEL,
            key: key.to_vec(),
            value: None,
            meta: None,
            expire_at: 0,
            datetimes: None,
            pk: key.to_vec(),
            index_value: None,
        })
    }

    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<V>, MiniDbError> {
        self.ensure_open()?;
        let key = key.as_ref();
        let bytes = {
            let mut store = self
                .inner
                .store
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            let value = store.get(key)?;
            let mut derived = self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            drain_expired(&mut store, &mut derived);
            value
        };
        if bytes.is_some() {
            self.touch_access(key)?;
        }
        bytes
            .map(|bytes| self.inner.codec.decode(&bytes).map_err(MiniDbError::Codec))
            .transpose()
    }

    pub fn get_record(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<DocumentRecord<V>>, MiniDbError> {
        let key = key.as_ref();
        let Some(value) = self.get(key)? else {
            return Ok(None);
        };
        let datetimes = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .map()
            .get(key)
            .and_then(|record| record.datetimes.clone());
        Ok(Some(DocumentRecord {
            key: String::from_utf8_lossy(key).into_owned(),
            value,
            datetimes,
        }))
    }

    // Original: MiniDb.set(). Store and derived state apply in the same synchronous segment as append_loc.
    pub async fn set(
        &self,
        key: impl AsRef<[u8]>,
        value: V,
        options: SetOptions,
    ) -> Result<(), MiniDbError> {
        self.ensure_open()?;
        self.ensure_writable()?;
        let _write = self.inner.write_lock.lock().await;
        let _rotation = self.inner.rotation_gate.read().await;
        let operation = self.prepare_set(key.as_ref(), value, options)?;
        self.ensure_memory_for(std::slice::from_ref(&operation))
            .await?;
        self.check_unique(&operation)?;
        let frame = encode_frame(&Frame {
            frame_type: TYPE_SET,
            key: operation.key.clone(),
            value: operation.value.clone().expect("set value"),
            meta: operation.meta.clone(),
            expire_at: operation.expire_at,
        })?;
        let wal = self.inner.wal.read().await.clone();
        let appended = wal.append_loc(frame)?;
        let offset = appended.offset;
        let (previous, sequence) = self.apply_operation(&operation)?;
        if let Err(error) = appended.done().await {
            self.restore_key(&operation.pk, previous, sequence)?;
            return Err(error.into());
        }
        if self.inner.value_mode == ValueMode::Disk {
            let location = ValueLoc {
                file: ValueFile::Wal,
                offset: offset + HEADER_SIZE as u64 + operation.key.len() as u64,
                len: operation.value.as_ref().expect("set value").len() as u32,
            };
            self.publish_wal_ref(&operation, sequence, location)?;
        }
        drop(_rotation);
        drop(_write);
        self.maybe_auto_compact();
        Ok(())
    }

    pub async fn del(&self, key: impl AsRef<[u8]>) -> Result<bool, MiniDbError> {
        self.ensure_open()?;
        self.ensure_writable()?;
        let key = key.as_ref();
        let _write = self.inner.write_lock.lock().await;
        let _rotation = self.inner.rotation_gate.read().await;
        if !self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .has(key)
        {
            return Ok(false);
        }
        let operation = self.prepare_del(key)?;
        let wal = self.inner.wal.read().await.clone();
        let pending = wal.append(encode_frame(&Frame {
            frame_type: TYPE_DEL,
            key: operation.key.clone(),
            value: Vec::new(),
            meta: None,
            expire_at: 0,
        })?);
        let (previous, sequence) = self.apply_operation(&operation)?;
        if let Err(error) = pending.await {
            self.restore_key(&operation.pk, previous, sequence)?;
            return Err(error.into());
        }
        drop(_rotation);
        drop(_write);
        self.maybe_auto_compact();
        Ok(true)
    }

    pub async fn batch(&self, operations: Vec<BatchInputOp<V>>) -> Result<(), MiniDbError> {
        self.ensure_open()?;
        self.ensure_writable()?;
        if operations.is_empty() {
            return Ok(());
        }
        let _write = self.inner.write_lock.lock().await;
        let _rotation = self.inner.rotation_gate.read().await;
        let mut prepared = Vec::with_capacity(operations.len());
        for operation in operations {
            match operation {
                BatchInputOp::Set {
                    key,
                    value,
                    options,
                } => prepared.push(self.prepare_set(key.as_bytes(), value, options)?),
                BatchInputOp::Del { key } => prepared.push(self.prepare_del(key.as_bytes())?),
            }
        }
        self.ensure_memory_for(&prepared).await?;
        self.check_unique_batch(&prepared)?;
        let encoded_ops = prepared
            .iter()
            .map(|operation| BatchOp {
                op_type: operation.op_type,
                key: operation.key.clone(),
                value: operation.value.clone(),
                meta: operation.meta.clone(),
                expire_at: operation.expire_at,
            })
            .collect::<Vec<_>>();
        let body = encode_batch_ops(&encoded_ops)?;
        let frame = encode_frame(&Frame {
            frame_type: TYPE_BATCH,
            key: Vec::new(),
            value: body.clone(),
            meta: None,
            expire_at: 0,
        })?;
        let wal = self.inner.wal.read().await.clone();
        let appended = wal.append_loc(frame)?;
        let frame_offset = appended.offset;
        let mut previous = HashMap::<Vec<u8>, Option<StoreRecord>>::new();
        let mut sequences = HashMap::new();
        for operation in &prepared {
            let (record, _) = self.apply_operation(operation)?;
            previous.entry(operation.pk.clone()).or_insert(record);
        }
        {
            let store = self
                .inner
                .store
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            for key in previous.keys() {
                sequences.insert(
                    key.clone(),
                    store.map().get(key).map(|record| record.sequence),
                );
            }
        }
        if let Err(error) = appended.done().await {
            for (key, record) in previous {
                self.restore_key(&key, record, sequences[&key])?;
            }
            return Err(error.into());
        }
        if self.inner.value_mode == ValueMode::Disk {
            let references = scan_batch_op_refs(&body, 0)?;
            let mut last = HashMap::new();
            for (operation, reference) in prepared.iter().zip(references) {
                if operation.op_type == TYPE_SET {
                    last.insert(operation.pk.clone(), (operation, reference));
                }
            }
            for (key, (operation, reference)) in last {
                self.publish_wal_ref(
                    operation,
                    sequences[&key],
                    ValueLoc {
                        file: ValueFile::Wal,
                        offset: frame_offset + HEADER_SIZE as u64 + reference.value_offset,
                        len: reference.value_len,
                    },
                )?;
            }
        }
        drop(_rotation);
        drop(_write);
        self.maybe_auto_compact();
        Ok(())
    }

    fn apply_operation(
        &self,
        operation: &PreparedOp,
    ) -> Result<(Option<StoreRecord>, Option<u64>), MiniDbError> {
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        let mut derived = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        drain_expired(&mut store, &mut derived);
        let old_bytes = store.get(&operation.pk)?;
        drain_expired(&mut store, &mut derived);
        let previous = store.map().get(&operation.pk).cloned();
        let old_document = old_bytes
            .and_then(|bytes| self.inner.codec.decode(&bytes).ok())
            .and_then(|value| self.inner.codec.index_value(&value));
        if operation.op_type == TYPE_SET {
            store.set(
                operation.key.clone(),
                operation.value.clone().expect("set value"),
                operation.expire_at,
                operation.datetimes.clone(),
            );
            apply_derived_set(
                &mut derived,
                &operation.pk,
                old_document.as_ref(),
                operation.index_value.as_ref(),
                operation.datetimes.as_ref(),
            );
            touch(&mut derived.access, &operation.pk);
        } else if store.del(&operation.pk) {
            apply_derived_delete(&mut derived, &operation.pk, old_document.as_ref());
        }
        let sequence = store.map().get(&operation.pk).map(|record| record.sequence);
        Ok((previous, sequence))
    }

    fn restore_key(
        &self,
        key: &[u8],
        previous: Option<StoreRecord>,
        applied_sequence: Option<u64>,
    ) -> Result<(), MiniDbError> {
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        let current = store.map().get(key);
        if applied_sequence.map_or(current.is_some(), |sequence| {
            current.is_none_or(|record| record.sequence != sequence)
        }) {
            return Ok(());
        }
        let mut derived = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        apply_derived_delete(&mut derived, key, None);
        if let Some(previous) = previous {
            store.set_ref(
                key.to_vec(),
                previous.value_ref,
                previous.expire_at,
                previous.datetimes.clone(),
            );
            let document = store
                .get(key)?
                .and_then(|bytes| self.inner.codec.decode(&bytes).ok())
                .and_then(|value| self.inner.codec.index_value(&value));
            apply_derived_set(
                &mut derived,
                key,
                None,
                document.as_ref(),
                previous.datetimes.as_ref(),
            );
            touch(&mut derived.access, key);
        } else {
            store.del(key);
            derived.access.retain(|entry| entry != key);
        }
        Ok(())
    }

    fn publish_wal_ref(
        &self,
        operation: &PreparedOp,
        sequence: Option<u64>,
        location: ValueLoc,
    ) -> Result<(), MiniDbError> {
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        if sequence.is_some()
            && store
                .map()
                .get(&operation.pk)
                .is_some_and(|record| Some(record.sequence) == sequence)
        {
            store.set_ref(
                operation.pk.clone(),
                ValueRef::Disk(location),
                operation.expire_at,
                operation.datetimes.clone(),
            );
        }
        Ok(())
    }

    fn check_unique(&self, operation: &PreparedOp) -> Result<(), MiniDbError> {
        if let Some(document) = &operation.index_value {
            self.inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?
                .indexes
                .check_unique(&key_string(&operation.pk), document)?;
        }
        Ok(())
    }

    fn check_unique_batch(&self, operations: &[PreparedOp]) -> Result<(), MiniDbError> {
        let batch = operations
            .iter()
            .map(|operation| BatchIndexOp {
                pk: key_string(&operation.pk),
                op: if operation.op_type == TYPE_DEL {
                    BatchIndexOpType::Del
                } else {
                    BatchIndexOpType::Set
                },
                doc: operation.index_value.clone().unwrap_or(Value::Null),
            })
            .collect::<Vec<_>>();
        self.inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .indexes
            .check_unique_batch(&batch)?;
        Ok(())
    }

    async fn ensure_memory_for(&self, operations: &[PreparedOp]) -> Result<(), MiniDbError> {
        let Some(maximum) = self.inner.max_memory_bytes else {
            return Ok(());
        };
        loop {
            let (projected, victim) = {
                let mut store = self
                    .inner
                    .store
                    .lock()
                    .map_err(|_| MiniDbError::StatePoisoned)?;
                store.reap_expired(None);
                let derived = self
                    .inner
                    .derived
                    .lock()
                    .map_err(|_| MiniDbError::StatePoisoned)?;
                let mut projected = store.bytes();
                let mut considered = HashMap::new();
                for operation in operations {
                    let current = considered
                        .get(&operation.pk)
                        .copied()
                        .unwrap_or_else(|| store.record_bytes(&operation.pk));
                    projected = projected.saturating_sub(current);
                    let next = if operation.op_type == TYPE_SET {
                        store.estimate_set_bytes(
                            &operation.key,
                            operation.value.as_ref().expect("set"),
                            operation.datetimes.as_ref(),
                            self.inner.value_mode == ValueMode::Memory,
                        )
                    } else {
                        0
                    };
                    projected += next;
                    considered.insert(operation.pk.clone(), next);
                }
                let skipped = operations
                    .iter()
                    .map(|operation| operation.pk.as_slice())
                    .collect::<HashSet<_>>();
                let victim = derived
                    .access
                    .iter()
                    .find(|key| !skipped.contains(key.as_slice()) && store.map().contains_key(*key))
                    .cloned();
                (projected, victim)
            };
            if projected <= maximum {
                return Ok(());
            }
            if self.inner.max_memory_policy == MemoryPolicy::EvictLru
                && let Some(victim) = victim
            {
                self.evict(&victim).await?;
                continue;
            }
            self.inner
                .stats
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?
                .max_memory_rejections += 1;
            return Err(MiniDbError::MaxMemory { projected, maximum });
        }
    }

    async fn evict(&self, key: &[u8]) -> Result<(), MiniDbError> {
        let operation = self.prepare_del(key)?;
        let wal = self.inner.wal.read().await.clone();
        let pending = wal.append(encode_frame(&Frame {
            frame_type: TYPE_DEL,
            key: key.to_vec(),
            value: Vec::new(),
            meta: None,
            expire_at: 0,
        })?);
        let (previous, sequence) = self.apply_operation(&operation)?;
        if let Err(error) = pending.await {
            self.restore_key(key, previous, sequence)?;
            return Err(error.into());
        }
        self.inner
            .stats
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .evictions += 1;
        Ok(())
    }

    fn touch_access(&self, key: &[u8]) -> Result<(), MiniDbError> {
        touch(
            &mut self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?
                .access,
            key,
        );
        Ok(())
    }

    fn maybe_auto_compact(&self) {
        if !self.inner.auto_compact || self.compaction().is_compacting() {
            return;
        }
        let target = Arc::clone(self.compaction());
        tokio::spawn(async move {
            if target.should_compact().await {
                let _ = compact(&target).await;
            }
        });
    }

    pub fn has(&self, key: impl AsRef<[u8]>) -> Result<bool, MiniDbError> {
        self.ensure_open()?;
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        let found = store.has(key.as_ref());
        let mut derived = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        drain_expired(&mut store, &mut derived);
        Ok(found)
    }

    pub fn len(&self) -> Result<usize, MiniDbError> {
        Ok(self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .len())
    }
    pub fn is_empty(&self) -> Result<bool, MiniDbError> {
        Ok(self.len()? == 0)
    }

    pub async fn mset(&self, entries: Vec<(String, V)>) -> Result<(), MiniDbError> {
        self.batch(
            entries
                .into_iter()
                .map(|(key, value)| BatchInputOp::Set {
                    key,
                    value,
                    options: SetOptions::default(),
                })
                .collect(),
        )
        .await
    }

    pub fn mget(&self, keys: &[String]) -> Result<Vec<Option<V>>, MiniDbError> {
        keys.iter().map(|key| self.get(key.as_bytes())).collect()
    }

    pub async fn expire(
        &self,
        key: impl AsRef<[u8]>,
        ttl_millis: f64,
    ) -> Result<bool, MiniDbError> {
        if !ttl_millis.is_finite() {
            return Err(MiniDbError::InvalidTtl);
        }
        let key = key.as_ref();
        let Some(record) = self.get_record(key)? else {
            return Ok(false);
        };
        self.set(
            key,
            record.value,
            SetOptions {
                ttl_millis: Some(ttl_millis),
                datetimes: record.datetimes,
            },
        )
        .await?;
        Ok(true)
    }

    pub fn ttl(&self, key: impl AsRef<[u8]>) -> Result<i64, MiniDbError> {
        self.ensure_open()?;
        let store = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        let Some(record) = store.map().get(key.as_ref()) else {
            return Ok(-2);
        };
        if record.expire_at == 0 {
            return Ok(-1);
        }
        let remaining = record.expire_at - now_millis();
        Ok(if remaining > 0 { remaining } else { -2 })
    }

    pub fn scan(
        &self,
        options: &RangeOptions<Vec<u8>>,
    ) -> Result<Vec<DocumentRecord<V>>, MiniDbError> {
        self.ensure_open()?;
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        let entries = store.scan(options)?;
        let mut derived = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        drain_expired(&mut store, &mut derived);
        self.decode_entries(entries)
    }

    pub fn prefix(
        &self,
        prefix: impl AsRef<[u8]>,
        limit: usize,
    ) -> Result<Vec<DocumentRecord<V>>, MiniDbError> {
        self.ensure_open()?;
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        let entries = store.prefix(prefix.as_ref(), limit)?;
        let mut derived = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        drain_expired(&mut store, &mut derived);
        self.decode_entries(entries)
    }

    fn decode_entries(
        &self,
        entries: Vec<StoreEntry>,
    ) -> Result<Vec<DocumentRecord<V>>, MiniDbError> {
        entries
            .into_iter()
            .map(|entry| {
                Ok(DocumentRecord {
                    key: String::from_utf8_lossy(&entry.key).into_owned(),
                    value: self
                        .inner
                        .codec
                        .decode(&entry.value)
                        .map_err(MiniDbError::Codec)?,
                    datetimes: entry.datetimes,
                })
            })
            .collect()
    }

    pub fn datetime_columns(&self) -> Result<Vec<String>, MiniDbError> {
        Ok(self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .datetimes
            .columns())
    }

    pub fn datetime_range(
        &self,
        column: &str,
        options: &RangeOptions<f64>,
    ) -> Result<Vec<DateTimeDocumentRecord<V>>, MiniDbError> {
        let rows = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .datetimes
            .range(column, options);
        rows.into_iter()
            .filter_map(|row| {
                let bytes = key_bytes(&row.key);
                self.get(&bytes).transpose().map(|result| {
                    result.map(|value| DateTimeDocumentRecord {
                        record: DocumentRecord {
                            key: display_key(&bytes),
                            value,
                            datetimes: None,
                        },
                        datetime_value: row.value,
                    })
                })
            })
            .collect()
    }

    pub async fn create_index(&self, name: &str, definition: IndexDef) -> Result<(), MiniDbError> {
        self.ensure_json_codec("secondary")?;
        self.ensure_writable()?;
        let documents = self.live_json_documents()?;
        {
            let mut derived = self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            derived.indexes.create(name, definition)?;
            derived
                .indexes
                .rebuild(documents.iter().map(|(key, value)| (key.as_str(), value)));
            if let Err(error) = derived.indexes.assert_unique_valid(name) {
                derived.indexes.drop(name);
                return Err(error.into());
            }
        }
        self.persist_indexes().await
    }

    pub async fn drop_index(&self, name: &str) -> Result<bool, MiniDbError> {
        self.ensure_writable()?;
        let dropped = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .indexes
            .drop(name);
        self.persist_indexes().await?;
        Ok(dropped)
    }
    pub fn list_indexes(&self) -> Result<Vec<IndexInfo>, MiniDbError> {
        Ok(self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .indexes
            .list())
    }

    pub fn find_eq(
        &self,
        name: &str,
        value: &Value,
    ) -> Result<Vec<DocumentRecord<V>>, MiniDbError> {
        let keys = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .indexes
            .find_eq(name, value)?;
        keys.into_iter()
            .filter_map(|key| {
                let bytes = key_bytes(&key);
                self.get(&bytes).transpose().map(|result| {
                    result.map(|value| DocumentRecord {
                        key: display_key(&bytes),
                        value,
                        datetimes: None,
                    })
                })
            })
            .collect()
    }

    pub fn find_range(
        &self,
        name: &str,
        options: &NumericRangeOptions,
    ) -> Result<Vec<IndexedDocumentRecord<V>>, MiniDbError> {
        let rows = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .indexes
            .find_range(name, options)?;
        rows.into_iter()
            .filter_map(|row| {
                let bytes = key_bytes(&row.pk);
                self.get(&bytes).transpose().map(|result| {
                    result.map(|value| IndexedDocumentRecord {
                        key: display_key(&bytes),
                        value,
                        field: row.value,
                    })
                })
            })
            .collect()
    }

    pub async fn create_compound_index(
        &self,
        name: &str,
        definition: CompoundIndexDef,
    ) -> Result<(), MiniDbError> {
        self.ensure_json_codec("compound")?;
        self.ensure_writable()?;
        let documents = self.live_json_documents_with_datetimes()?;
        {
            let mut derived = self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            derived.compound.create(name, definition)?;
            derived.compound.rebuild(
                documents
                    .iter()
                    .map(|(key, value, dt)| (key.as_str(), value, dt.as_ref())),
            );
        }
        self.persist_compound_indexes().await
    }

    pub async fn drop_compound_index(&self, name: &str) -> Result<bool, MiniDbError> {
        self.ensure_writable()?;
        let dropped = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .compound
            .drop(name);
        self.persist_compound_indexes().await?;
        Ok(dropped)
    }
    pub fn list_compound_indexes(&self) -> Result<Vec<CompoundIndexInfo>, MiniDbError> {
        Ok(self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .compound
            .list())
    }

    pub fn compound_range(
        &self,
        name: &str,
        group: &Value,
        options: &RangeOptions<OrderValue>,
    ) -> Result<Vec<CompoundDocumentRecord<V>>, MiniDbError> {
        let rows = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .compound
            .range(name, group, options)?;
        rows.into_iter()
            .filter_map(|row| {
                let bytes = key_bytes(&row.key);
                self.get(&bytes).transpose().map(|result| {
                    result.map(|value| CompoundDocumentRecord {
                        key: display_key(&bytes),
                        value,
                        order_value: row.order_value,
                    })
                })
            })
            .collect()
    }

    pub async fn create_text_index(
        &self,
        name: &str,
        fields: Option<Vec<String>>,
    ) -> Result<(), MiniDbError> {
        self.ensure_json_codec("text")?;
        self.ensure_writable()?;
        let documents = self.live_json_documents()?;
        let path = text_postings_path(&self.inner.directory, name);
        let mut index = TextIndex::new(TextIndexOptions {
            fields: fields.clone(),
            postings_path: Some(path.clone()),
            cache_terms: None,
        });
        index.build(documents.iter().map(|(key, value)| (key.as_str(), value)))?;
        {
            let mut derived = self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            if derived.text.contains_key(name) {
                return Err(MiniDbError::TextIndexExists(name.into()));
            }
            derived.text.insert(name.into(), index);
            derived.text_definitions.push(TextIndexDefinition {
                name: name.into(),
                fields,
            });
        }
        if let Err(error) = self.persist_text_indexes().await {
            {
                let mut derived = self
                    .inner
                    .derived
                    .lock()
                    .map_err(|_| MiniDbError::StatePoisoned)?;
                derived.text.remove(name);
                derived
                    .text_definitions
                    .retain(|definition| definition.name != name);
            }
            let _ = tokio::fs::remove_file(path).await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn drop_text_index(&self, name: &str) -> Result<bool, MiniDbError> {
        self.ensure_writable()?;
        let dropped = {
            let mut derived = self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            derived
                .text_definitions
                .retain(|definition| definition.name != name);
            derived.text.remove(name).is_some()
        };
        if dropped {
            let _ = tokio::fs::remove_file(text_postings_path(&self.inner.directory, name)).await;
        }
        self.persist_text_indexes().await?;
        Ok(dropped)
    }

    pub fn search(
        &self,
        name: &str,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchDocumentRecord<V>>, MiniDbError> {
        let hits = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .text
            .get_mut(name)
            .ok_or_else(|| MiniDbError::TextIndexNotFound(name.into()))?
            .search(query, options)?;
        hits.into_iter()
            .filter_map(|hit| {
                let bytes = key_bytes(&hit.key);
                self.get(&bytes).transpose().map(|result| {
                    result.map(|value| SearchDocumentRecord {
                        key: display_key(&bytes),
                        value,
                        score: hit.score,
                    })
                })
            })
            .collect()
    }

    // Original: MiniDb.query(). This general path preserves results while leaving source-specific fast paths to indexes.
    pub fn query(&self, options: &QueryOptions) -> Result<Vec<DocumentRecord<V>>, MiniDbError> {
        self.ensure_open()?;
        let mut keys = if let Some(key) = &options.key {
            if let Some(exact) = &key.exact {
                vec![exact.as_bytes().to_vec()]
            } else if let Some(prefix) = &key.prefix {
                self.inner
                    .store
                    .lock()
                    .map_err(|_| MiniDbError::StatePoisoned)?
                    .raw_keys(&RangeOptions {
                        gte: Some(prefix.as_bytes().to_vec()),
                        lt: Some({
                            let mut upper = prefix.as_bytes().to_vec();
                            upper.extend_from_slice("\u{ffff}".as_bytes());
                            upper
                        }),
                        ..Default::default()
                    })
            } else {
                self.inner
                    .store
                    .lock()
                    .map_err(|_| MiniDbError::StatePoisoned)?
                    .raw_keys(&key.range)
            }
        } else {
            self.inner
                .store
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?
                .raw_keys(&RangeOptions::default())
        };
        for (column, range) in &options.datetimes {
            let allowed = self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?
                .datetimes
                .range(column, range)
                .into_iter()
                .map(|entry| entry.key.into_bytes())
                .collect::<HashSet<_>>();
            keys.retain(|key| allowed.contains(key));
        }
        let mut text_rank = None;
        if let Some(text) = &options.text {
            let hits = self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?
                .text
                .get_mut(&text.index)
                .ok_or_else(|| MiniDbError::TextIndexNotFound(text.index.clone()))?
                .search(&text.query, &text.options)?;
            let rank = hits
                .iter()
                .enumerate()
                .map(|(index, hit)| (key_bytes(&hit.key), index))
                .collect::<HashMap<_, _>>();
            keys.retain(|key| rank.contains_key(key));
            text_rank = Some(rank);
        }
        let mut documents = Vec::new();
        for key in keys {
            let Some(mut record) = self.get_record(&key)? else {
                continue;
            };
            if let Some(filter) = &options.filter {
                let Some(document) = self.inner.codec.index_value(&record.value) else {
                    continue;
                };
                if !matches_filter(&document, filter.as_object()) {
                    continue;
                }
            }
            if let Some(fields) = &options.project
                && let Some(document) = self.inner.codec.index_value(&record.value)
            {
                let projected = project(&document, fields);
                record.value = self
                    .inner
                    .codec
                    .decode(&serde_json::to_vec(&projected)?)
                    .map_err(MiniDbError::Codec)?;
            }
            documents.push(record);
        }
        if let Some(rank) = text_rank {
            documents.sort_by_key(|record| {
                rank.get(record.key.as_bytes())
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }
        if !options.sort.is_empty() {
            documents.sort_by(|left, right| {
                let left = self
                    .inner
                    .codec
                    .index_value(&left.value)
                    .unwrap_or(Value::Null);
                let right = self
                    .inner
                    .codec
                    .index_value(&right.value)
                    .unwrap_or(Value::Null);
                for (path, direction) in &options.sort {
                    let order = compare_json_path(&left, &right, path);
                    if order != std::cmp::Ordering::Equal {
                        return if *direction < 0 {
                            order.reverse()
                        } else {
                            order
                        };
                    }
                }
                std::cmp::Ordering::Equal
            });
        }
        let start = options.skip.min(documents.len());
        let end = options.limit.map_or(documents.len(), |limit| {
            start.saturating_add(limit).min(documents.len())
        });
        Ok(documents.drain(start..end).collect())
    }

    pub async fn compact(&self) -> Result<(), MiniDbError> {
        self.ensure_open()?;
        self.ensure_writable()?;
        compact(self.compaction()).await?;
        Ok(())
    }

    pub async fn backup(
        &self,
        destination: impl AsRef<Path>,
        compact_first: bool,
    ) -> Result<(), MiniDbError> {
        self.ensure_open()?;
        if compact_first && !self.inner.read_only {
            self.compact().await?;
        }
        let _rotation = self.inner.rotation_gate.write().await;
        if !self.inner.read_only {
            self.inner.wal.read().await.flush().await?;
        }
        tokio::fs::create_dir_all(destination.as_ref()).await?;
        let mut copied = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.inner.directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if persistent_name(&name) {
                tokio::fs::copy(entry.path(), destination.as_ref().join(&name)).await?;
                copied.push(name);
            }
        }
        tokio::fs::write(
            destination.as_ref().join("backup.manifest.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({ "version": 1, "createdAt": now_millis(), "files": copied }),
            )?,
        )
        .await?;
        Ok(())
    }

    pub async fn restore(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        force: bool,
        mut options: OpenOptions<V>,
    ) -> Result<Self, MiniDbError> {
        if force {
            let _ = tokio::fs::remove_dir_all(destination.as_ref()).await;
        } else if let Ok(mut entries) = tokio::fs::read_dir(destination.as_ref()).await
            && entries.next_entry().await?.is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "restore destination is not empty",
            )
            .into());
        }
        tokio::fs::create_dir_all(destination.as_ref()).await?;
        let mut entries = tokio::fs::read_dir(source.as_ref()).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if persistent_name(&name) || name == "backup.manifest.json" {
                tokio::fs::copy(entry.path(), destination.as_ref().join(name)).await?;
            }
        }
        options.directory = destination.as_ref().to_owned();
        Self::open(options).await
    }

    pub async fn renew_lock(&self) -> Result<(), MiniDbError> {
        if let Some(lock) = &self.inner.lock {
            lock.renew().await?;
        }
        Ok(())
    }

    pub fn recovery_info(&self) -> &RecoveryInfo {
        &self.inner.recovery_info
    }

    pub async fn catch_up_from_wal(
        &self,
        offset: u64,
    ) -> Result<Option<CatchUpResult>, MiniDbError> {
        self.ensure_open()?;
        let anchor = {
            let tail = self
                .inner
                .wal_tail
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            tail.as_ref()
                .map(|tail| (tail.anchor, tail.offset))
                .or_else(|| {
                    (self.inner.recovery_info.wal_inode != 0).then_some((
                        WalAnchor {
                            device: self.inner.recovery_info.wal_device,
                            inode: self.inner.recovery_info.wal_inode,
                        },
                        self.inner.recovery_info.wal_scan_end,
                    ))
                })
        };
        let Some((anchor, expected)) = anchor else {
            return Ok(None);
        };
        if offset != expected {
            return Ok(None);
        }
        let result = catch_up_wal(&self.inner.wal_path, offset, anchor, |frame, file| {
            let operations = frame_to_ops(frame, ValueFile::Wal, file, self.inner.value_mode)?;
            for operation in operations {
                self.apply_recovered(operation)
                    .map_err(|error| RecoveryError::Io(io::Error::other(error.to_string())))?;
            }
            Ok(())
        })?;
        if let Some(result) = result {
            *self
                .inner
                .wal_tail
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)? = Some(WalTail {
                anchor,
                offset: result.offset,
            });
        }
        Ok(result)
    }

    fn apply_recovered(&self, operation: RecoveredOp) -> Result<(), MiniDbError> {
        let mut store = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        let mut derived = self
            .inner
            .derived
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?;
        match operation {
            RecoveredOp::Del { key } => {
                let old = store
                    .get(&key)?
                    .and_then(|bytes| self.inner.codec.decode(&bytes).ok())
                    .and_then(|value| self.inner.codec.index_value(&value));
                if store.del(&key) {
                    apply_derived_delete(&mut derived, &key, old.as_ref());
                }
            }
            RecoveredOp::Set {
                key,
                value_ref,
                expire_at,
                datetimes,
            } => {
                let old = store
                    .get(&key)?
                    .and_then(|bytes| self.inner.codec.decode(&bytes).ok())
                    .and_then(|value| self.inner.codec.index_value(&value));
                store.set_ref(key.clone(), value_ref, expire_at, datetimes.clone());
                if let Some(bytes) = store.get(&key)? {
                    let value = self
                        .inner
                        .codec
                        .decode(&bytes)
                        .map_err(MiniDbError::Codec)?;
                    let document = self.inner.codec.index_value(&value);
                    apply_derived_set(
                        &mut derived,
                        &key,
                        old.as_ref(),
                        document.as_ref(),
                        datetimes.as_ref(),
                    );
                    touch(&mut derived.access, &key);
                }
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> Result<MiniDbStats, MiniDbError> {
        Ok(self
            .inner
            .stats
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .clone())
    }

    pub fn compaction_stats(&self) -> Result<crate::compaction::CompactionStats, MiniDbError> {
        Ok(*self
            .compaction()
            .stats
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?)
    }

    pub async fn close(&self) -> Result<(), MiniDbError> {
        if self
            .inner
            .closed
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return Ok(());
        }
        if self.compaction().is_compacting() {
            let _ = compact(self.compaction()).await;
        }
        if let Some(task) = self
            .inner
            .expire_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
        {
            let mut derived = self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?;
            for index in derived.text.values_mut() {
                index.close();
            }
        }
        if let Some(reader) = &self.inner.value_reader {
            reader.close()?;
        }
        if !self.inner.read_only {
            self.inner.wal.read().await.close().await?;
        }
        if let Some(lock) = &self.inner.lock {
            lock.release().await?;
        }
        Ok(())
    }

    fn ensure_json_codec(&self, kind: &'static str) -> Result<(), MiniDbError> {
        if self.inner.codec.name() == CodecName::Json {
            Ok(())
        } else {
            Err(MiniDbError::JsonCodecRequired(kind))
        }
    }

    fn live_json_documents(&self) -> Result<Vec<(String, Value)>, MiniDbError> {
        Ok(self
            .live_json_documents_with_datetimes()?
            .into_iter()
            .map(|(key, value, _)| (key, value))
            .collect())
    }
    fn live_json_documents_with_datetimes(&self) -> Result<Vec<LiveJsonRecord>, MiniDbError> {
        let entries = self
            .inner
            .store
            .lock()
            .map_err(|_| MiniDbError::StatePoisoned)?
            .entries()?;
        entries
            .into_iter()
            .filter_map(|entry| {
                self.inner
                    .codec
                    .decode(&entry.value)
                    .ok()
                    .and_then(|value| self.inner.codec.index_value(&value))
                    .map(|value| (key_string(&entry.key), value, entry.datetimes))
            })
            .collect::<Vec<_>>()
            .pipe(Ok)
    }

    async fn persist_indexes(&self) -> Result<(), MiniDbError> {
        let data = serde_json::to_vec(&self.list_indexes()?)?;
        write_atomic(&self.inner.index_path, &data).await?;
        Ok(())
    }
    async fn persist_compound_indexes(&self) -> Result<(), MiniDbError> {
        let data = serde_json::to_vec(&self.list_compound_indexes()?)?;
        write_atomic(&self.inner.compound_index_path, &data).await?;
        Ok(())
    }
    async fn persist_text_indexes(&self) -> Result<(), MiniDbError> {
        let data = serde_json::to_vec(
            &self
                .inner
                .derived
                .lock()
                .map_err(|_| MiniDbError::StatePoisoned)?
                .text_definitions,
        )?;
        write_atomic(&self.inner.text_index_path, &data).await?;
        Ok(())
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}
impl<T> Pipe for T {}

fn check_key(key: &[u8]) -> Result<(), MiniDbError> {
    if key.is_empty() {
        Err(MiniDbError::EmptyKey)
    } else if key.len() > MAX_KEY_LEN {
        Err(MiniDbError::KeyTooLong)
    } else {
        Ok(())
    }
}
fn key_string(key: &[u8]) -> String {
    key.iter().map(|byte| char::from(*byte)).collect()
}

fn key_bytes(key: &str) -> Vec<u8> {
    key.chars()
        .map(|character| character as u32 as u8)
        .collect()
}

fn display_key(key: &[u8]) -> String {
    String::from_utf8_lossy(key).into_owned()
}

fn touch(access: &mut Vec<Vec<u8>>, key: &[u8]) {
    access.retain(|entry| entry != key);
    access.push(key.to_vec());
}

fn apply_derived_set(
    derived: &mut DerivedState,
    key: &[u8],
    old: Option<&Value>,
    new: Option<&Value>,
    datetimes: Option<&BTreeMap<String, f64>>,
) {
    let key = key_string(key);
    derived.datetimes.set(&key, datetimes);
    if let Some(old) = old {
        derived.indexes.remove(&key);
        let _ = old;
    }
    if let Some(new) = new {
        derived.indexes.add(&key, new);
        derived.compound.add(&key, new, datetimes);
        for index in derived.text.values_mut() {
            index.add(&key, new);
        }
    } else {
        derived.compound.remove(&key);
        for index in derived.text.values_mut() {
            index.remove(&key);
        }
    }
}

fn apply_derived_delete(derived: &mut DerivedState, key: &[u8], _old: Option<&Value>) {
    let key_string = key_string(key);
    derived.datetimes.delete(&key_string);
    derived.compound.remove(&key_string);
    derived.indexes.remove(&key_string);
    for index in derived.text.values_mut() {
        index.remove(&key_string);
    }
    derived.access.retain(|entry| entry != key);
}

fn drain_expired(store: &mut Store, derived: &mut DerivedState) {
    for (key, _) in store.take_expired() {
        apply_derived_delete(derived, &key, None);
    }
}

fn rebuild_derived<V>(
    store: &Store,
    codec: &Arc<dyn ValueCodec<V>>,
    derived: &mut DerivedState,
) -> Result<(), MiniDbError> {
    let entries = store.entries()?;
    let mut documents = Vec::new();
    for entry in &entries {
        if let Ok(value) = codec.decode(&entry.value)
            && let Some(document) = codec.index_value(&value)
        {
            documents.push((key_string(&entry.key), document, entry.datetimes.clone()));
        }
    }
    derived.indexes.rebuild(
        documents
            .iter()
            .map(|(key, value, _)| (key.as_str(), value)),
    );
    derived.datetimes.rebuild(
        entries
            .iter()
            .map(|entry| (key_string(&entry.key), entry.datetimes.as_ref()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(key, dt)| (key.as_str(), *dt)),
    );
    derived.compound.rebuild(
        documents
            .iter()
            .map(|(key, value, dt)| (key.as_str(), value, dt.as_ref())),
    );
    for index in derived.text.values_mut() {
        index.build(
            documents
                .iter()
                .map(|(key, value, _)| (key.as_str(), value)),
        )?;
    }
    Ok(())
}

fn rebuild_text_indexes<V>(inner: &MiniDbInner<V>) -> Result<(), MiniDbError> {
    let entries = inner
        .store
        .lock()
        .map_err(|_| MiniDbError::StatePoisoned)?
        .entries()?;
    let documents = entries
        .iter()
        .filter_map(|entry| {
            inner
                .codec
                .decode(&entry.value)
                .ok()
                .and_then(|value| inner.codec.index_value(&value))
                .map(|value| (key_string(&entry.key), value))
        })
        .collect::<Vec<_>>();
    let mut derived = inner
        .derived
        .lock()
        .map_err(|_| MiniDbError::StatePoisoned)?;
    for index in derived.text.values_mut() {
        index.build(documents.iter().map(|(key, value)| (key.as_str(), value)))?;
    }
    Ok(())
}

async fn load_definitions(
    directory: &Path,
    read_only: bool,
    derived: &mut DerivedState,
) -> Result<(), MiniDbError> {
    if let Some(definitions) =
        read_json::<Vec<IndexInfo>>(&directory.join("db.indexes.json")).await?
    {
        for definition in definitions {
            derived.indexes.create(
                &definition.name,
                IndexDef {
                    field: definition.field,
                    index_type: definition.index_type,
                    unique: definition.unique,
                    sparse: definition.sparse,
                },
            )?;
        }
    }
    if let Some(definitions) =
        read_json::<Vec<CompoundIndexInfo>>(&directory.join("db.compound-indexes.json")).await?
    {
        for definition in definitions {
            derived.compound.create(
                &definition.name,
                CompoundIndexDef {
                    group_by: definition.group_by,
                    order_by: definition.order_by,
                    order_type: definition.order_type,
                },
            )?;
        }
    }
    derived.text_definitions = read_json(&directory.join("db.textindexes.json"))
        .await?
        .unwrap_or_default();
    for definition in &derived.text_definitions {
        derived.text.insert(
            definition.name.clone(),
            TextIndex::new(TextIndexOptions {
                fields: definition.fields.clone(),
                postings_path: (!read_only)
                    .then(|| text_postings_path(directory, &definition.name)),
                cache_terms: None,
            }),
        );
    }
    Ok(())
}

async fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, MiniDbError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    tokio::fs::write(&temporary, data).await?;
    tokio::fs::rename(temporary, path).await
}

async fn resolve_value_mode(
    setting: ValueModeSetting,
    directory: &Path,
    maximum: Option<usize>,
) -> io::Result<ValueMode> {
    match setting {
        ValueModeSetting::Memory => Ok(ValueMode::Memory),
        ValueModeSetting::Disk => Ok(ValueMode::Disk),
        ValueModeSetting::Auto => {
            let Some(maximum) = maximum else {
                return Ok(ValueMode::Memory);
            };
            let mut size = 0;
            for name in ["db.snapshot", "db.wal"] {
                match tokio::fs::metadata(directory.join(name)).await {
                    Ok(metadata) => size += metadata.len(),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(if size > maximum as u64 {
                ValueMode::Disk
            } else {
                ValueMode::Memory
            })
        }
    }
}

async fn cleanup_temporary_files(directory: &Path) -> io::Result<()> {
    for name in [
        "db.snapshot.tmp",
        "db.wal.tmp",
        "db.indexes.json.tmp",
        "db.textindexes.json.tmp",
        "db.compound-indexes.json.tmp",
    ] {
        let _ = tokio::fs::remove_file(directory.join(name)).await;
    }
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("db.text-") && name.ends_with(".postings.tmp") {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    Ok(())
}

fn text_postings_path(directory: &Path, name: &str) -> PathBuf {
    let safe = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || "_.-".contains(character) {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    directory.join(format!("db.text-{safe}.postings"))
}

fn persistent_name(name: &str) -> bool {
    matches!(
        name,
        "db.snapshot"
            | "db.wal"
            | "db.indexes.json"
            | "db.compound-indexes.json"
            | "db.textindexes.json"
    ) || (name.starts_with("db.text-") && name.ends_with(".postings"))
}

fn compare_json_path(left: &Value, right: &Value, path: &str) -> std::cmp::Ordering {
    let left = crate::query::get_path(left, path).unwrap_or(&Value::Null);
    let right = crate::query::get_path(right, path).unwrap_or(&Value::Null);
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left
            .as_f64()
            .partial_cmp(&right.as_f64())
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(left), Value::String(right)) => left.cmp(right),
        _ => left.to_string().cmp(&right.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opens_persists_recovers_and_queries_json_documents() {
        let directory = tempfile::tempdir().unwrap();
        let database = MiniDb::open(MiniDb::<Value>::json_options(directory.path()))
            .await
            .unwrap();
        database
            .set(
                "a",
                serde_json::json!({"name":"alpha","score":2}),
                SetOptions::default(),
            )
            .await
            .unwrap();
        database
            .set(
                "b",
                serde_json::json!({"name":"beta","score":1}),
                SetOptions::default(),
            )
            .await
            .unwrap();
        database
            .create_index("name", IndexDef::equality("name"))
            .await
            .unwrap();
        assert_eq!(
            database.find_eq("name", &Value::from("alpha")).unwrap()[0].key,
            "a"
        );
        database.close().await.unwrap();
        let reopened = MiniDb::open(MiniDb::<Value>::json_options(directory.path()))
            .await
            .unwrap();
        assert_eq!(reopened.get("a").unwrap().unwrap()["score"], 2);
        reopened.close().await.unwrap();
    }

    #[tokio::test]
    async fn batch_ttl_scan_and_backup_work() {
        let directory = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let database = MiniDb::open(MiniDb::<String>::string_options(directory.path()))
            .await
            .unwrap();
        database
            .batch(vec![
                BatchInputOp::Set {
                    key: "a".into(),
                    value: "1".into(),
                    options: SetOptions::default(),
                },
                BatchInputOp::Set {
                    key: "b".into(),
                    value: "2".into(),
                    options: SetOptions::default(),
                },
            ])
            .await
            .unwrap();
        assert_eq!(database.scan(&RangeOptions::default()).unwrap().len(), 2);
        database.expire("a", -1.0).await.unwrap();
        assert_eq!(database.get("a").unwrap(), None);
        database.backup(backup.path(), false).await.unwrap();
        database.close().await.unwrap();
    }
}
