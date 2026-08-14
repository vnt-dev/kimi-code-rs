//! Atomic typed-document persistence contract.
//!
//! Original: `packages/agent-core-v2/src/persistence/interface/atomicDocumentStore.ts`.
//!
//! Rust adaptation: injectable storage and codec traits exchange JSON values
//! to remain object safe. The public handle restores typed `get` and `set`
//! operations using Serde at the Rust boundary.

use std::{error::Error, sync::Arc};

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::_base::{
    di::{instantiation::ServiceIdentifier, lifecycle::DisposableHandle},
    errors::errors::{Error2Options, ErrorCause},
    event::Event,
};

use super::storage::{STORAGE_DECODE_FAILED, StorageError};

pub trait DocumentCodec: Send + Sync {
    fn format(&self) -> &str;
    fn encode(&self, value: &Value) -> Result<Vec<u8>, StorageError>;
    fn decode(&self, bytes: &[u8]) -> Result<Value, StorageError>;
}

#[async_trait]
pub trait AtomicDocumentStoreService: Send + Sync {
    async fn get_value(&self, scope: &str, key: &str) -> Result<Option<Value>, StorageError>;
    async fn set_value(&self, scope: &str, key: &str, value: Value) -> Result<(), StorageError>;
    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError>;
    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError>;
    fn watch(&self, scope: &str, key: &str) -> Event<()>;
    fn acquire(&self, scope: &str, key: &str) -> DisposableHandle;
}

#[derive(Clone)]
pub struct AtomicDocumentStoreHandle(pub Arc<dyn AtomicDocumentStoreService>);

impl AtomicDocumentStoreHandle {
    // Original: IAtomicDocumentStore.get<T>().
    pub async fn get<T: DeserializeOwned>(
        &self,
        scope: &str,
        key: &str,
    ) -> Result<Option<T>, StorageError> {
        self.0
            .get_value(scope, key)
            .await?
            .map(|value| serde_json::from_value(value).map_err(document_conversion_error))
            .transpose()
    }

    // Original: IAtomicDocumentStore.set<T>(). Serialization happens before
    // entering the backend's atomic write boundary.
    pub async fn set<T: Serialize>(
        &self,
        scope: &str,
        key: &str,
        value: &T,
    ) -> Result<(), StorageError> {
        let value = serde_json::to_value(value).map_err(document_conversion_error)?;
        self.0.set_value(scope, key, value).await
    }

    pub async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
        self.0.delete(scope, key).await
    }

    pub async fn list(
        &self,
        scope: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<String>, StorageError> {
        self.0.list(scope, prefix).await
    }

    pub fn watch(&self, scope: &str, key: &str) -> Event<()> {
        self.0.watch(scope, key)
    }

    pub fn acquire(&self, scope: &str, key: &str) -> DisposableHandle {
        self.0.acquire(scope, key)
    }
}

fn document_conversion_error(error: serde_json::Error) -> StorageError {
    let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
    StorageError::with_options(
        STORAGE_DECODE_FAILED,
        "stored document could not be converted to its requested Rust type",
        Error2Options {
            cause: Some(ErrorCause::Error(cause)),
            ..Error2Options::default()
        },
    )
}

pub const ATOMIC_DOCUMENT_STORE_SERVICE_ID: ServiceIdentifier<AtomicDocumentStoreHandle> =
    ServiceIdentifier::new("atomicDocumentStore");

pub const ATOMIC_TOML_DOCUMENT_STORE_SERVICE_ID: ServiceIdentifier<AtomicDocumentStoreHandle> =
    ServiceIdentifier::new("atomicTomlDocumentStore");

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::_base::di::lifecycle::disposable_none;

    #[derive(Default)]
    struct StubAtomicDocumentStore {
        value: Mutex<Option<Value>>,
    }

    #[async_trait]
    impl AtomicDocumentStoreService for StubAtomicDocumentStore {
        async fn get_value(&self, _scope: &str, _key: &str) -> Result<Option<Value>, StorageError> {
            Ok(self.value.lock().clone())
        }

        async fn set_value(
            &self,
            _scope: &str,
            _key: &str,
            value: Value,
        ) -> Result<(), StorageError> {
            *self.value.lock() = Some(value);
            Ok(())
        }

        async fn delete(&self, _scope: &str, _key: &str) -> Result<(), StorageError> {
            *self.value.lock() = None;
            Ok(())
        }

        async fn list(
            &self,
            _scope: &str,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, StorageError> {
            Ok(vec!["config.json".into()])
        }

        fn watch(&self, _scope: &str, _key: &str) -> Event<()> {
            Event::none()
        }

        fn acquire(&self, _scope: &str, _key: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Document {
        enabled: bool,
    }

    #[tokio::test]
    async fn typed_handle_round_trips_documents_and_deletes_them() {
        let handle = AtomicDocumentStoreHandle(Arc::new(StubAtomicDocumentStore::default()));
        assert_eq!(handle.get::<Document>("app", "config").await.unwrap(), None);

        handle
            .set("app", "config", &Document { enabled: true })
            .await
            .unwrap();
        assert_eq!(
            handle.get::<Document>("app", "config").await.unwrap(),
            Some(Document { enabled: true })
        );

        handle.delete("app", "config").await.unwrap();
        assert_eq!(handle.get::<Document>("app", "config").await.unwrap(), None);
    }

    #[test]
    fn service_identifiers_preserve_original_names() {
        assert_eq!(
            ATOMIC_DOCUMENT_STORE_SERVICE_ID.to_string(),
            "atomicDocumentStore"
        );
        assert_eq!(
            ATOMIC_TOML_DOCUMENT_STORE_SERVICE_ID.to_string(),
            "atomicTomlDocumentStore"
        );
    }
}
