//! Write-once, key-addressed blob persistence contract.
//!
//! Original: `packages/agent-core-v2/src/persistence/interface/blobStore.ts`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::_base::di::instantiation::ServiceIdentifier;

use super::storage::{StorageByteStream, StorageError, StorageReadRange};

pub type BlobReadRange = StorageReadRange;

#[async_trait]
pub trait BlobStoreService: Send + Sync {
    async fn put(&self, scope: &str, key: &str, data: &[u8]) -> Result<(), StorageError>;
    async fn get(&self, scope: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    fn get_stream(&self, scope: &str, key: &str, range: Option<BlobReadRange>)
    -> StorageByteStream;
    async fn has(&self, scope: &str, key: &str) -> Result<bool, StorageError>;
    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError>;
    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError>;
}

#[derive(Clone)]
pub struct BlobStoreHandle(pub Arc<dyn BlobStoreService>);

pub const BLOB_STORE_SERVICE_ID: ServiceIdentifier<BlobStoreHandle> =
    ServiceIdentifier::new("blobStore");

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::collections::HashMap;

    use futures_util::{StreamExt, stream};

    use super::*;

    #[derive(Default)]
    struct StubBlobStore {
        values: Mutex<HashMap<(String, String), Vec<u8>>>,
    }

    #[async_trait]
    impl BlobStoreService for StubBlobStore {
        async fn put(&self, scope: &str, key: &str, data: &[u8]) -> Result<(), StorageError> {
            self.values
                .lock()
                .insert((scope.into(), key.into()), data.into());
            Ok(())
        }

        async fn get(&self, scope: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
            Ok(self.values.lock().get(&(scope.into(), key.into())).cloned())
        }

        fn get_stream(
            &self,
            scope: &str,
            key: &str,
            _range: Option<BlobReadRange>,
        ) -> StorageByteStream {
            let value = self.values.lock().get(&(scope.into(), key.into())).cloned();
            Box::pin(stream::iter(value.into_iter().map(Ok)))
        }

        async fn has(&self, scope: &str, key: &str) -> Result<bool, StorageError> {
            Ok(self.values.lock().contains_key(&(scope.into(), key.into())))
        }

        async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
            self.values.lock().remove(&(scope.into(), key.into()));
            Ok(())
        }

        async fn list(
            &self,
            scope: &str,
            prefix: Option<&str>,
        ) -> Result<Vec<String>, StorageError> {
            let mut keys = self
                .values
                .lock()
                .keys()
                .filter(|(stored_scope, key)| {
                    stored_scope == scope && prefix.is_none_or(|prefix| key.starts_with(prefix))
                })
                .map(|(_, key)| key.clone())
                .collect::<Vec<_>>();
            keys.sort();
            Ok(keys)
        }
    }

    #[tokio::test]
    async fn contract_preserves_binary_and_stream_operations() {
        let store: Arc<dyn BlobStoreService> = Arc::new(StubBlobStore::default());
        store.put("agent", "sha256", b"payload").await.unwrap();
        assert!(store.has("agent", "sha256").await.unwrap());
        assert_eq!(
            store.get("agent", "sha256").await.unwrap(),
            Some(b"payload".to_vec())
        );
        assert_eq!(
            store
                .get_stream("agent", "sha256", None)
                .collect::<Vec<_>>()
                .await
                .pop()
                .unwrap()
                .unwrap(),
            b"payload"
        );
        assert_eq!(store.list("agent", Some("sha")).await.unwrap(), ["sha256"]);
        store.delete("agent", "sha256").await.unwrap();
        assert!(!store.has("agent", "sha256").await.unwrap());
    }

    #[test]
    fn range_and_identifier_preserve_source_contract() {
        let range = BlobReadRange { start: 3, end: 9 };
        assert_eq!(range.start, 3);
        assert_eq!(range.end, 9);
        assert_eq!(BLOB_STORE_SERVICE_ID.to_string(), "blobStore");
    }
}
