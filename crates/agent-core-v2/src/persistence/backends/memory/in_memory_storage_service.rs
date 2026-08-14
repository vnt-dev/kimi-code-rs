//! In-memory `FileSystemStorageService` backend.
//!
//! Original: `packages/agent-core-v2/src/persistence/backends/memory/inMemoryStorageService.ts`.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures_util::stream;
use indexmap::IndexMap;

use crate::_base::{
    di::lifecycle::{combined_disposable, to_disposable},
    event::{Emitter, Event},
};

use crate::persistence::interface::storage::{
    FileSystemStorageService, StorageAppendOptions, StorageByteStream, StorageError,
    StorageReadRange, StorageWriteOptions,
};

#[derive(Clone, Default)]
pub struct InMemoryStorageService {
    inner: Arc<InMemoryStorageInner>,
}

#[derive(Default)]
struct InMemoryStorageInner {
    scopes: Mutex<HashMap<String, IndexMap<String, Vec<u8>>>>,
    watchers: Mutex<HashMap<String, Arc<WatchEntry>>>,
}

struct WatchEntry {
    emitter: Emitter<()>,
    count: AtomicUsize,
}

#[async_trait]
impl FileSystemStorageService for InMemoryStorageService {
    async fn read(&self, scope: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self
            .inner
            .scopes
            .lock()
            .get(scope)
            .and_then(|bucket| bucket.get(key))
            .cloned())
    }

    fn read_stream(
        &self,
        scope: &str,
        key: &str,
        range: Option<StorageReadRange>,
    ) -> StorageByteStream {
        let data = self
            .inner
            .scopes
            .lock()
            .get(scope)
            .and_then(|bucket| bucket.get(key))
            .cloned()
            .and_then(|data| read_range(data, range));
        Box::pin(stream::iter(data.into_iter().map(Ok)))
    }

    async fn write(
        &self,
        scope: &str,
        key: &str,
        data: &[u8],
        _options: StorageWriteOptions,
    ) -> Result<(), StorageError> {
        self.inner
            .scopes
            .lock()
            .entry(scope.into())
            .or_default()
            .insert(key.into(), data.into());
        self.notify_watchers(scope, key);
        Ok(())
    }

    async fn append(
        &self,
        scope: &str,
        key: &str,
        data: &[u8],
        _options: StorageAppendOptions,
    ) -> Result<(), StorageError> {
        self.inner
            .scopes
            .lock()
            .entry(scope.into())
            .or_default()
            .entry(key.into())
            .or_default()
            .extend_from_slice(data);
        self.notify_watchers(scope, key);
        Ok(())
    }

    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError> {
        Ok(self
            .inner
            .scopes
            .lock()
            .get(scope)
            .into_iter()
            .flat_map(IndexMap::keys)
            .filter(|key| prefix.is_none_or(|prefix| key.starts_with(prefix)))
            .cloned()
            .collect())
    }

    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
        if let Some(bucket) = self.inner.scopes.lock().get_mut(scope) {
            bucket.shift_remove(key);
        }
        self.notify_watchers(scope, key);
        Ok(())
    }

    fn watch(&self, scope: &str, key: &str) -> Option<Event<()>> {
        let id = watch_key(scope, key);
        let inner = Arc::clone(&self.inner);
        Some(Event::from_subscribe(move |listener| {
            let entry = {
                let mut watchers = inner.watchers.lock();
                Arc::clone(watchers.entry(id.clone()).or_insert_with(|| {
                    Arc::new(WatchEntry {
                        emitter: Emitter::new(),
                        count: AtomicUsize::new(0),
                    })
                }))
            };
            entry.count.fetch_add(1, Ordering::Relaxed);
            let subscription = entry
                .emitter
                .event()
                .subscribe(move |value| listener(value));
            let inner = Arc::clone(&inner);
            let id = id.clone();
            let entry_for_teardown = Arc::clone(&entry);
            let teardown = to_disposable(move || {
                if entry_for_teardown.count.fetch_sub(1, Ordering::AcqRel) == 1 {
                    let mut watchers = inner.watchers.lock();
                    if watchers
                        .get(&id)
                        .is_some_and(|current| Arc::ptr_eq(current, &entry_for_teardown))
                    {
                        watchers.remove(&id);
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

impl InMemoryStorageService {
    fn notify_watchers(&self, scope: &str, key: &str) {
        let entry = self
            .inner
            .watchers
            .lock()
            .get(&watch_key(scope, key))
            .cloned();
        if let Some(entry) = entry {
            entry.emitter.fire(&());
        }
    }

    #[cfg(test)]
    fn watcher_count(&self) -> usize {
        self.inner.watchers.lock().len()
    }
}

fn watch_key(scope: &str, key: &str) -> String {
    format!("{scope}\0{key}")
}

fn read_range(data: Vec<u8>, range: Option<StorageReadRange>) -> Option<Vec<u8>> {
    let Some(range) = range else {
        return Some(data);
    };
    let start = usize::try_from(range.start)
        .unwrap_or(usize::MAX)
        .min(data.len());
    let inclusive_end = range.end.saturating_add(1);
    let end = usize::try_from(inclusive_end)
        .unwrap_or(usize::MAX)
        .min(data.len());
    (start < end).then(|| data[start..end].to_vec())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::StreamExt;

    use super::*;

    #[tokio::test]
    async fn reads_writes_appends_lists_and_deletes_with_scope_isolation() {
        let storage = InMemoryStorageService::default();
        assert_eq!(storage.read("s", "missing").await.unwrap(), None);

        storage
            .write("a", "alpha", b"first", Default::default())
            .await
            .unwrap();
        storage
            .write("a", "alpha", b"second", Default::default())
            .await
            .unwrap();
        storage
            .append("a", "alpha", b"+tail", Default::default())
            .await
            .unwrap();
        storage
            .write("a", "beta", b"B", Default::default())
            .await
            .unwrap();
        storage
            .write("b", "alpha", b"other", Default::default())
            .await
            .unwrap();

        assert_eq!(
            storage.read("a", "alpha").await.unwrap().unwrap(),
            b"second+tail"
        );
        assert_eq!(storage.list("a", None).await.unwrap(), ["alpha", "beta"]);
        assert_eq!(storage.list("a", Some("alp")).await.unwrap(), ["alpha"]);
        assert_eq!(storage.read("b", "alpha").await.unwrap().unwrap(), b"other");

        storage.delete("a", "alpha").await.unwrap();
        storage.delete("a", "alpha").await.unwrap();
        assert_eq!(storage.read("a", "alpha").await.unwrap(), None);
    }

    #[tokio::test]
    async fn ranged_stream_uses_inclusive_end_and_empty_ranges_yield_nothing() {
        let storage = InMemoryStorageService::default();
        storage
            .write("s", "k", b"abcdef", Default::default())
            .await
            .unwrap();
        let chunks = storage
            .read_stream("s", "k", Some(StorageReadRange { start: 1, end: 3 }))
            .collect::<Vec<_>>()
            .await;
        assert_eq!(chunks[0].as_ref().unwrap(), b"bcd");
        assert!(
            storage
                .read_stream("s", "k", Some(StorageReadRange { start: 9, end: 10 }))
                .collect::<Vec<_>>()
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn watch_is_keyed_fires_after_mutation_and_retires_last_listener() {
        let storage = InMemoryStorageService::default();
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_listener = Arc::clone(&fired);
        let subscription = storage.watch("s", "k").unwrap().subscribe(move |_| {
            fired_for_listener.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(storage.watcher_count(), 1);

        storage
            .write("s", "other", b"x", Default::default())
            .await
            .unwrap();
        storage
            .append("s", "k", b"x", Default::default())
            .await
            .unwrap();
        storage.delete("s", "k").await.unwrap();
        assert_eq!(fired.load(Ordering::Relaxed), 2);

        subscription.dispose().unwrap();
        assert_eq!(storage.watcher_count(), 0);
    }
}
