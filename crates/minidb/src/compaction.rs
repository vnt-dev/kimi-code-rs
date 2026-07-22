use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use thiserror::Error;
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Mutex as AsyncMutex, Notify, RwLock},
};

use crate::{
    rename_replace::{RenameReplaceOptions, rename_replace},
    snapshot::{SnapshotError, write_snapshot_entries},
    store::{Store, StoreError, ValueFile, ValueLoc},
    value_reader::{PositionedValueReader, ValueReaderError},
    wal::{FsyncPolicy, Wal, WalError, WalOptions, WalStats},
};

const COPY_CHUNK: usize = 1 << 20;
const SMALL_DELTA: u64 = 64 * 1024;
const MAX_PRECOPY_PASSES: usize = 5;
const CONVERGE_RATIO: f64 = 0.7;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CompactionStats {
    pub compactions: u64,
    pub snapshot_bytes_written: u64,
    pub compact_errors: u64,
}

#[derive(Default)]
struct RunState {
    running: bool,
    generation: u64,
    last_error: Option<String>,
}

pub struct CompactionTarget {
    pub directory: PathBuf,
    pub wal_path: PathBuf,
    pub fsync_policy: FsyncPolicy,
    pub store: Arc<Mutex<Store>>,
    pub wal: Arc<RwLock<Wal>>,
    pub wal_stats: Arc<Mutex<WalStats>>,
    pub compact_threshold_bytes: u64,
    pub rotation_gate: Arc<RwLock<()>>,
    pub value_reader: Option<Arc<PositionedValueReader>>,
    pub on_compacted: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    pub stats: Arc<Mutex<CompactionStats>>,
    compacting: AtomicBool,
    run_state: AsyncMutex<RunState>,
    completed: Notify,
}

impl CompactionTarget {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        directory: impl Into<PathBuf>,
        wal_path: impl Into<PathBuf>,
        fsync_policy: FsyncPolicy,
        store: Arc<Mutex<Store>>,
        wal: Arc<RwLock<Wal>>,
        wal_stats: Arc<Mutex<WalStats>>,
        compact_threshold_bytes: u64,
        rotation_gate: Arc<RwLock<()>>,
    ) -> Self {
        Self {
            directory: directory.into(),
            wal_path: wal_path.into(),
            fsync_policy,
            store,
            wal,
            wal_stats,
            compact_threshold_bytes,
            rotation_gate,
            value_reader: None,
            on_compacted: None,
            stats: Arc::new(Mutex::new(CompactionStats::default())),
            compacting: AtomicBool::new(false),
            run_state: AsyncMutex::new(RunState::default()),
            completed: Notify::new(),
        }
    }

    pub fn is_compacting(&self) -> bool {
        self.compacting.load(Ordering::Acquire)
    }

    pub async fn should_compact(&self) -> bool {
        self.wal.read().await.size() >= self.compact_threshold_bytes
    }
}

#[derive(Debug, Error)]
pub enum CompactionError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    ValueReader(#[from] ValueReaderError),
    #[error("compaction hook failed: {0}")]
    Hook(String),
    #[error("concurrent compaction failed: {0}")]
    Concurrent(String),
    #[error("store lock is poisoned")]
    StorePoisoned,
    #[error("statistics lock is poisoned")]
    StatsPoisoned,
}

pub async fn fsync_directory(directory: impl AsRef<Path>) {
    if let Ok(file) = File::open(directory).await {
        let _ = file.sync_all().await;
    }
}

// Original: packages/minidb/src/compaction.ts, copyFileRange().
pub async fn copy_file_range(
    source_path: impl AsRef<Path>,
    destination_path: impl AsRef<Path>,
    start: u64,
    end: u64,
    append: bool,
) -> Result<(), CompactionError> {
    if end < start {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("copy_file_range: end ({end}) < start ({start})"),
        )
        .into());
    }
    let mut destination = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(destination_path)
        .await?;
    if end > start {
        let mut source = File::open(source_path).await?;
        source.seek(std::io::SeekFrom::Start(start)).await?;
        let mut position = start;
        let mut buffer = vec![0; COPY_CHUNK];
        while position < end {
            let length = (end - position).min(COPY_CHUNK as u64) as usize;
            let count = source.read(&mut buffer[..length]).await?;
            if count == 0 {
                break;
            }
            destination.write_all(&buffer[..count]).await?;
            position += count as u64;
        }
    }
    destination.sync_all().await?;
    Ok(())
}

// Original: packages/minidb/src/compaction.ts, compact()/runCompaction().
pub async fn compact(target: &CompactionTarget) -> Result<(), CompactionError> {
    loop {
        let notified = target.completed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let mut state = target.run_state.lock().await;
        if state.running {
            let generation = state.generation;
            drop(state);
            notified.await;
            let state = target.run_state.lock().await;
            if state.generation == generation {
                continue;
            }
            return state
                .last_error
                .clone()
                .map_or(Ok(()), |error| Err(CompactionError::Concurrent(error)));
        }
        state.running = true;
        target.compacting.store(true, Ordering::Release);
        drop(state);
        break;
    }

    let result = run_compaction(target).await.and_then(|()| {
        target
            .on_compacted
            .as_ref()
            .map_or(Ok(()), |hook| hook().map_err(CompactionError::Hook))
    });
    {
        let mut stats = target
            .stats
            .lock()
            .map_err(|_| CompactionError::StatsPoisoned)?;
        if result.is_ok() {
            stats.compactions += 1;
        } else {
            stats.compact_errors += 1;
        }
    }
    let mut state = target.run_state.lock().await;
    state.running = false;
    state.generation = state.generation.wrapping_add(1);
    state.last_error = result.as_ref().err().map(ToString::to_string);
    target.compacting.store(false, Ordering::Release);
    drop(state);
    target.completed.notify_waiters();
    result
}

async fn run_compaction(target: &CompactionTarget) -> Result<(), CompactionError> {
    let temporary_snapshot = target.directory.join("db.snapshot.tmp");
    let snapshot_path = target.directory.join("db.snapshot");
    let temporary_wal = target.directory.join("db.wal.tmp");
    let old_wal = target.wal.read().await.clone();
    old_wal.flush().await?;
    let base_offset = old_wal.size();

    // Materializing the live view is a short synchronous fence. Snapshot encoding and disk I/O remain non-blocking.
    let entries = target
        .store
        .lock()
        .map_err(|_| CompactionError::StorePoisoned)?
        .entries()?;
    let snapshot = write_snapshot_entries(entries, &temporary_snapshot, 2_000).await?;
    target
        .stats
        .lock()
        .map_err(|_| CompactionError::StatsPoisoned)?
        .snapshot_bytes_written += snapshot.bytes;

    let mut copied_up_to = base_offset;
    let mut appended = false;
    let mut previous_gap = u64::MAX;
    for pass in 0..MAX_PRECOPY_PASSES {
        old_wal.flush().await?;
        let head = old_wal.size();
        let gap = head.saturating_sub(copied_up_to);
        if gap <= SMALL_DELTA {
            break;
        }
        if pass > 0 && gap as f64 > previous_gap as f64 * CONVERGE_RATIO {
            break;
        }
        copy_file_range(
            &target.wal_path,
            &temporary_wal,
            copied_up_to,
            head,
            appended,
        )
        .await?;
        appended = true;
        copied_up_to = head;
        previous_gap = gap;
    }

    // Writers acquire a read guard around append; the write guard is the short rotation critical section.
    let _rotation = target.rotation_gate.write().await;
    let mut rotated = false;
    let mut remapped = false;
    let rotation_result = async {
        old_wal.seal();
        loop {
            old_wal.flush().await?;
            let end = old_wal.size();
            if end == copied_up_to && appended {
                break;
            }
            copy_file_range(
                &target.wal_path,
                &temporary_wal,
                copied_up_to,
                end,
                appended,
            )
            .await?;
            appended = true;
            copied_up_to = end;
        }
        old_wal.close().await?;
        #[cfg(windows)]
        if let Some(reader) = &target.value_reader {
            reader.close()?;
        }
        rename_replace(
            &temporary_snapshot,
            &snapshot_path,
            RenameReplaceOptions::default(),
        )
        .await?;
        fsync_directory(&target.directory).await;
        rename_replace(
            &temporary_wal,
            &target.wal_path,
            RenameReplaceOptions::default(),
        )
        .await?;
        rotated = true;
        fsync_directory(&target.directory).await;
        let fresh = new_wal(target);
        fresh.open().await?;
        *target.wal.write().await = fresh;
        remap_store(target, base_offset, &snapshot.locations)?;
        remapped = true;
        if let Some(reader) = &target.value_reader {
            reader.reopen_both()?;
        }
        Ok::<(), CompactionError>(())
    }
    .await;

    if let Err(error) = rotation_result {
        let _ = old_wal.close().await;
        let fresh = new_wal(target);
        if fresh.open().await.is_ok() {
            *target.wal.write().await = fresh;
            if rotated {
                if !remapped {
                    let _ = remap_store(target, base_offset, &snapshot.locations);
                }
                if let Some(reader) = &target.value_reader {
                    let _ = reader.reopen_both();
                }
            }
        }
        return Err(error);
    }
    Ok(())
}

fn new_wal(target: &CompactionTarget) -> Wal {
    Wal::new(
        &target.wal_path,
        WalOptions {
            fsync_policy: target.fsync_policy,
            stats: Some(Arc::clone(&target.wal_stats)),
            ..Default::default()
        },
    )
}

fn remap_store(
    target: &CompactionTarget,
    base_offset: u64,
    snapshot_locations: &HashMap<Vec<u8>, ValueLoc>,
) -> Result<(), CompactionError> {
    target
        .store
        .lock()
        .map_err(|_| CompactionError::StorePoisoned)?
        .remap_locations(|key, location, _| {
            if location.file == ValueFile::Wal && location.offset >= base_offset {
                Some(ValueLoc {
                    file: ValueFile::Wal,
                    offset: location.offset - base_offset,
                    len: location.len,
                })
            } else {
                snapshot_locations.get(key).copied()
            }
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        codec::{Frame, TYPE_SET, encode_frame},
        store::StoreOptions,
    };

    use super::*;

    #[tokio::test]
    async fn compacts_snapshot_and_rotates_wal() {
        let directory = tempfile::tempdir().unwrap();
        let wal_path = directory.path().join("db.wal");
        let wal_stats = Arc::new(Mutex::new(WalStats::default()));
        let wal = Wal::new(
            &wal_path,
            WalOptions {
                fsync_policy: FsyncPolicy::No,
                stats: Some(Arc::clone(&wal_stats)),
                ..Default::default()
            },
        );
        wal.open().await.unwrap();
        wal.append(
            encode_frame(&Frame {
                frame_type: TYPE_SET,
                key: b"a".to_vec(),
                value: b"1".to_vec(),
                meta: None,
                expire_at: 0,
            })
            .unwrap(),
        )
        .await
        .unwrap();
        let store = Arc::new(Mutex::new(Store::new(StoreOptions::default())));
        store
            .lock()
            .unwrap()
            .set(b"a".to_vec(), b"1".to_vec(), 0, None);
        let target = CompactionTarget::new(
            directory.path(),
            &wal_path,
            FsyncPolicy::No,
            store,
            Arc::new(RwLock::new(wal)),
            wal_stats,
            1,
            Arc::new(RwLock::new(())),
        );
        compact(&target).await.unwrap();
        assert!(directory.path().join("db.snapshot").exists());
        assert_eq!(tokio::fs::metadata(&wal_path).await.unwrap().len(), 0);
        assert_eq!(target.stats.lock().unwrap().compactions, 1);
    }
}
