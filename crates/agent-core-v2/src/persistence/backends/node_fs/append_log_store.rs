//! Ordered, buffered JSONL append-log store.
//!
//! Original: `packages/agent-core-v2/src/persistence/backends/node-fs/appendLogStore.ts`.

use std::{
    error::Error,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt, future::join_all, stream};
use indexmap::IndexMap;
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::_base::{
    di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        lifecycle::{DisposableHandle, to_disposable},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    errors::errors::{Error2Options, ErrorCause},
};

use crate::persistence::interface::{
    append_log_store::{
        APPEND_LOG_STORE_SERVICE_ID, AppendLogCorruptedError, AppendLogError, AppendLogOptions,
        AppendLogStoreHandle, AppendLogStoreService, AppendLogValueStream,
    },
    storage::{
        FILE_SYSTEM_STORAGE_SERVICE_ID, FileSystemStorageService, STORAGE_DECODE_FAILED,
        STORAGE_IO_FAILED, StorageAppendOptions, StorageError, StorageWriteOptions,
    },
};

#[derive(Clone)]
pub struct AppendLogStore {
    inner: Arc<AppendLogStoreInner>,
}

struct AppendLogStoreInner {
    storage: Arc<dyn FileSystemStorageService>,
    logs: Mutex<IndexMap<String, Arc<LogState>>>,
}

struct LogState {
    values: Mutex<LogValues>,
    operation: AsyncMutex<()>,
    ready: Option<Arc<Settlement>>,
    settled: Arc<Settlement>,
}

#[derive(Default)]
struct LogValues {
    pending: Vec<Value>,
    flush_scheduled: bool,
    storage_failure: Option<AppendLogError>,
    cutover_epoch: u64,
    ref_count: usize,
    retired: bool,
    on_error: Option<crate::persistence::interface::append_log_store::AppendLogErrorHandler>,
}

#[derive(Default)]
struct Settlement {
    done: AtomicBool,
    notify: Notify,
}

impl Settlement {
    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.done.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    fn complete(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}

impl AppendLogStore {
    pub fn new(storage: Arc<dyn FileSystemStorageService>) -> Self {
        Self {
            inner: Arc::new(AppendLogStoreInner {
                storage,
                logs: Mutex::new(IndexMap::new()),
            }),
        }
    }

    fn state(&self, scope: &str, key: &str) -> Arc<LogState> {
        let id = log_id(scope, key);
        let mut logs = self.inner.logs.lock().unwrap();
        if let Some(state) = logs.get(&id)
            && !state.values.lock().unwrap().retired
        {
            return Arc::clone(state);
        }
        let ready = logs.get(&id).map(|state| Arc::clone(&state.settled));
        let state = Arc::new(LogState {
            values: Mutex::new(LogValues::default()),
            operation: AsyncMutex::new(()),
            ready,
            settled: Arc::new(Settlement::default()),
        });
        logs.insert(id, Arc::clone(&state));
        state
    }

    fn schedule_flush(&self, scope: String, key: String, state: Arc<LogState>) {
        let should_schedule = {
            let mut values = state.values.lock().unwrap();
            if values.flush_scheduled {
                false
            } else {
                values.flush_scheduled = true;
                true
            }
        };
        if !should_schedule {
            return;
        }
        let store = self.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    tokio::task::yield_now().await;
                    state.values.lock().unwrap().flush_scheduled = false;
                    if let Err(error) = store.flush_state(&scope, &key, &state).await {
                        let handler = state.values.lock().unwrap().on_error.clone();
                        if let Some(handler) = handler {
                            handler(&error);
                        }
                    }
                });
            }
            Err(error) => {
                state.values.lock().unwrap().flush_scheduled = false;
                let append_error = runtime_error(error);
                if let Some(handler) = state.values.lock().unwrap().on_error.clone() {
                    handler(&append_error);
                }
            }
        }
    }

    async fn wait_ready(state: &LogState) {
        if let Some(ready) = &state.ready {
            ready.wait().await;
        }
    }

    // Original: flushState() + drain(). The operation mutex is the owned
    // flush promise; epoch checks preserve in-flight appends across rewrites.
    async fn flush_state(
        &self,
        scope: &str,
        key: &str,
        state: &Arc<LogState>,
    ) -> Result<(), AppendLogError> {
        let _operation = state.operation.lock().await;
        Self::wait_ready(state).await;
        self.drain_locked(scope, key, state).await
    }

    async fn drain_locked(
        &self,
        scope: &str,
        key: &str,
        state: &Arc<LogState>,
    ) -> Result<(), AppendLogError> {
        loop {
            let (batch, epoch, failure) = {
                let values = state.values.lock().unwrap();
                (
                    values.pending.clone(),
                    values.cutover_epoch,
                    values.storage_failure.clone(),
                )
            };
            if let Some(failure) = failure {
                return Err(failure);
            }
            if batch.is_empty() {
                return Ok(());
            }
            let encoded = encode_batch(&batch)?;
            if let Err(error) = self
                .inner
                .storage
                .append(scope, key, &encoded, StorageAppendOptions { durable: true })
                .await
            {
                let failure = AppendLogError::Storage(error);
                let mut values = state.values.lock().unwrap();
                if values.storage_failure.is_none() {
                    values.storage_failure = Some(failure.clone());
                }
                return Err(values.storage_failure.clone().unwrap());
            }
            let mut values = state.values.lock().unwrap();
            if values.cutover_epoch != epoch {
                return Ok(());
            }
            values.pending.drain(..batch.len());
        }
    }

    async fn release(&self, scope: String, key: String, state: Arc<LogState>) {
        let should_retire = {
            let mut values = state.values.lock().unwrap();
            values.ref_count = values.ref_count.saturating_sub(1);
            if values.ref_count > 0 || values.retired {
                false
            } else {
                values.retired = true;
                true
            }
        };
        if !should_retire {
            return;
        }
        let _ = self.flush_state(&scope, &key, &state).await;
        state.settled.complete();
        let id = log_id(&scope, &key);
        let mut logs = self.inner.logs.lock().unwrap();
        if logs
            .get(&id)
            .is_some_and(|current| Arc::ptr_eq(current, &state))
        {
            logs.shift_remove(&id);
        }
    }
}

/// Registers the App-scoped ordered append-log backend.
///
/// Original: the module-level `registerScopedService(...)` call in
/// `appendLogStore.ts`.
pub fn register_append_log_store() {
    register_scoped_service(
        LifecycleScope::App,
        APPEND_LOG_STORE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let storage = accessor.get(FILE_SYSTEM_STORAGE_SERVICE_ID)?;
            let service: Arc<dyn AppendLogStoreService> =
                Arc::new(AppendLogStore::new(Arc::clone(&storage.0)));
            Ok(AppendLogStoreHandle(service))
        }),
        InstantiationType::Eager,
        "storage",
    );
}

#[async_trait]
impl AppendLogStoreService for AppendLogStore {
    // Original: AppendLogStore.append<R>().
    fn append_value(&self, scope: &str, key: &str, record: Value, options: AppendLogOptions) {
        let state = self.state(scope, key);
        {
            let mut values = state.values.lock().unwrap();
            values.pending.push(record);
            if values.on_error.is_none() {
                values.on_error = options.on_error;
            }
        }
        self.schedule_flush(scope.into(), key.into(), state);
    }

    // Original: AppendLogStore.read<R>() + parseLine<R>().
    fn read_values(&self, scope: &str, key: &str) -> AppendLogValueStream {
        let store = self.clone();
        let scope = scope.to_owned();
        let key = key.to_owned();
        let state = self.state(&scope, &key);
        let opened = stream::once(async move {
            store.flush_state(&scope, &key, &state).await?;
            Ok::<AppendLogValueStream, AppendLogError>(parse_json_lines(
                store.inner.storage.read_stream(&scope, &key, None),
                scope,
                key,
            ))
        });
        Box::pin(opened.try_flatten())
    }

    // Original: AppendLogStore.rewrite<R>(). Epoch ownership starts before
    // waiting for an in-flight append, preserving that append as the live tail.
    async fn rewrite_values(
        &self,
        scope: &str,
        key: &str,
        records: Vec<Value>,
    ) -> Result<(), AppendLogError> {
        let encoded = encode_batch(&records)?;
        let state = self.state(scope, key);
        {
            let mut values = state.values.lock().unwrap();
            values.cutover_epoch = values.cutover_epoch.wrapping_add(1);
        }
        let operation_guard = state.operation.lock().await;
        Self::wait_ready(&state).await;
        match self
            .inner
            .storage
            .write(scope, key, &encoded, StorageWriteOptions { atomic: true })
            .await
        {
            Ok(()) => state.values.lock().unwrap().storage_failure = None,
            Err(error) => {
                let failure = AppendLogError::Storage(error);
                state.values.lock().unwrap().storage_failure = Some(failure.clone());
                return Err(failure);
            }
        }
        let result = self.drain_locked(scope, key, &state).await;
        drop(operation_guard);
        result
    }

    async fn flush(&self) -> Result<(), AppendLogError> {
        let entries = self
            .inner
            .logs
            .lock()
            .unwrap()
            .iter()
            .map(|(id, state)| {
                let (scope, key) = from_log_id(id);
                (scope.to_owned(), key.to_owned(), Arc::clone(state))
            })
            .collect::<Vec<_>>();
        let results = join_all(
            entries
                .iter()
                .map(|(scope, key, state)| self.flush_state(scope, key, state)),
        )
        .await;
        results
            .into_iter()
            .find_map(Result::err)
            .map_or(Ok(()), Err)
    }

    async fn close(&self) -> Result<(), AppendLogError> {
        self.flush().await
    }

    fn acquire(&self, scope: &str, key: &str) -> DisposableHandle {
        let state = self.state(scope, key);
        state.values.lock().unwrap().ref_count += 1;
        let store = self.clone();
        let scope = scope.to_owned();
        let key = key.to_owned();
        to_disposable(move || match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    store.release(scope, key, state).await;
                });
            }
            Err(error) => {
                let append_error = runtime_error(error);
                if let Some(handler) = state.values.lock().unwrap().on_error.clone() {
                    handler(&append_error);
                }
            }
        })
    }
}

fn encode_batch(records: &[Value]) -> Result<Vec<u8>, AppendLogError> {
    let mut encoded = Vec::new();
    for record in records {
        serde_json::to_writer(&mut encoded, record).map_err(codec_error)?;
        encoded.push(b'\n');
    }
    Ok(encoded)
}

fn codec_error(error: impl Error + Send + Sync + 'static) -> AppendLogError {
    StorageError::with_options(
        STORAGE_DECODE_FAILED,
        "append-log record could not be encoded",
        Error2Options {
            cause: Some(ErrorCause::Error(Arc::new(error))),
            ..Error2Options::default()
        },
    )
    .into()
}

fn runtime_error(error: impl Error + Send + Sync + 'static) -> AppendLogError {
    StorageError::with_options(
        STORAGE_IO_FAILED,
        "append-log background flush requires an active Tokio runtime",
        Error2Options {
            cause: Some(ErrorCause::Error(Arc::new(error))),
            ..Error2Options::default()
        },
    )
    .into()
}

fn parse_json_lines(
    input: crate::persistence::interface::storage::StorageByteStream,
    scope: String,
    key: String,
) -> AppendLogValueStream {
    let state = JsonLineState {
        input,
        pending: Vec::new(),
        line_number: 0,
        eof: false,
        scope,
        key,
    };
    Box::pin(stream::try_unfold(state, |mut state| async move {
        loop {
            if let Some(newline) = state.pending.iter().position(|byte| *byte == b'\n') {
                let mut raw = state.pending.drain(..=newline).collect::<Vec<_>>();
                raw.pop();
                state.line_number += 1;
                if let Some(record) =
                    parse_line(&raw, &state.scope, &state.key, state.line_number, false)?
                {
                    return Ok(Some((record, state)));
                }
                continue;
            }
            if state.eof {
                if state.pending.is_empty() {
                    return Ok(None);
                }
                state.line_number += 1;
                let raw = std::mem::take(&mut state.pending);
                return parse_line(&raw, &state.scope, &state.key, state.line_number, true)
                    .map(|record| record.map(|record| (record, state)));
            }
            match state.input.next().await {
                Some(Ok(chunk)) => state.pending.extend_from_slice(&chunk),
                Some(Err(error)) => return Err(AppendLogError::Storage(error)),
                None => state.eof = true,
            }
        }
    }))
}

struct JsonLineState {
    input: crate::persistence::interface::storage::StorageByteStream,
    pending: Vec<u8>,
    line_number: u64,
    eof: bool,
    scope: String,
    key: String,
}

fn parse_line(
    raw: &[u8],
    scope: &str,
    key: &str,
    line_number: u64,
    allow_truncated: bool,
) -> Result<Option<Value>, AppendLogError> {
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    if raw.is_empty() {
        return Ok(None);
    }
    match serde_json::from_str(&String::from_utf8_lossy(raw)) {
        Ok(record) => Ok(Some(record)),
        Err(_) if allow_truncated => Ok(None),
        Err(error) => {
            let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
            Err(AppendLogCorruptedError::new(scope, key, line_number, cause).into())
        }
    }
}

fn log_id(scope: &str, key: &str) -> String {
    format!("{scope}\n{key}")
}

fn from_log_id(id: &str) -> (&str, &str) {
    id.split_once('\n').unwrap_or((id, ""))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::persistence::{
        backends::memory::in_memory_storage_service::InMemoryStorageService,
        interface::{
            append_log_store::AppendLogStoreHandle,
            storage::{
                STORAGE_CORRUPTED, STORAGE_IO_FAILED, StorageAppendOptions, StorageByteStream,
                StorageReadRange,
            },
        },
    };

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Record {
        n: u8,
    }

    struct FailingAppendStorage {
        inner: InMemoryStorageService,
        failures_remaining: AtomicUsize,
        append_calls: AtomicUsize,
    }

    #[async_trait]
    impl FileSystemStorageService for FailingAppendStorage {
        async fn read(&self, scope: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
            self.inner.read(scope, key).await
        }

        fn read_stream(
            &self,
            scope: &str,
            key: &str,
            range: Option<StorageReadRange>,
        ) -> StorageByteStream {
            self.inner.read_stream(scope, key, range)
        }

        async fn write(
            &self,
            scope: &str,
            key: &str,
            data: &[u8],
            options: StorageWriteOptions,
        ) -> Result<(), StorageError> {
            self.inner.write(scope, key, data, options).await
        }

        async fn append(
            &self,
            scope: &str,
            key: &str,
            data: &[u8],
            options: StorageAppendOptions,
        ) -> Result<(), StorageError> {
            self.append_calls.fetch_add(1, Ordering::Relaxed);
            if self
                .failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(StorageError::new(STORAGE_IO_FAILED, "ambiguous append"));
            }
            self.inner.append(scope, key, data, options).await
        }

        async fn list(
            &self,
            scope: &str,
            prefix: Option<&str>,
        ) -> Result<Vec<String>, StorageError> {
            self.inner.list(scope, prefix).await
        }

        async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
            self.inner.delete(scope, key).await
        }

        fn watch(&self, scope: &str, key: &str) -> Option<crate::_base::event::Event<()>> {
            self.inner.watch(scope, key)
        }

        async fn flush(&self) -> Result<(), StorageError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn append_flush_and_read_preserve_record_order() {
        let storage = Arc::new(InMemoryStorageService::default());
        let log = AppendLogStoreHandle(Arc::new(AppendLogStore::new(storage.clone())));
        log.append("s", "log", &Record { n: 1 }, Default::default())
            .unwrap();
        log.append("s", "log", &Record { n: 2 }, Default::default())
            .unwrap();
        log.flush().await.unwrap();
        assert_eq!(
            storage.read("s", "log").await.unwrap().unwrap(),
            b"{\"n\":1}\n{\"n\":2}\n"
        );
        let records = log
            .read::<Record>("s", "log")
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(records, [Record { n: 1 }, Record { n: 2 }]);
    }

    #[tokio::test]
    async fn rewrite_replaces_history_and_drains_live_tail() {
        let storage = Arc::new(InMemoryStorageService::default());
        let log = AppendLogStoreHandle(Arc::new(AppendLogStore::new(storage)));
        log.append("s", "log", &Record { n: 1 }, Default::default())
            .unwrap();
        log.rewrite("s", "log", &[Record { n: 9 }]).await.unwrap();
        let records = log
            .read::<Record>("s", "log")
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(records, [Record { n: 9 }, Record { n: 1 }]);
    }

    #[tokio::test]
    async fn drops_torn_final_line_but_reports_corrupted_middle_line() {
        let storage = Arc::new(InMemoryStorageService::default());
        storage
            .append(
                "s",
                "torn",
                b"{\"n\":1}\n{\"n\"",
                StorageAppendOptions::default(),
            )
            .await
            .unwrap();
        storage
            .append(
                "s",
                "bad",
                b"{\"n\":1}\nGARBAGE\n{\"n\":3}\n",
                StorageAppendOptions::default(),
            )
            .await
            .unwrap();
        let log = AppendLogStoreHandle(Arc::new(AppendLogStore::new(storage)));
        assert_eq!(
            log.read::<Record>("s", "torn")
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            [Record { n: 1 }]
        );
        let error = log
            .read::<Record>("s", "bad")
            .try_collect::<Vec<_>>()
            .await
            .unwrap_err();
        let AppendLogError::Corrupted(error) = error else {
            panic!("expected corrupted append-log error");
        };
        assert_eq!(error.code(), STORAGE_CORRUPTED);
        assert_eq!(error.details().unwrap()["lineNumber"], 2);
    }

    #[tokio::test]
    async fn final_acquire_release_flushes_and_allows_a_fresh_generation() {
        let storage = Arc::new(InMemoryStorageService::default());
        let backend = AppendLogStore::new(storage);
        let log = AppendLogStoreHandle(Arc::new(backend.clone()));
        let acquired = log.acquire("s", "log");
        log.append("s", "log", &Record { n: 1 }, Default::default())
            .unwrap();
        acquired.dispose().unwrap();
        tokio::task::yield_now().await;
        let replacement = log.acquire("s", "log");
        log.append("s", "log", &Record { n: 2 }, Default::default())
            .unwrap();
        log.flush().await.unwrap();
        let records = log
            .read::<Record>("s", "log")
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(records, [Record { n: 1 }, Record { n: 2 }]);
        replacement.dispose().unwrap();
    }

    #[tokio::test]
    async fn ambiguous_append_failure_is_sticky_until_successful_rewrite() {
        let storage = Arc::new(FailingAppendStorage {
            inner: InMemoryStorageService::default(),
            failures_remaining: AtomicUsize::new(1),
            append_calls: AtomicUsize::new(0),
        });
        let log = AppendLogStoreHandle(Arc::new(AppendLogStore::new(storage.clone())));
        log.append("s", "log", &Record { n: 1 }, Default::default())
            .unwrap();

        assert!(log.flush().await.is_err());
        assert!(log.flush().await.is_err());
        assert_eq!(storage.append_calls.load(Ordering::Relaxed), 1);

        log.rewrite("s", "log", &[Record { n: 9 }]).await.unwrap();
        assert_eq!(storage.append_calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            log.read::<Record>("s", "log")
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            [Record { n: 9 }, Record { n: 1 }]
        );
    }
}
