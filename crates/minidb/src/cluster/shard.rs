use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    codec::CorruptionMode,
    minidb::{MemoryPolicy, MiniDb, MiniDbError, OpenOptions, ValueCodec, ValueModeSetting},
    wal::FsyncPolicy,
};

pub struct ShardOpenOptions<V> {
    pub codec: Arc<dyn ValueCodec<V>>,
    pub fsync_policy: FsyncPolicy,
    pub value_mode: ValueModeSetting,
    pub compact_threshold_bytes: u64,
    pub auto_compact: bool,
    pub active_expire_interval: Duration,
    pub recovery_mode: CorruptionMode,
    pub max_memory_bytes: Option<usize>,
    pub max_memory_policy: MemoryPolicy,
}

impl<V> Clone for ShardOpenOptions<V> {
    fn clone(&self) -> Self {
        Self {
            codec: Arc::clone(&self.codec),
            fsync_policy: self.fsync_policy,
            value_mode: self.value_mode,
            compact_threshold_bytes: self.compact_threshold_bytes,
            auto_compact: self.auto_compact,
            active_expire_interval: self.active_expire_interval,
            recovery_mode: self.recovery_mode,
            max_memory_bytes: self.max_memory_bytes,
            max_memory_policy: self.max_memory_policy,
        }
    }
}

impl<V> ShardOpenOptions<V> {
    pub fn from_open(options: &OpenOptions<V>) -> Self {
        Self {
            codec: Arc::clone(&options.codec),
            fsync_policy: options.fsync_policy,
            value_mode: options.value_mode,
            compact_threshold_bytes: options.compact_threshold_bytes,
            auto_compact: options.auto_compact,
            active_expire_interval: options.active_expire_interval,
            recovery_mode: options.recovery_mode,
            max_memory_bytes: options.max_memory_bytes,
            max_memory_policy: options.max_memory_policy,
        }
    }

    fn open_options(&self, directory: PathBuf, read_only: bool) -> OpenOptions<V> {
        let mut options = OpenOptions::new(directory, Arc::clone(&self.codec));
        options.fsync_policy = if read_only {
            FsyncPolicy::No
        } else {
            self.fsync_policy
        };
        options.value_mode = self.value_mode;
        options.compact_threshold_bytes = self.compact_threshold_bytes;
        options.auto_compact = !read_only && self.auto_compact;
        options.active_expire_interval = self.active_expire_interval;
        options.recovery_mode = self.recovery_mode;
        options.max_memory_bytes = self.max_memory_bytes;
        options.max_memory_policy = self.max_memory_policy;
        options.read_only = read_only;
        options
    }
}

pub struct ShardHandle<V> {
    pub shard_id: usize,
    pub directory: PathBuf,
    pub database: MiniDb<V>,
    pub writer: bool,
    lease_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl<V: Send + Sync + 'static> ShardHandle<V> {
    pub async fn open_writer(
        shard_id: usize,
        directory: PathBuf,
        options: &ShardOpenOptions<V>,
        renew_millis: u64,
    ) -> Result<Arc<Self>, MiniDbError> {
        let database = MiniDb::open(options.open_options(directory.clone(), false)).await?;
        let handle = Arc::new(Self {
            shard_id,
            directory,
            database,
            writer: true,
            lease_task: Mutex::new(None),
        });
        if renew_millis > 0 {
            let database = handle.database.clone();
            let task = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(renew_millis)).await;
                    if database.renew_lock().await.is_err() {
                        break;
                    }
                }
            });
            *handle
                .lease_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
        }
        Ok(handle)
    }

    pub async fn open_reader(
        shard_id: usize,
        directory: PathBuf,
        options: &ShardOpenOptions<V>,
    ) -> Result<Arc<Self>, MiniDbError> {
        let database = MiniDb::open(options.open_options(directory.clone(), true)).await?;
        Ok(Arc::new(Self {
            shard_id,
            directory,
            database,
            writer: false,
            lease_task: Mutex::new(None),
        }))
    }

    pub async fn close(&self) -> Result<(), MiniDbError> {
        if let Some(task) = self
            .lease_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
        self.database.close().await
    }
}
