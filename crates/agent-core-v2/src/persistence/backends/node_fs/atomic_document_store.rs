//! JSON and TOML atomic-document stores over byte storage.
//!
//! Original: `packages/agent-core-v2/src/persistence/backends/node-fs/atomicDocumentStore.ts`.

use std::{error::Error, sync::Arc};

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::_base::{
    di::lifecycle::{DisposableHandle, disposable_none},
    errors::errors::{Error2Options, ErrorCause},
    event::Event,
};

use crate::persistence::interface::{
    atomic_document_store::{AtomicDocumentStoreService, DocumentCodec},
    storage::{FileSystemStorageService, STORAGE_DECODE_FAILED, StorageError, StorageWriteOptions},
};

#[derive(Debug, Default)]
pub struct JsonDocumentCodec;

impl DocumentCodec for JsonDocumentCodec {
    fn format(&self) -> &str {
        "json"
    }

    fn encode(&self, value: &Value) -> Result<Vec<u8>, StorageError> {
        serde_json::to_vec(value).map_err(codec_storage_error)
    }

    fn decode(&self, bytes: &[u8]) -> Result<Value, StorageError> {
        serde_json::from_str(&String::from_utf8_lossy(bytes)).map_err(codec_storage_error)
    }
}

#[derive(Debug, Default)]
pub struct TomlDocumentCodec;

impl DocumentCodec for TomlDocumentCodec {
    fn format(&self) -> &str {
        "toml"
    }

    fn encode(&self, value: &Value) -> Result<Vec<u8>, StorageError> {
        let mut text = toml::to_string(value).map_err(codec_storage_error)?;
        text.push('\n');
        Ok(text.into_bytes())
    }

    fn decode(&self, bytes: &[u8]) -> Result<Value, StorageError> {
        let text = String::from_utf8_lossy(bytes);
        if text.trim().is_empty() {
            return Ok(Value::Object(Map::new()));
        }
        toml::from_str(&text).map_err(codec_storage_error)
    }
}

struct AtomicDocumentStore {
    storage: Arc<dyn FileSystemStorageService>,
    codec: Arc<dyn DocumentCodec>,
}

impl AtomicDocumentStore {
    fn new(storage: Arc<dyn FileSystemStorageService>, codec: Arc<dyn DocumentCodec>) -> Self {
        Self { storage, codec }
    }
}

#[async_trait]
impl AtomicDocumentStoreService for AtomicDocumentStore {
    // Original: AtomicDocumentStoreBase.get<T>().
    async fn get_value(&self, scope: &str, key: &str) -> Result<Option<Value>, StorageError> {
        let Some(bytes) = self.storage.read(scope, key).await? else {
            return Ok(None);
        };
        self.codec.decode(&bytes).map(Some).map_err(|error| {
            let format = self.codec.format();
            let details = Map::from_iter([
                ("scope".into(), Value::String(scope.into())),
                ("key".into(), Value::String(key.into())),
                ("format".into(), Value::String(format.into())),
            ]);
            StorageError::with_options(
                STORAGE_DECODE_FAILED,
                format!("failed to decode {scope}/{key} as {format}"),
                Error2Options {
                    details: Some(details),
                    cause: Some(ErrorCause::Error(Arc::new(error))),
                    ..Error2Options::default()
                },
            )
        })
    }

    // Original: AtomicDocumentStoreBase.set<T>().
    async fn set_value(&self, scope: &str, key: &str, value: Value) -> Result<(), StorageError> {
        let bytes = self.codec.encode(&value)?;
        self.storage
            .write(scope, key, &bytes, StorageWriteOptions { atomic: true })
            .await
    }

    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
        self.storage.delete(scope, key).await
    }

    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError> {
        self.storage.list(scope, prefix).await
    }

    fn watch(&self, scope: &str, key: &str) -> Event<()> {
        self.storage.watch(scope, key).unwrap_or_else(Event::none)
    }

    fn acquire(&self, _scope: &str, _key: &str) -> DisposableHandle {
        disposable_none()
    }
}

fn codec_storage_error(error: impl Error + Send + Sync + 'static) -> StorageError {
    StorageError::with_options(
        STORAGE_DECODE_FAILED,
        "document codec failed",
        Error2Options {
            cause: Some(ErrorCause::Error(Arc::new(error))),
            ..Error2Options::default()
        },
    )
}

#[derive(Clone)]
pub struct JsonAtomicDocumentStore(Arc<AtomicDocumentStore>);

impl JsonAtomicDocumentStore {
    pub fn new(storage: Arc<dyn FileSystemStorageService>) -> Self {
        Self(Arc::new(AtomicDocumentStore::new(
            storage,
            Arc::new(JsonDocumentCodec),
        )))
    }
}

#[async_trait]
impl AtomicDocumentStoreService for JsonAtomicDocumentStore {
    async fn get_value(&self, scope: &str, key: &str) -> Result<Option<Value>, StorageError> {
        self.0.get_value(scope, key).await
    }

    async fn set_value(&self, scope: &str, key: &str, value: Value) -> Result<(), StorageError> {
        self.0.set_value(scope, key, value).await
    }

    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
        self.0.delete(scope, key).await
    }

    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError> {
        self.0.list(scope, prefix).await
    }

    fn watch(&self, scope: &str, key: &str) -> Event<()> {
        self.0.watch(scope, key)
    }

    fn acquire(&self, scope: &str, key: &str) -> DisposableHandle {
        self.0.acquire(scope, key)
    }
}

#[derive(Clone)]
pub struct TomlAtomicDocumentStore(Arc<AtomicDocumentStore>);

impl TomlAtomicDocumentStore {
    pub fn new(storage: Arc<dyn FileSystemStorageService>) -> Self {
        Self(Arc::new(AtomicDocumentStore::new(
            storage,
            Arc::new(TomlDocumentCodec),
        )))
    }
}

#[async_trait]
impl AtomicDocumentStoreService for TomlAtomicDocumentStore {
    async fn get_value(&self, scope: &str, key: &str) -> Result<Option<Value>, StorageError> {
        self.0.get_value(scope, key).await
    }

    async fn set_value(&self, scope: &str, key: &str, value: Value) -> Result<(), StorageError> {
        self.0.set_value(scope, key, value).await
    }

    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
        self.0.delete(scope, key).await
    }

    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError> {
        self.0.list(scope, prefix).await
    }

    fn watch(&self, scope: &str, key: &str) -> Event<()> {
        self.0.watch(scope, key)
    }

    fn acquire(&self, scope: &str, key: &str) -> DisposableHandle {
        self.0.acquire(scope, key)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::persistence::{
        backends::memory::in_memory_storage_service::InMemoryStorageService,
        interface::{
            atomic_document_store::AtomicDocumentStoreHandle,
            storage::{StorageAppendOptions, StorageWriteOptions},
        },
    };

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct State {
        title: String,
        count: u8,
    }

    #[tokio::test]
    async fn json_store_round_trips_replaces_watches_lists_and_deletes() {
        let storage = Arc::new(InMemoryStorageService::default());
        let store =
            AtomicDocumentStoreHandle(Arc::new(JsonAtomicDocumentStore::new(storage.clone())));
        assert_eq!(
            store.get::<State>("session", "state.json").await.unwrap(),
            None
        );

        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired_for_listener = Arc::clone(&fired);
        let subscription = store.watch("session", "state.json").subscribe(move |_| {
            fired_for_listener.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        store
            .set(
                "session",
                "state.json",
                &State {
                    title: "hello".into(),
                    count: 1,
                },
            )
            .await
            .unwrap();
        assert!(fired.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(
            store.get::<State>("session", "state.json").await.unwrap(),
            Some(State {
                title: "hello".into(),
                count: 1
            })
        );
        assert_eq!(
            store.list("session", Some("state")).await.unwrap(),
            ["state.json"]
        );
        store.delete("session", "state.json").await.unwrap();
        assert_eq!(
            store.get::<State>("session", "state.json").await.unwrap(),
            None
        );
        subscription.dispose().unwrap();
    }

    #[tokio::test]
    async fn invalid_json_is_wrapped_with_address_and_format() {
        let storage = Arc::new(InMemoryStorageService::default());
        storage
            .append(
                "session",
                "bad.json",
                b"{ not json",
                StorageAppendOptions::default(),
            )
            .await
            .unwrap();
        let store = JsonAtomicDocumentStore::new(storage);
        let error = store.get_value("session", "bad.json").await.unwrap_err();
        assert_eq!(error.code(), STORAGE_DECODE_FAILED);
        assert_eq!(error.error().details.as_ref().unwrap()["scope"], "session");
        assert_eq!(error.error().details.as_ref().unwrap()["key"], "bad.json");
        assert_eq!(error.error().details.as_ref().unwrap()["format"], "json");
        assert!(error.source().is_some());
    }

    #[tokio::test]
    async fn toml_store_round_trips_and_empty_document_decodes_as_object() {
        let storage = Arc::new(InMemoryStorageService::default());
        let store =
            AtomicDocumentStoreHandle(Arc::new(TomlAtomicDocumentStore::new(storage.clone())));
        store
            .set(
                "session",
                "config.toml",
                &State {
                    title: "hello".into(),
                    count: 2,
                },
            )
            .await
            .unwrap();
        let raw = storage
            .read("session", "config.toml")
            .await
            .unwrap()
            .unwrap();
        assert!(
            String::from_utf8(raw)
                .unwrap()
                .contains("title = \"hello\"")
        );
        assert_eq!(
            store.get::<State>("session", "config.toml").await.unwrap(),
            Some(State {
                title: "hello".into(),
                count: 2
            })
        );

        storage
            .write(
                "session",
                "empty.toml",
                b"  \n",
                StorageWriteOptions::default(),
            )
            .await
            .unwrap();
        let raw_store = TomlAtomicDocumentStore::new(storage);
        assert_eq!(
            raw_store.get_value("session", "empty.toml").await.unwrap(),
            Some(Value::Object(Map::new()))
        );
    }
}
