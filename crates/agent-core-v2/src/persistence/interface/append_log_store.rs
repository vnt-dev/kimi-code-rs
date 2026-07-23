//! Ordered JSON-record append-log persistence contract.
//!
//! Original: `packages/agent-core-v2/src/persistence/interface/appendLogStore.ts`.
//!
//! Rust adaptation: the injectable trait operates on `serde_json::Value` so it
//! remains object safe. `AppendLogStoreHandle` supplies the original typed
//! `append`, `read`, and `rewrite` responsibilities at the public boundary.

use std::{error::Error, fmt, pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::_base::{
    di::{instantiation::ServiceIdentifier, lifecycle::DisposableHandle},
    errors::errors::{Error2Options, ErrorCause},
};

use super::storage::{STORAGE_CORRUPTED, STORAGE_DECODE_FAILED, StorageError};

#[derive(Clone, Debug)]
pub struct AppendLogCorruptedError {
    inner: StorageError,
}

impl AppendLogCorruptedError {
    pub fn new(
        scope: impl Into<String>,
        key: impl Into<String>,
        line_number: u64,
        cause: Arc<dyn Error + Send + Sync>,
    ) -> Self {
        let scope = scope.into();
        let key = key.into();
        let details = Map::from_iter([
            ("scope".into(), Value::String(scope.clone())),
            ("key".into(), Value::String(key.clone())),
            ("lineNumber".into(), Value::from(line_number)),
        ]);
        Self {
            inner: StorageError::with_options(
                STORAGE_CORRUPTED,
                format!("append-log {scope}/{key}: corrupted line {line_number}"),
                Error2Options {
                    details: Some(details),
                    cause: Some(ErrorCause::Error(cause)),
                    name: Some("AppendLogCorruptedError".into()),
                },
            ),
        }
    }

    pub fn code(&self) -> &str {
        self.inner.code()
    }

    pub fn details(&self) -> Option<&Map<String, Value>> {
        self.inner.error().details.as_ref()
    }

    pub fn storage_error(&self) -> &StorageError {
        &self.inner
    }
}

impl fmt::Display for AppendLogCorruptedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for AppendLogCorruptedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum AppendLogError {
    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Corrupted(#[from] AppendLogCorruptedError),
}

pub type AppendLogErrorHandler = Arc<dyn Fn(&AppendLogError) + Send + Sync + 'static>;

#[derive(Clone, Default)]
pub struct AppendLogOptions {
    pub on_error: Option<AppendLogErrorHandler>,
}

pub type AppendLogValueStream =
    Pin<Box<dyn Stream<Item = Result<Value, AppendLogError>> + Send + 'static>>;

#[async_trait]
pub trait AppendLogStoreService: Send + Sync {
    fn append_value(&self, scope: &str, key: &str, record: Value, options: AppendLogOptions);

    fn read_values(&self, scope: &str, key: &str) -> AppendLogValueStream;

    async fn rewrite_values(
        &self,
        scope: &str,
        key: &str,
        records: Vec<Value>,
    ) -> Result<(), AppendLogError>;

    async fn flush(&self) -> Result<(), AppendLogError>;
    async fn close(&self) -> Result<(), AppendLogError>;
    fn acquire(&self, scope: &str, key: &str) -> DisposableHandle;
}

#[derive(Clone)]
pub struct AppendLogStoreHandle(pub Arc<dyn AppendLogStoreService>);

impl AppendLogStoreHandle {
    // Original: IAppendLogStore.append<R>(). Serialization is made explicit at
    // the Rust boundary because arbitrary Rust values are not JSON-shaped.
    pub fn append<R: Serialize>(
        &self,
        scope: &str,
        key: &str,
        record: &R,
        options: AppendLogOptions,
    ) -> Result<(), AppendLogError> {
        let record = serde_json::to_value(record).map_err(codec_error)?;
        self.0.append_value(scope, key, record, options);
        Ok(())
    }

    // Original: IAppendLogStore.read<R>().
    pub fn read<R>(&self, scope: &str, key: &str) -> TypedAppendLogStream<R>
    where
        R: DeserializeOwned + Send + 'static,
    {
        Box::pin(self.0.read_values(scope, key).map(|record| {
            record.and_then(|record| serde_json::from_value(record).map_err(codec_error))
        }))
    }

    // Original: IAppendLogStore.rewrite<R>(). Values are completely encoded
    // before the backend's atomic cutover boundary is entered.
    pub async fn rewrite<R: Serialize>(
        &self,
        scope: &str,
        key: &str,
        records: &[R],
    ) -> Result<(), AppendLogError> {
        let records = records
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(codec_error)?;
        self.0.rewrite_values(scope, key, records).await
    }

    pub async fn flush(&self) -> Result<(), AppendLogError> {
        self.0.flush().await
    }

    pub async fn close(&self) -> Result<(), AppendLogError> {
        self.0.close().await
    }

    pub fn acquire(&self, scope: &str, key: &str) -> DisposableHandle {
        self.0.acquire(scope, key)
    }
}

pub type TypedAppendLogStream<R> =
    Pin<Box<dyn Stream<Item = Result<R, AppendLogError>> + Send + 'static>>;

fn codec_error(error: serde_json::Error) -> AppendLogError {
    let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
    StorageError::with_options(
        STORAGE_DECODE_FAILED,
        "append-log record could not be converted to its requested Rust type",
        Error2Options {
            cause: Some(ErrorCause::Error(cause)),
            ..Error2Options::default()
        },
    )
    .into()
}

pub const APPEND_LOG_STORE_SERVICE_ID: ServiceIdentifier<AppendLogStoreHandle> =
    ServiceIdentifier::new("appendLogStore");

#[cfg(test)]
mod tests {
    use futures_util::stream;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::_base::di::lifecycle::disposable_none;

    #[derive(Default)]
    struct StubAppendLog;

    #[async_trait]
    impl AppendLogStoreService for StubAppendLog {
        fn append_value(
            &self,
            _scope: &str,
            _key: &str,
            _record: Value,
            _options: AppendLogOptions,
        ) {
        }

        fn read_values(&self, _scope: &str, _key: &str) -> AppendLogValueStream {
            Box::pin(stream::iter([Ok(serde_json::json!({"n": 7}))]))
        }

        async fn rewrite_values(
            &self,
            _scope: &str,
            _key: &str,
            _records: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn flush(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        fn acquire(&self, _scope: &str, _key: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Record {
        n: u8,
    }

    #[tokio::test]
    async fn typed_handle_preserves_record_conversion_and_streaming() {
        let handle = AppendLogStoreHandle(Arc::new(StubAppendLog));
        handle
            .append("agent", "wire.jsonl", &Record { n: 1 }, Default::default())
            .unwrap();
        handle
            .rewrite("agent", "wire.jsonl", &[Record { n: 2 }])
            .await
            .unwrap();
        let records = handle
            .read::<Record>("agent", "wire.jsonl")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].as_ref().unwrap(), &Record { n: 7 });
    }

    #[test]
    fn corrupted_error_preserves_source_name_code_details_and_cause() {
        let cause: Arc<dyn Error + Send + Sync> = Arc::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad json",
        ));
        let error = AppendLogCorruptedError::new("agents/main", "wire.jsonl", 2, cause);
        assert_eq!(error.code(), STORAGE_CORRUPTED);
        assert_eq!(
            error.to_string(),
            "append-log agents/main/wire.jsonl: corrupted line 2"
        );
        assert_eq!(error.details().unwrap()["lineNumber"], 2);
        assert!(error.source().is_some());
        assert_eq!(
            error.storage_error().error().name,
            "AppendLogCorruptedError"
        );
    }

    #[test]
    fn service_identifier_preserves_original_name() {
        assert_eq!(APPEND_LOG_STORE_SERVICE_ID.to_string(), "appendLogStore");
    }
}
