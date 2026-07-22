use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use thiserror::Error;
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
    sync::{Mutex as AsyncMutex, Notify, oneshot},
    task::JoinHandle,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FsyncPolicy {
    #[serde(rename = "always")]
    Always,
    #[serde(rename = "everysec")]
    EverySecond,
    #[serde(rename = "no")]
    No,
}

#[derive(Debug, Clone)]
pub struct WalOptions {
    pub fsync_policy: FsyncPolicy,
    pub sync_interval: Duration,
    pub stats: Option<Arc<Mutex<WalStats>>>,
}

impl Default for WalOptions {
    fn default() -> Self {
        Self {
            fsync_policy: FsyncPolicy::EverySecond,
            sync_interval: Duration::from_secs(1),
            stats: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalStats {
    pub bytes_written: u64,
    pub fsyncs: u64,
}

#[derive(Debug, Error)]
pub enum WalError {
    #[error("WAL is closed")]
    Closed,
    #[error("WAL is sealed by a compaction rotation; retry against the new WAL")]
    Sealed,
    #[error("WAL is not open")]
    NotOpen,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("WAL completion channel closed")]
    CompletionClosed,
}

impl WalError {
    pub fn code(&self) -> Option<&'static str> {
        matches!(self, Self::Sealed).then_some("WAL_SEALED")
    }
}

struct PendingWrite {
    bytes: Vec<u8>,
    completion: oneshot::Sender<Result<(), WalError>>,
}

#[derive(Default)]
struct WalState {
    size: u64,
    next_offset: u64,
    queue: Vec<PendingWrite>,
    flushing: bool,
    sealed: bool,
    closed: bool,
    open: bool,
}

struct WalInner {
    path: PathBuf,
    policy: FsyncPolicy,
    sync_interval: Duration,
    stats: Option<Arc<Mutex<WalStats>>>,
    state: Mutex<WalState>,
    file: AsyncMutex<Option<File>>,
    changed: Notify,
    timer: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct Wal {
    inner: Arc<WalInner>,
}

pub struct WalAppend {
    pub offset: u64,
    completion: oneshot::Receiver<Result<(), WalError>>,
}

impl WalAppend {
    pub async fn done(self) -> Result<(), WalError> {
        self.completion
            .await
            .map_err(|_| WalError::CompletionClosed)?
    }
}

impl Wal {
    pub fn new(path: impl Into<PathBuf>, options: WalOptions) -> Self {
        Self {
            inner: Arc::new(WalInner {
                path: path.into(),
                policy: options.fsync_policy,
                sync_interval: options.sync_interval,
                stats: options.stats,
                state: Mutex::new(WalState::default()),
                file: AsyncMutex::new(None),
                changed: Notify::new(),
                timer: Mutex::new(None),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn size(&self) -> u64 {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .size
    }

    // Original: packages/minidb/src/wal.ts, WAL.open().
    pub async fn open(&self) -> Result<(), WalError> {
        {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.open {
                return Ok(());
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.inner.path)
            .await?;
        let size = file.metadata().await?.len();
        *self.inner.file.lock().await = Some(file);
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.size = size;
            state.next_offset = size;
            state.open = true;
            state.closed = false;
            state.sealed = false;
        }
        if self.inner.policy == FsyncPolicy::EverySecond {
            self.start_sync_timer();
        }
        Ok(())
    }

    fn start_sync_timer(&self) {
        let weak = Arc::downgrade(&self.inner);
        let interval = self.inner.sync_interval;
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = Weak::upgrade(&weak) else {
                    break;
                };
                let closed = inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .closed;
                if closed {
                    break;
                }
                let _ = sync_inner(&inner).await;
            }
        });
        *self
            .inner
            .timer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
    }

    pub fn seal(&self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sealed = true;
    }

    // Original: packages/minidb/src/wal.ts, WAL.appendLoc().
    pub fn append_loc(&self, frame: impl Into<Vec<u8>>) -> Result<WalAppend, WalError> {
        let frame = frame.into();
        let (completion, receiver) = oneshot::channel();
        let mut spawn_flush = false;
        let offset;
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.closed {
                return Err(WalError::Closed);
            }
            if state.sealed {
                return Err(WalError::Sealed);
            }
            if !state.open {
                return Err(WalError::NotOpen);
            }
            offset = state.next_offset;
            state.next_offset = state.next_offset.saturating_add(frame.len() as u64);
            state.queue.push(PendingWrite {
                bytes: frame,
                completion,
            });
            if !state.flushing {
                state.flushing = true;
                spawn_flush = true;
            }
        }
        if spawn_flush {
            let inner = Arc::clone(&self.inner);
            tokio::spawn(async move {
                tokio::task::yield_now().await;
                flush_loop(inner).await;
            });
        }
        Ok(WalAppend {
            offset,
            completion: receiver,
        })
    }

    pub async fn append(&self, frame: impl Into<Vec<u8>>) -> Result<(), WalError> {
        self.append_loc(frame)?.done().await
    }

    pub async fn refresh_size(&self) -> Result<(), WalError> {
        let size = {
            let file = self.inner.file.lock().await;
            file.as_ref()
                .ok_or(WalError::NotOpen)?
                .metadata()
                .await?
                .len()
        };
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.size = size;
        state.next_offset = size;
        Ok(())
    }

    pub async fn sync(&self) -> Result<(), WalError> {
        sync_inner(&self.inner).await
    }

    // Original: packages/minidb/src/wal.ts, WAL.flush().
    pub async fn flush(&self) -> Result<(), WalError> {
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let done = {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.queue.is_empty() && !state.flushing
            };
            if done {
                return Ok(());
            }
            notified.await;
        }
    }

    // Original: packages/minidb/src/wal.ts, WAL.close().
    pub async fn close(&self) -> Result<(), WalError> {
        let was_open = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.open
        };
        if !was_open {
            return Ok(());
        }
        if let Some(timer) = self
            .inner
            .timer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            timer.abort();
        }
        let flush_result = self.flush().await;
        let sync_result = self.sync().await;
        self.inner.file.lock().await.take();
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open = false;
        flush_result.and(sync_result)
    }
}

async fn flush_loop(inner: Arc<WalInner>) {
    loop {
        let batch = {
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.queue.is_empty() {
                state.flushing = false;
                inner.changed.notify_waiters();
                return;
            }
            std::mem::take(&mut state.queue)
        };
        let byte_count = batch
            .iter()
            .map(|pending| pending.bytes.len())
            .sum::<usize>();
        let mut combined = Vec::with_capacity(byte_count);
        for pending in &batch {
            combined.extend_from_slice(&pending.bytes);
        }
        let result = async {
            let mut file = inner.file.lock().await;
            let file = file.as_mut().ok_or(WalError::NotOpen)?;
            file.write_all(&combined).await?;
            if inner.policy == FsyncPolicy::Always {
                file.sync_all().await?;
                increment_fsyncs(&inner);
            }
            Ok::<(), WalError>(())
        }
        .await;
        if result.is_ok() {
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.size = state.size.saturating_add(byte_count as u64);
            if let Some(stats) = &inner.stats {
                stats
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .bytes_written += byte_count as u64;
            }
        }
        let error_message = result.as_ref().err().map(ToString::to_string);
        for pending in batch {
            let completion = match &error_message {
                None => Ok(()),
                Some(message) => Err(WalError::Io(io::Error::other(message.clone()))),
            };
            let _ = pending.completion.send(completion);
        }
        inner.changed.notify_waiters();
    }
}

async fn sync_inner(inner: &WalInner) -> Result<(), WalError> {
    let file = inner.file.lock().await;
    file.as_ref().ok_or(WalError::NotOpen)?.sync_all().await?;
    increment_fsyncs(inner);
    Ok(())
}

fn increment_fsyncs(inner: &WalInner) {
    if let Some(stats) = &inner.stats {
        stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fsyncs += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn appends_concurrently_in_order_and_seals() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.wal");
        let wal = Wal::new(
            &path,
            WalOptions {
                fsync_policy: FsyncPolicy::No,
                ..WalOptions::default()
            },
        );
        wal.open().await.unwrap();
        let writes = (0_u32..1000)
            .map(|number| wal.append(number.to_le_bytes()))
            .collect::<Vec<_>>();
        futures_util::future::try_join_all(writes).await.unwrap();
        wal.seal();
        let error = wal.append(vec![1]).await.unwrap_err();
        assert_eq!(error.code(), Some("WAL_SEALED"));
        wal.close().await.unwrap();

        let bytes = std::fs::read(path).unwrap();
        let numbers = bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(numbers, (0..1000).collect::<Vec<_>>());
    }
}
