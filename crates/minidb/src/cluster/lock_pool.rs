use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::sync::Mutex;

use crate::{lockfile::LockError, minidb::MiniDbError, recovery::WalAnchor};

use super::shard::{ShardHandle, ShardOpenOptions};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockPoolStats {
    pub writer_opens: u64,
    pub reader_opens: u64,
    pub reader_reopens: u64,
    pub incremental_catchups: u64,
    pub catchup_frames_applied: u64,
    pub lock_waits: u64,
    pub evictions: u64,
}

pub struct LockPoolOptions<V> {
    pub writer_options: ShardOpenOptions<V>,
    pub reader_options: ShardOpenOptions<V>,
    pub lock_renew_millis: u64,
    pub lock_acquire_timeout_millis: u64,
    pub lock_hold_millis: u64,
    pub max_writers: usize,
    pub max_readers: usize,
    pub read_only: bool,
}

struct WriterEntry<V> {
    handle: Arc<ShardHandle<V>>,
    last_used: Instant,
    opened: Instant,
}
struct ReaderEntry<V> {
    handle: Arc<ShardHandle<V>>,
    fingerprint: Vec<String>,
    wal_mark: Option<(WalAnchor, u64)>,
    last_used: Instant,
}

struct PoolState<V> {
    writers: HashMap<usize, WriterEntry<V>>,
    readers: HashMap<usize, ReaderEntry<V>>,
    closed: bool,
    stats: LockPoolStats,
}

#[derive(Debug, Error)]
pub enum LockPoolError {
    #[error("ClusterDb is open in read-only mode")]
    ReadOnly,
    #[error("ClusterDb is closed")]
    Closed,
    #[error(transparent)]
    Database(#[from] MiniDbError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub struct ShardLockPool<V> {
    options: LockPoolOptions<V>,
    state: Mutex<PoolState<V>>,
}

impl<V: Send + Sync + 'static> ShardLockPool<V> {
    pub fn new(options: LockPoolOptions<V>) -> Self {
        Self {
            options,
            state: Mutex::new(PoolState {
                writers: HashMap::new(),
                readers: HashMap::new(),
                closed: false,
                stats: LockPoolStats::default(),
            }),
        }
    }

    // Original: packages/minidb/src/cluster/lock-pool.ts, withWriter()/acquireWriter().
    pub async fn writer(
        self: &Arc<Self>,
        shard_id: usize,
        directory: PathBuf,
    ) -> Result<Arc<ShardHandle<V>>, LockPoolError> {
        if self.options.read_only {
            return Err(LockPoolError::ReadOnly);
        }
        let mut state = self.state.lock().await;
        if state.closed {
            return Err(LockPoolError::Closed);
        }
        if let Some(entry) = state.writers.get_mut(&shard_id)
            && (self.options.lock_hold_millis == 0
                || entry.opened.elapsed() < Duration::from_millis(self.options.lock_hold_millis)
                || Arc::strong_count(&entry.handle) > 1)
        {
            entry.last_used = Instant::now();
            return Ok(Arc::clone(&entry.handle));
        }
        if let Some(entry) = state.writers.remove(&shard_id) {
            entry.handle.close().await?;
        }
        let deadline =
            Instant::now() + Duration::from_millis(self.options.lock_acquire_timeout_millis);
        let mut delay = Duration::from_millis(10);
        let handle = loop {
            match ShardHandle::open_writer(
                shard_id,
                directory.clone(),
                &self.options.writer_options,
                self.options.lock_renew_millis,
            )
            .await
            {
                Ok(handle) => break handle,
                Err(error) if is_locked(&error) && Instant::now() + delay <= deadline => {
                    state.stats.lock_waits += 1;
                    drop(state);
                    tokio::time::sleep(delay).await;
                    state = self.state.lock().await;
                    delay = (delay * 2).min(Duration::from_millis(250));
                }
                Err(error) => return Err(error.into()),
            }
        };
        state.stats.writer_opens += 1;
        state.writers.insert(
            shard_id,
            WriterEntry {
                handle: Arc::clone(&handle),
                last_used: Instant::now(),
                opened: Instant::now(),
            },
        );
        evict_writers(&mut state, self.options.max_writers).await;
        if self.options.lock_hold_millis > 0 {
            let pool = Arc::downgrade(self);
            let leased_handle = Arc::clone(&handle);
            let hold = Duration::from_millis(self.options.lock_hold_millis);
            tokio::spawn(async move {
                tokio::time::sleep(hold).await;
                if let Some(pool) = pool.upgrade() {
                    pool.retire_writer(shard_id, &leased_handle).await;
                }
            });
        }
        Ok(handle)
    }

    async fn retire_writer(&self, shard_id: usize, leased_handle: &Arc<ShardHandle<V>>) {
        loop {
            let mut state = self.state.lock().await;
            let Some(entry) = state.writers.get(&shard_id) else {
                return;
            };
            if !Arc::ptr_eq(&entry.handle, leased_handle) {
                return;
            }
            if Arc::strong_count(&entry.handle) <= 2 {
                let entry = state.writers.remove(&shard_id).expect("entry was checked");
                drop(state);
                let _ = entry.handle.close().await;
                return;
            }
            drop(state);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    pub async fn invalidate_writer(&self, shard_id: usize) -> Result<(), LockPoolError> {
        let entry = self.state.lock().await.writers.remove(&shard_id);
        if let Some(entry) = entry {
            entry.handle.close().await?;
        }
        Ok(())
    }

    // Original: withReader()/refreshReader(). Cached readers are fingerprint revalidated before every use.
    pub async fn reader(
        &self,
        shard_id: usize,
        directory: PathBuf,
    ) -> Result<Arc<ShardHandle<V>>, LockPoolError> {
        let (mut state, fingerprint) = loop {
            let mut state = self.state.lock().await;
            if state.closed {
                return Err(LockPoolError::Closed);
            }
            if !self.options.read_only
                && let Some(writer) = state.writers.get_mut(&shard_id)
            {
                writer.last_used = Instant::now();
                return Ok(Arc::clone(&writer.handle));
            }
            let fingerprint = shard_fingerprint(&directory).await;
            let reader_busy = state.readers.get(&shard_id).is_some_and(|reader| {
                reader.fingerprint != fingerprint && Arc::strong_count(&reader.handle) > 1
            });
            if !reader_busy {
                break (state, fingerprint);
            }
            drop(state);
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        if let Some(reader) = state.readers.get_mut(&shard_id) {
            if reader.fingerprint == fingerprint {
                reader.last_used = Instant::now();
                return Ok(Arc::clone(&reader.handle));
            }
            let wal_only = reader.fingerprint.get(1..) == fingerprint.get(1..);
            if wal_only
                && let Some((anchor, offset)) = reader.wal_mark
                && let Ok(Some(result)) = reader.handle.database.catch_up_from_wal(offset).await
            {
                reader.wal_mark = Some((anchor, result.offset));
                reader.fingerprint = fingerprint;
                reader.last_used = Instant::now();
                let handle = Arc::clone(&reader.handle);
                let applied_frames = result.applied_frames as u64;
                state.stats.incremental_catchups += 1;
                state.stats.catchup_frames_applied += applied_frames;
                return Ok(handle);
            }
        }
        if let Some(entry) = state.readers.remove(&shard_id) {
            state.stats.reader_reopens += 1;
            if Arc::strong_count(&entry.handle) == 1 {
                entry.handle.close().await?;
            }
        }
        let mut last_error = None;
        let mut opened = None;
        for attempt in 0..3 {
            match ShardHandle::open_reader(
                shard_id,
                directory.clone(),
                &self.options.reader_options,
            )
            .await
            {
                Ok(handle) => {
                    opened = Some(handle);
                    break;
                }
                Err(error) => {
                    last_error = Some(error);
                    if attempt < 2 {
                        drop(state);
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        state = self.state.lock().await;
                    }
                }
            }
        }
        let handle = opened
            .ok_or_else(|| LockPoolError::Database(last_error.expect("failed open has error")))?;
        state.stats.reader_opens += 1;
        let recovery = handle.database.recovery_info();
        let wal_mark = (recovery.wal_inode != 0).then_some((
            WalAnchor {
                device: recovery.wal_device,
                inode: recovery.wal_inode,
            },
            recovery.wal_scan_end,
        ));
        let opened_fingerprint = shard_fingerprint(&directory).await;
        state.readers.insert(
            shard_id,
            ReaderEntry {
                handle: Arc::clone(&handle),
                fingerprint: opened_fingerprint,
                wal_mark,
                last_used: Instant::now(),
            },
        );
        evict_readers(&mut state, self.options.max_readers).await;
        Ok(handle)
    }

    pub async fn cached_counts(&self) -> (usize, usize) {
        let state = self.state.lock().await;
        (state.writers.len(), state.readers.len())
    }
    pub async fn stats(&self) -> LockPoolStats {
        self.state.lock().await.stats.clone()
    }

    pub async fn close_all(&self) -> Result<(), LockPoolError> {
        let mut state = self.state.lock().await;
        if state.closed {
            return Ok(());
        }
        state.closed = true;
        let mut handles = state
            .writers
            .drain()
            .map(|(_, entry)| entry.handle)
            .collect::<Vec<_>>();
        handles.extend(state.readers.drain().map(|(_, entry)| entry.handle));
        drop(state);
        for handle in handles {
            handle.close().await?;
        }
        Ok(())
    }
}

fn is_locked(error: &MiniDbError) -> bool {
    matches!(error, MiniDbError::Lock(LockError::Locked(_)))
}

async fn evict_writers<V: Send + Sync + 'static>(state: &mut PoolState<V>, maximum: usize) {
    while state.writers.len() > maximum {
        let victim = state
            .writers
            .iter()
            .filter(|(_, entry)| Arc::strong_count(&entry.handle) == 1)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(&id, _)| id);
        let Some(victim) = victim else {
            break;
        };
        if let Some(entry) = state.writers.remove(&victim) {
            state.stats.evictions += 1;
            let _ = entry.handle.close().await;
        }
    }
}

async fn evict_readers<V: Send + Sync + 'static>(state: &mut PoolState<V>, maximum: usize) {
    while state.readers.len() > maximum {
        let victim = state
            .readers
            .iter()
            .filter(|(_, entry)| Arc::strong_count(&entry.handle) == 1)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(&id, _)| id);
        let Some(victim) = victim else {
            break;
        };
        if let Some(entry) = state.readers.remove(&victim) {
            state.stats.evictions += 1;
            let _ = entry.handle.close().await;
        }
    }
}

async fn shard_fingerprint(directory: &Path) -> Vec<String> {
    let mut output = Vec::new();
    for name in [
        "db.wal",
        "db.snapshot",
        "db.indexes.json",
        "db.textindexes.json",
    ] {
        output.push(match tokio::fs::metadata(directory.join(name)).await {
            Ok(metadata) => format!("{:?}:{}", metadata.modified().ok(), metadata.len()),
            Err(_) => "-".into(),
        });
    }
    output
}
