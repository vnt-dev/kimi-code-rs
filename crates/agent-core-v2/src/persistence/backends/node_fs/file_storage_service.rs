//! Local-filesystem byte storage backend.
//!
//! Original: `packages/agent-core-v2/src/persistence/backends/node-fs/fileStorageService.ts`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt, stream};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncSeekExt, AsyncWriteExt},
    task::JoinHandle,
    time::{Duration, Instant, MissedTickBehavior},
};
use tokio_util::{io::ReaderStream, sync::CancellationToken};

use crate::_base::{
    di::lifecycle::{combined_disposable, to_disposable},
    errors::unexpected_error::on_unexpected_error,
    event::{Emitter, Event},
    utils::fs::{atomic_write, sync_dir},
};

use crate::persistence::interface::storage::{
    FileSystemStorageService, StorageAppendOptions, StorageByteStream, StorageError,
    StorageReadRange, StorageWriteOptions, to_storage_io_error,
};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct FileStorageService {
    base_dir: PathBuf,
    dir_mode: Option<u32>,
    file_mode: Option<u32>,
    synced_dirs: Mutex<HashSet<PathBuf>>,
}

impl FileStorageService {
    pub fn new(
        base_dir: impl Into<PathBuf>,
        dir_mode: Option<u32>,
        file_mode: Option<u32>,
    ) -> Self {
        Self {
            base_dir: base_dir.into(),
            dir_mode,
            file_mode,
            synced_dirs: Mutex::new(HashSet::new()),
        }
    }

    pub fn with_default_modes(base_dir: impl Into<PathBuf>) -> Self {
        Self::new(base_dir, None, None)
    }

    fn path(&self, scope: &str, key: &str) -> PathBuf {
        self.base_dir.join(scope).join(key)
    }

    fn scope_path(&self, scope: &str) -> PathBuf {
        self.base_dir.join(scope)
    }

    async fn create_parent(&self, directory: &Path) -> std::io::Result<()> {
        fs::create_dir_all(directory).await?;
        set_directory_mode(directory, self.dir_mode).await
    }

    // Original: FileStorageService.syncDirOnce(). A removed directory is a no-op.
    async fn sync_dir_once(&self, directory: &Path) -> std::io::Result<()> {
        if self.synced_dirs.lock().unwrap().contains(directory) {
            return Ok(());
        }
        match sync_dir(directory).await {
            Ok(()) => {
                self.synced_dirs.lock().unwrap().insert(directory.into());
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[async_trait]
impl FileSystemStorageService for FileStorageService {
    async fn read(&self, scope: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let path = self.path(scope, key);
        match fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(storage_io(error, &path, "read")),
        }
    }

    fn read_stream(
        &self,
        scope: &str,
        key: &str,
        range: Option<StorageReadRange>,
    ) -> StorageByteStream {
        let path = self.path(scope, key);
        let opened = stream::once(async move {
            let mut file = match File::open(&path).await {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok::<StorageByteStream, StorageError>(Box::pin(stream::empty()));
                }
                Err(error) => return Err(storage_io(error, &path, "read")),
            };
            let stream: StorageByteStream = if let Some(range) = range {
                if let Err(error) = file.seek(std::io::SeekFrom::Start(range.start)).await {
                    return Err(storage_io(error, &path, "read"));
                }
                let length = range.end.saturating_sub(range.start).saturating_add(1);
                let path = path.clone();
                Box::pin(
                    ReaderStream::new(tokio::io::AsyncReadExt::take(file, length)).map(
                        move |chunk| {
                            chunk
                                .map(|bytes| bytes.to_vec())
                                .map_err(|error| storage_io(error, &path, "read"))
                        },
                    ),
                )
            } else {
                let path = path.clone();
                Box::pin(ReaderStream::new(file).map(move |chunk| {
                    chunk
                        .map(|bytes| bytes.to_vec())
                        .map_err(|error| storage_io(error, &path, "read"))
                }))
            };
            Ok(stream)
        });
        Box::pin(opened.try_flatten())
    }

    async fn write(
        &self,
        scope: &str,
        key: &str,
        data: &[u8],
        _options: StorageWriteOptions,
    ) -> Result<(), StorageError> {
        let path = self.path(scope, key);
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let operation = async {
            self.create_parent(directory).await?;
            atomic_write(&path, data, self.file_mode).await?;
            self.sync_dir_once(directory).await
        };
        operation
            .await
            .map_err(|error| storage_io(error, &path, "write"))
    }

    async fn append(
        &self,
        scope: &str,
        key: &str,
        data: &[u8],
        options: StorageAppendOptions,
    ) -> Result<(), StorageError> {
        let path = self.path(scope, key);
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let operation = async {
            self.create_parent(directory).await?;
            let mut open = OpenOptions::new();
            open.create(true).append(true);
            #[cfg(unix)]
            if let Some(mode) = self.file_mode {
                open.mode(mode);
            }
            let mut file = open.open(&path).await?;
            if !data.is_empty() {
                file.write_all(data).await?;
            }
            if options.durable {
                file.sync_all().await?;
            }
            drop(file);
            self.sync_dir_once(directory).await
        };
        operation
            .await
            .map_err(|error| storage_io(error, &path, "append"))
    }

    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError> {
        let path = self.scope_path(scope);
        let mut directory = match fs::read_dir(&path).await {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(storage_io(error, &path, "list")),
        };
        let mut entries = Vec::new();
        loop {
            match directory.next_entry().await {
                Ok(Some(entry)) => {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if prefix.is_none_or(|prefix| name.starts_with(prefix)) {
                        entries.push(name);
                    }
                }
                Ok(None) => return Ok(entries),
                Err(error) => return Err(storage_io(error, &path, "list")),
            }
        }
    }

    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
        let path = self.path(scope, key);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(storage_io(error, &path, "delete")),
        }
    }

    // Rust adaptation: chokidar is replaced by a runtime-owned metadata poller.
    // It observes external changes and atomic renames, retains exact-key filtering,
    // and applies the original 150ms debounce without a detached task lifecycle.
    fn watch(&self, scope: &str, key: &str) -> Option<Event<()>> {
        let target = self.path(scope, key);
        let directory = target.parent().unwrap_or_else(|| Path::new(".")).to_owned();
        let dir_mode = self.dir_mode;
        let shared = Arc::new(WatchShared {
            emitter: Emitter::new(),
            state: Mutex::new(WatchRuntime::default()),
        });
        Some(Event::from_subscribe(move |listener| {
            let subscription = shared
                .emitter
                .event()
                .subscribe(move |value| listener(value));
            let mut runtime = shared.state.lock().unwrap();
            runtime.ref_count += 1;
            if runtime.ref_count == 1 {
                if let Err(error) = create_watch_directory(&directory, dir_mode) {
                    on_unexpected_error(&error);
                } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let cancellation = CancellationToken::new();
                    let task = handle.spawn(watch_file(
                        target.clone(),
                        Arc::clone(&shared),
                        cancellation.clone(),
                    ));
                    runtime.cancellation = Some(cancellation);
                    runtime.task = Some(task);
                } else {
                    on_unexpected_error(&std::io::Error::other(
                        "file watch requires an active Tokio runtime",
                    ));
                }
            }
            drop(runtime);
            let shared_for_teardown = Arc::clone(&shared);
            let teardown = to_disposable(move || {
                let mut runtime = shared_for_teardown.state.lock().unwrap();
                runtime.ref_count = runtime.ref_count.saturating_sub(1);
                if runtime.ref_count == 0 {
                    if let Some(cancellation) = runtime.cancellation.take() {
                        cancellation.cancel();
                    }
                    if let Some(task) = runtime.task.take() {
                        task.abort();
                    }
                }
            });
            combined_disposable(vec![subscription, teardown])
        }))
    }

    async fn flush(&self) -> Result<(), StorageError> {
        Ok(())
    }

    async fn close(&self) -> Result<(), StorageError> {
        Ok(())
    }
}

fn storage_io(error: std::io::Error, path: &Path, operation: &str) -> StorageError {
    to_storage_io_error(Box::new(error), &path.to_string_lossy(), operation)
}

async fn set_directory_mode(path: &Path, mode: Option<u32>) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn create_watch_directory(path: &Path, mode: Option<u32>) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

#[derive(Default)]
struct WatchRuntime {
    ref_count: usize,
    cancellation: Option<CancellationToken>,
    task: Option<JoinHandle<()>>,
}

struct WatchShared {
    emitter: Emitter<()>,
    state: Mutex<WatchRuntime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileStamp {
    exists: bool,
    length: u64,
    modified: Option<SystemTime>,
}

async fn file_stamp(path: &Path) -> std::io::Result<FileStamp> {
    match fs::metadata(path).await {
        Ok(metadata) => Ok(FileStamp {
            exists: true,
            length: metadata.len(),
            modified: metadata.modified().ok(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileStamp {
            exists: false,
            length: 0,
            modified: None,
        }),
        Err(error) => Err(error),
    }
}

async fn watch_file(target: PathBuf, shared: Arc<WatchShared>, cancellation: CancellationToken) {
    let mut previous = match file_stamp(&target).await {
        Ok(stamp) => stamp,
        Err(error) => {
            on_unexpected_error(&error);
            return;
        }
    };
    let mut pending_since = None;
    let mut interval = tokio::time::interval(WATCH_POLL_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancellation.cancelled() => return,
            _ = interval.tick() => {
                match file_stamp(&target).await {
                    Ok(current) if current != previous => {
                        previous = current;
                        pending_since = Some(Instant::now());
                    }
                    Ok(_) => {
                        if pending_since.is_some_and(|since| since.elapsed() >= WATCH_DEBOUNCE) {
                            pending_since = None;
                            shared.emitter.fire(&());
                        }
                    }
                    Err(error) => on_unexpected_error(&error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_util::StreamExt;
    use uuid::Uuid;

    use super::*;

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!("kimi-file-storage-{}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn round_trips_replaces_appends_ranges_lists_and_missing_values() {
        let directory = temporary_directory();
        let storage = FileStorageService::with_default_modes(&directory);
        assert_eq!(storage.read("s", "missing").await.unwrap(), None);
        assert!(storage.list("missing", None).await.unwrap().is_empty());

        storage
            .write("s", "k", b"first", Default::default())
            .await
            .unwrap();
        storage
            .write("s", "k", b"second", Default::default())
            .await
            .unwrap();
        storage
            .append("s", "k", b"+tail", Default::default())
            .await
            .unwrap();
        storage
            .write("s", "other", b"x", Default::default())
            .await
            .unwrap();
        assert_eq!(
            storage.read("s", "k").await.unwrap().unwrap(),
            b"second+tail"
        );
        let chunks = storage
            .read_stream("s", "k", Some(StorageReadRange { start: 1, end: 3 }))
            .collect::<Vec<_>>()
            .await;
        assert_eq!(chunks[0].as_ref().unwrap(), b"eco");
        let mut keys = storage.list("s", None).await.unwrap();
        keys.sort();
        assert_eq!(keys, ["k", "other"]);

        storage.delete("s", "k").await.unwrap();
        storage.delete("s", "k").await.unwrap();
        assert_eq!(storage.read("s", "k").await.unwrap(), None);
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn applies_directory_and_file_modes_and_translates_io_errors() {
        use std::os::unix::fs::PermissionsExt;

        let directory = temporary_directory();
        fs::create_dir_all(&directory).await.unwrap();
        let storage = FileStorageService::new(&directory, Some(0o700), Some(0o600));
        storage
            .write("cron/ws", "state.json", b"{}", Default::default())
            .await
            .unwrap();
        assert_eq!(
            fs::metadata(directory.join("cron/ws"))
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(directory.join("cron/ws/state.json"))
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        fs::create_dir_all(directory.join("scope/a-directory"))
            .await
            .unwrap();
        let error = storage.read("scope", "a-directory").await.unwrap_err();
        assert_eq!(
            error.code(),
            crate::persistence::interface::storage::STORAGE_IO_FAILED
        );
        assert_eq!(error.error().details.as_ref().unwrap()["errno"], "EISDIR");
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn watch_observes_exact_external_file_after_debounce() {
        let directory = temporary_directory();
        let storage = FileStorageService::with_default_modes(&directory);
        let fired = Arc::new(AtomicBool::new(false));
        let fired_for_listener = Arc::clone(&fired);
        let subscription = storage.watch("s", "k").unwrap().subscribe(move |_| {
            fired_for_listener.store(true, Ordering::Relaxed);
        });
        fs::write(directory.join("s/other"), b"x").await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(!fired.load(Ordering::Relaxed));
        fs::write(directory.join("s/k"), b"value").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !fired.load(Ordering::Relaxed) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        subscription.dispose().unwrap();
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn removed_directory_sync_is_a_no_op() {
        let directory = temporary_directory();
        let storage = FileStorageService::with_default_modes(&directory);
        storage
            .sync_dir_once(&directory.join("missing"))
            .await
            .unwrap();
    }
}
