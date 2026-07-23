//! Blob store facade over filesystem byte storage.
//!
//! Original: `packages/agent-core-v2/src/persistence/backends/node-fs/blobStoreService.ts`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::persistence::interface::{
    blob_store::{BlobReadRange, BlobStoreService as BlobStoreServiceContract},
    storage::{FileSystemStorageService, StorageByteStream, StorageError, StorageWriteOptions},
};

pub struct BlobStoreService {
    storage: Arc<dyn FileSystemStorageService>,
}

impl BlobStoreService {
    pub fn new(storage: Arc<dyn FileSystemStorageService>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl BlobStoreServiceContract for BlobStoreService {
    async fn put(&self, scope: &str, key: &str, data: &[u8]) -> Result<(), StorageError> {
        self.storage
            .write(scope, key, data, StorageWriteOptions { atomic: true })
            .await
    }

    async fn get(&self, scope: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.storage.read(scope, key).await
    }

    fn get_stream(
        &self,
        scope: &str,
        key: &str,
        range: Option<BlobReadRange>,
    ) -> StorageByteStream {
        self.storage.read_stream(scope, key, range)
    }

    // Original: BlobStoreService.has(). Listing uses the full key as a prefix,
    // then requires an exact key match.
    async fn has(&self, scope: &str, key: &str) -> Result<bool, StorageError> {
        Ok(self
            .storage
            .list(scope, Some(key))
            .await?
            .iter()
            .any(|item| item == key))
    }

    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
        self.storage.delete(scope, key).await
    }

    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError> {
        self.storage.list(scope, prefix).await
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;
    use crate::persistence::backends::memory::in_memory_storage_service::InMemoryStorageService;

    #[tokio::test]
    async fn delegates_all_blob_operations_and_requires_exact_has_match() {
        let storage = Arc::new(InMemoryStorageService::default());
        let blobs = BlobStoreService::new(storage);

        blobs.put("agent", "blob", b"abcdef").await.unwrap();
        blobs.put("agent", "blob-child", b"other").await.unwrap();
        assert!(blobs.has("agent", "blob").await.unwrap());
        assert!(!blobs.has("agent", "blo").await.unwrap());
        assert_eq!(
            blobs.get("agent", "blob").await.unwrap(),
            Some(b"abcdef".to_vec())
        );
        assert_eq!(
            blobs
                .get_stream("agent", "blob", Some(BlobReadRange { start: 2, end: 4 }))
                .collect::<Vec<_>>()
                .await[0]
                .as_ref()
                .unwrap(),
            b"cde"
        );
        assert_eq!(
            blobs.list("agent", Some("blob")).await.unwrap(),
            ["blob", "blob-child"]
        );

        blobs.delete("agent", "blob").await.unwrap();
        assert!(!blobs.has("agent", "blob").await.unwrap());
    }
}
