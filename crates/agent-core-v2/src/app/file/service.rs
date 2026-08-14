//! Blob-backed uploaded-file service.
//!
//! Original: `packages/agent-core-v2/src/app/file/fileServiceImpl.ts`.

use parking_lot::Mutex;
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::StreamExt;
use indexmap::IndexMap;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    persistence::interface::blob_store::{BLOB_STORE_SERVICE_ID, BlobReadRange, BlobStoreService},
};

use super::contract::{
    DEFAULT_MAX_UPLOAD_BYTES, FILE_SERVICE_ID, FileByteStream, FileMeta, FileReadRange,
    FileServiceContract, FileServiceError, FileServiceHandle, FileServiceResult, GetResult,
    SaveOptions, ensure_file_errors_registered, file_not_found_error, file_too_large_error,
};

const BLOB_SCOPE: &str = "files";
const INDEX_SCOPE: &str = "file";
const INDEX_KEY: &str = "index.json";
const MAX_JS_DATE_MILLIS: i64 = 8_640_000_000_000_000;
// Legacy persisted indexes may store integer-valued sizes as JSON floats.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

static FILE_ID_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^f_[A-Za-z0-9][A-Za-z0-9_-]*$").expect("file id regex is valid"));

pub struct FileService {
    blobs: Arc<dyn BlobStoreService>,
    index_cache: Mutex<Option<IndexMap<String, FileMeta>>>,
    index_load: AsyncMutex<()>,
}

impl FileService {
    pub fn new(blobs: Arc<dyn BlobStoreService>) -> Self {
        ensure_file_errors_registered();
        Self {
            blobs,
            index_cache: Mutex::new(None),
            index_load: AsyncMutex::new(()),
        }
    }

    // Original: FileServiceImpl.ensureIndex(). The async mutex is the Rust
    // counterpart to the shared indexLoadPromise and is released after either
    // success or failure, allowing a later call to retry.
    async fn ensure_index(&self) -> FileServiceResult<()> {
        if self.index_cache.lock().is_some() {
            return Ok(());
        }
        let _load = self.index_load.lock().await;
        if self.index_cache.lock().is_some() {
            return Ok(());
        }
        self.load_index().await
    }

    // Original: FileServiceImpl.loadIndex(). Invalid JSON and invalid entries
    // are ignored, while blob-store read errors continue to propagate.
    async fn load_index(&self) -> FileServiceResult<()> {
        let Some(raw) = self.blobs.get(INDEX_SCOPE, INDEX_KEY).await? else {
            *self.index_cache.lock() = Some(IndexMap::new());
            return Ok(());
        };
        let mut index = IndexMap::new();
        if let Ok(Value::Object(root)) =
            serde_json::from_str::<Value>(&String::from_utf8_lossy(&raw))
            && let Some(Value::Array(files)) = root.get("files")
        {
            for value in files {
                if let Some(meta) = parse_file_meta(value) {
                    index.insert(meta.id.clone(), meta);
                }
            }
        }
        *self.index_cache.lock() = Some(index);
        Ok(())
    }

    // Original: FileServiceImpl.writeIndex(). The snapshot is serialized
    // before awaiting persistence, matching JSON.stringify's call order.
    async fn write_index(&self) -> FileServiceResult<()> {
        let files = self
            .index_cache
            .lock()
            .as_ref()
            .map(|index| index.values().cloned().collect::<Vec<_>>());
        let Some(files) = files else {
            return Ok(());
        };
        let data = serde_json::to_vec(&IndexFile { version: 1, files })?;
        self.blobs.put(INDEX_SCOPE, INDEX_KEY, &data).await?;
        Ok(())
    }
}

#[async_trait]
impl FileServiceContract for FileService {
    // Original: FileServiceImpl.save(). Upload chunks are fully collected and
    // size-checked before the first blob-store write.
    async fn save(
        &self,
        source: FileByteStream,
        filename: &str,
        options: Option<SaveOptions>,
    ) -> FileServiceResult<FileMeta> {
        self.ensure_index().await?;
        let data = collect_upload(source, DEFAULT_MAX_UPLOAD_BYTES).await?;
        let id = format!("f_{}", uuid::Uuid::new_v4());
        self.blobs.put(BLOB_SCOPE, &id, &data).await?;

        let now: DateTime<Utc> = SystemTime::now().into();
        let now_millis = now.timestamp_millis();
        let options = options.unwrap_or_default();
        let expires_at = options
            .expires_in_sec
            .map(|seconds| {
                let milliseconds = now_millis
                    .saturating_add(seconds.saturating_mul(1_000).try_into().unwrap_or(i64::MAX));
                js_date_to_iso(milliseconds)
            })
            .transpose()?;
        let meta = FileMeta {
            id: id.clone(),
            name: options.name.unwrap_or_else(|| filename.to_owned()),
            media_type: options
                .mime_type
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: data.len() as u64,
            created_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
            expires_at,
        };

        self.index_cache
            .lock()
            .as_mut()
            .expect("ensure_index initialized the cache")
            .insert(id, meta.clone());
        self.write_index().await?;
        Ok(meta)
    }

    // Original: FileServiceImpl.get(). Missing backing blobs prune the cached
    // and persisted index before returning file.not_found.
    async fn get(&self, file_id: &str) -> FileServiceResult<GetResult> {
        if !is_file_id(file_id) {
            return Err(Box::new(file_not_found_error(file_id)));
        }
        self.ensure_index().await?;
        let meta = self
            .index_cache
            .lock()
            .as_ref()
            .and_then(|index| index.get(file_id))
            .cloned()
            .ok_or_else(|| Box::new(file_not_found_error(file_id)) as FileServiceError)?;
        if !self.blobs.has(BLOB_SCOPE, file_id).await? {
            self.index_cache
                .lock()
                .as_mut()
                .expect("ensure_index initialized the cache")
                .shift_remove(file_id);
            self.write_index().await?;
            return Err(Box::new(file_not_found_error(file_id)));
        }

        let blobs = Arc::clone(&self.blobs);
        let id = file_id.to_owned();
        let stream = Arc::new(move |range: Option<FileReadRange>| {
            let range = range.map(|range| BlobReadRange {
                start: range.start,
                end: range.end,
            });
            let stream = blobs
                .get_stream(BLOB_SCOPE, &id, range)
                .map(|item| item.map_err(|error| Box::new(error) as FileServiceError));
            Box::pin(stream) as FileByteStream
        });
        Ok(GetResult { meta, stream })
    }

    // Original: FileServiceImpl.delete(). The cache entry is removed before
    // deleting the blob, and the index is written only after blob deletion.
    async fn delete(&self, file_id: &str) -> FileServiceResult<()> {
        if !is_file_id(file_id) {
            return Err(Box::new(file_not_found_error(file_id)));
        }
        self.ensure_index().await?;
        let removed = self
            .index_cache
            .lock()
            .as_mut()
            .expect("ensure_index initialized the cache")
            .shift_remove(file_id)
            .is_some();
        if !removed {
            return Err(Box::new(file_not_found_error(file_id)));
        }
        self.blobs.delete(BLOB_SCOPE, file_id).await?;
        self.write_index().await
    }
}

#[derive(Serialize)]
struct IndexFile {
    version: u8,
    files: Vec<FileMeta>,
}

fn is_file_id(value: &str) -> bool {
    FILE_ID_PATTERN.is_match(value)
}

fn parse_file_meta(value: &Value) -> Option<FileMeta> {
    let Value::Object(meta) = value else {
        return None;
    };
    let id = meta.get("id")?.as_str()?;
    if !is_file_id(id) {
        return None;
    }
    let name = meta.get("name")?.as_str()?;
    let media_type = meta.get("media_type")?.as_str()?;
    let size = meta
        .get("size")?
        .as_f64()
        .filter(|value| {
            value.is_finite() && value.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER).contains(value)
        })
        .map(|value| value as u64)?;
    let created_at = meta.get("created_at")?.as_str()?;
    let expires_at = match meta.get("expires_at") {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return None,
    };
    Some(FileMeta {
        id: id.into(),
        name: name.into(),
        media_type: media_type.into(),
        size,
        created_at: created_at.into(),
        expires_at,
    })
}

async fn collect_upload(mut source: FileByteStream, limit: u64) -> FileServiceResult<Vec<u8>> {
    let mut data = Vec::new();
    let mut seen = 0_u64;
    while let Some(chunk) = source.next().await {
        let chunk = chunk?;
        seen = seen.saturating_add(chunk.len() as u64);
        if seen > limit {
            return Err(Box::new(file_too_large_error(seen, limit)));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid time value")]
struct InvalidTimeValue;

fn js_date_to_iso(milliseconds: i64) -> Result<String, InvalidTimeValue> {
    if milliseconds.abs() > MAX_JS_DATE_MILLIS {
        return Err(InvalidTimeValue);
    }
    DateTime::<Utc>::from_timestamp_millis(milliseconds)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or(InvalidTimeValue)
}

pub fn register_file_service() {
    ensure_file_errors_registered();
    register_scoped_service(
        LifecycleScope::App,
        FILE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let blobs = accessor.get(BLOB_STORE_SERVICE_ID)?;
            let service: Arc<dyn FileServiceContract> =
                Arc::new(FileService::new(Arc::clone(&blobs.0)));
            Ok(FileServiceHandle(service))
        }),
        InstantiationType::Eager,
        "file",
    );
}

#[cfg(test)]
mod tests {
    use futures_util::{StreamExt, stream};

    use crate::persistence::{
        backends::{
            memory::in_memory_storage_service::InMemoryStorageService,
            node_fs::blob_store_service::BlobStoreService as BlobStore,
        },
        interface::{
            blob_store::BlobStoreService,
            storage::{FileSystemStorageService, StorageWriteOptions},
        },
    };

    use super::*;
    use crate::app::file::{FILE_NOT_FOUND, FILE_TOO_LARGE, FileError};

    fn input(chunks: impl IntoIterator<Item = &'static [u8]>) -> FileByteStream {
        let chunks = chunks
            .into_iter()
            .map(|chunk| Ok::<_, FileServiceError>(chunk.to_vec()))
            .collect::<Vec<_>>();
        Box::pin(stream::iter(chunks))
    }

    fn setup() -> (
        FileService,
        Arc<InMemoryStorageService>,
        Arc<dyn BlobStoreService>,
    ) {
        let storage = Arc::new(InMemoryStorageService::default());
        let blobs: Arc<dyn BlobStoreService> = Arc::new(BlobStore::new(
            Arc::clone(&storage) as Arc<dyn FileSystemStorageService>
        ));
        (FileService::new(Arc::clone(&blobs)), storage, blobs)
    }

    fn file_error_code<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a str> {
        error.downcast_ref::<FileError>().map(FileError::code)
    }

    #[tokio::test]
    async fn saves_gets_ranges_and_deletes_files() -> FileServiceResult<()> {
        let (service, _, _) = setup();
        let meta = service
            .save(
                input([b"hello ".as_slice(), b"world".as_slice()]),
                "hello.txt",
                Some(SaveOptions {
                    mime_type: Some("text/plain".into()),
                    ..SaveOptions::default()
                }),
            )
            .await
            .unwrap();
        assert!(meta.id.starts_with("f_"));
        assert_eq!(meta.name, "hello.txt");
        assert_eq!(meta.media_type, "text/plain");
        assert_eq!(meta.size, 11);

        let result = service.get(&meta.id).await.unwrap();
        assert_eq!(result.meta, meta);
        let bytes = (result.stream)(Some(FileReadRange { start: 6, end: 10 }))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .concat();
        assert_eq!(bytes, b"world");

        service.delete(&meta.id).await.unwrap();
        let error = service.get(&meta.id).await.unwrap_err();
        assert_eq!(file_error_code(error.as_ref()), Some(FILE_NOT_FOUND));
        Ok::<(), FileServiceError>(())
    }

    #[tokio::test]
    async fn honors_overrides_expiry_and_persists_index_across_instances() {
        let (service, _, blobs) = setup();
        let meta = service
            .save(
                input([b"data".as_slice()]),
                "original.bin",
                Some(SaveOptions {
                    name: Some("renamed.bin".into()),
                    expires_in_sec: Some(60),
                    ..SaveOptions::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(meta.name, "renamed.bin");
        assert!(meta.expires_at.is_some());

        let reloaded = FileService::new(blobs);
        assert_eq!(reloaded.get(&meta.id).await.unwrap().meta, meta);
    }

    #[tokio::test]
    async fn rejects_invalid_ids_and_prunes_missing_blobs_from_the_index() {
        let (service, _, blobs) = setup();
        let invalid = service.get("f_../outside").await.unwrap_err();
        assert_eq!(file_error_code(invalid.as_ref()), Some(FILE_NOT_FOUND));

        let meta = service
            .save(input([b"payload".as_slice()]), "p.txt", None)
            .await
            .unwrap();
        blobs.delete(BLOB_SCOPE, &meta.id).await.unwrap();
        let missing = service.get(&meta.id).await.unwrap_err();
        assert_eq!(file_error_code(missing.as_ref()), Some(FILE_NOT_FOUND));

        let reloaded = FileService::new(blobs);
        let missing = reloaded.get(&meta.id).await.unwrap_err();
        assert_eq!(file_error_code(missing.as_ref()), Some(FILE_NOT_FOUND));
    }

    #[tokio::test]
    async fn skips_invalid_persisted_entries_but_keeps_lax_date_strings() {
        let (service, storage, _) = setup();
        storage
            .write(BLOB_SCOPE, "f_valid", b"ok", StorageWriteOptions::default())
            .await
            .unwrap();
        storage
            .write(
                INDEX_SCOPE,
                INDEX_KEY,
                br#"{"version":1,"files":[{"id":"f_valid","name":"valid.txt","media_type":"text/plain","size":2.0,"created_at":"not-validated-here"},{"id":"f_../outside","name":"bad","media_type":"x","size":3,"created_at":"x"},{"id":"f_incomplete"}]}"#,
                StorageWriteOptions::default(),
            )
            .await
            .unwrap();

        let meta = service.get("f_valid").await.unwrap().meta;
        assert_eq!(meta.created_at, "not-validated-here");
        assert_eq!(
            service.get("f_incomplete").await.unwrap_err().to_string(),
            "file not found: f_incomplete"
        );
    }

    #[tokio::test]
    async fn upload_limit_is_checked_before_any_blob_write() {
        let error = collect_upload(input([b"123".as_slice(), b"456".as_slice()]), 5)
            .await
            .unwrap_err();
        assert_eq!(file_error_code(error.as_ref()), Some(FILE_TOO_LARGE));
        let error = error.downcast_ref::<FileError>().unwrap();
        assert_eq!(error.error().details.as_ref().unwrap()["seen"], 6);
        assert_eq!(error.error().details.as_ref().unwrap()["limit"], 5);
    }

    #[tokio::test]
    async fn invalid_expiry_fails_after_blob_write_like_date_to_iso_string() {
        let (service, _, blobs) = setup();
        let error = service
            .save(
                input([b"orphan".as_slice()]),
                "orphan.bin",
                Some(SaveOptions {
                    expires_in_sec: Some(u64::MAX),
                    ..SaveOptions::default()
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Invalid time value");
        assert_eq!(blobs.list(BLOB_SCOPE, None).await.unwrap().len(), 1);
    }
}
