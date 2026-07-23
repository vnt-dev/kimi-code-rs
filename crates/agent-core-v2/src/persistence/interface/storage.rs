//! Byte-oriented filesystem persistence contract and structured errors.
//!
//! Original: `packages/agent-core-v2/src/persistence/interface/storage.ts`.

use std::{error::Error, fmt, pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_util::Stream;
use serde_json::{Map, Value};

use crate::_base::{
    di::instantiation::ServiceIdentifier,
    errors::{
        codes::{ErrorDomain, ErrorInfo, register_error_domain},
        errors::{Error2, Error2Options, ErrorCause},
    },
    event::Event,
};

pub const STORAGE_NOT_FOUND: &str = "storage.not_found";
pub const STORAGE_DECODE_FAILED: &str = "storage.decode_failed";
pub const STORAGE_CORRUPTED: &str = "storage.corrupted";
pub const STORAGE_IO_FAILED: &str = "storage.io_failed";
pub const STORAGE_LOCKED: &str = "storage.locked";

pub static STORAGE_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("STORAGE_NOT_FOUND", STORAGE_NOT_FOUND),
        ("STORAGE_DECODE_FAILED", STORAGE_DECODE_FAILED),
        ("STORAGE_CORRUPTED", STORAGE_CORRUPTED),
        ("STORAGE_IO_FAILED", STORAGE_IO_FAILED),
        ("STORAGE_LOCKED", STORAGE_LOCKED),
    ],
    retryable: &[STORAGE_IO_FAILED, STORAGE_LOCKED],
    info: &[
        (
            STORAGE_NOT_FOUND,
            ErrorInfo {
                title: "Stored value not found",
                retryable: false,
                public: true,
                action: None,
            },
        ),
        (
            STORAGE_DECODE_FAILED,
            ErrorInfo {
                title: "Stored value could not be decoded",
                retryable: false,
                public: true,
                action: Some(
                    "Inspect the stored document; it is not valid for its declared format.",
                ),
            },
        ),
        (
            STORAGE_CORRUPTED,
            ErrorInfo {
                title: "Stored data is corrupted",
                retryable: false,
                public: true,
                action: Some(
                    "Inspect the backing store; the corrupted entry must be repaired or dropped.",
                ),
            },
        ),
        (
            STORAGE_IO_FAILED,
            ErrorInfo {
                title: "Storage I/O failed",
                retryable: true,
                public: true,
                action: None,
            },
        ),
        (
            STORAGE_LOCKED,
            ErrorInfo {
                title: "Storage is locked",
                retryable: true,
                public: true,
                action: Some("Another process holds the store; close it or retry later."),
            },
        ),
    ],
};

static STORAGE_ERRORS_REGISTERED: std::sync::LazyLock<()> = std::sync::LazyLock::new(|| {
    register_error_domain(&STORAGE_ERRORS).expect("storage error codes are unique");
});

pub fn ensure_storage_errors_registered() {
    std::sync::LazyLock::force(&STORAGE_ERRORS_REGISTERED);
}

#[derive(Debug)]
pub struct StorageError {
    inner: Box<Error2>,
}

impl StorageError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_options(code, message, Error2Options::default())
    }

    pub fn with_options(
        code: &'static str,
        message: impl Into<String>,
        mut options: Error2Options,
    ) -> Self {
        ensure_storage_errors_registered();
        options.name.get_or_insert_with(|| "StorageError".into());
        Self {
            inner: Box::new(Error2::with_options(code, message, options)),
        }
    }

    pub fn code(&self) -> &str {
        &self.inner.code
    }

    pub fn error(&self) -> &Error2 {
        &self.inner
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}

pub fn is_storage_error(error: &(dyn Error + 'static), code: &str) -> bool {
    error
        .downcast_ref::<StorageError>()
        .is_some_and(|error| error.code() == code)
}

// Original: toStorageIoError(). Existing StorageError values pass through.
pub fn to_storage_io_error(
    error: Box<dyn Error + Send + Sync>,
    path: &str,
    operation: &str,
) -> StorageError {
    let error = match error.downcast::<StorageError>() {
        Ok(error) => return *error,
        Err(error) => error,
    };
    let mut details = Map::from_iter([
        ("path".into(), Value::String(path.into())),
        ("op".into(), Value::String(operation.into())),
    ]);
    if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
        let errno = io_error_code(io_error);
        details.insert("errno".into(), Value::String(errno));
    }
    let cause: Arc<dyn Error + Send + Sync> = Arc::from(error);
    StorageError::with_options(
        STORAGE_IO_FAILED,
        format!("storage {operation} failed"),
        Error2Options {
            details: Some(details),
            cause: Some(ErrorCause::Error(cause)),
            ..Error2Options::default()
        },
    )
}

fn io_error_code(error: &std::io::Error) -> String {
    use std::io::ErrorKind;

    let symbolic = match error.kind() {
        ErrorKind::NotFound => Some("ENOENT"),
        ErrorKind::PermissionDenied => Some("EACCES"),
        ErrorKind::AlreadyExists => Some("EEXIST"),
        ErrorKind::NotADirectory => Some("ENOTDIR"),
        ErrorKind::IsADirectory => Some("EISDIR"),
        ErrorKind::WouldBlock => Some("EWOULDBLOCK"),
        _ => None,
    };
    symbolic.map(str::to_owned).unwrap_or_else(|| {
        error
            .raw_os_error()
            .map_or_else(|| format!("{:?}", error.kind()), |errno| errno.to_string())
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StorageWriteOptions {
    pub atomic: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageAppendOptions {
    pub durable: bool,
}

impl Default for StorageAppendOptions {
    fn default() -> Self {
        Self { durable: true }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageReadRange {
    pub start: u64,
    pub end: u64,
}

pub type StorageByteStream =
    Pin<Box<dyn Stream<Item = Result<Vec<u8>, StorageError>> + Send + 'static>>;

#[async_trait]
pub trait FileSystemStorageService: Send + Sync {
    async fn read(&self, scope: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError>;

    fn read_stream(
        &self,
        scope: &str,
        key: &str,
        range: Option<StorageReadRange>,
    ) -> StorageByteStream;

    async fn write(
        &self,
        scope: &str,
        key: &str,
        data: &[u8],
        options: StorageWriteOptions,
    ) -> Result<(), StorageError>;

    async fn append(
        &self,
        scope: &str,
        key: &str,
        data: &[u8],
        options: StorageAppendOptions,
    ) -> Result<(), StorageError>;

    async fn list(&self, scope: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError>;
    async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError>;

    fn watch(&self, _scope: &str, _key: &str) -> Option<Event<()>> {
        None
    }

    async fn flush(&self) -> Result<(), StorageError>;
    async fn close(&self) -> Result<(), StorageError>;
}

#[derive(Clone)]
pub struct FileSystemStorageServiceHandle(pub Arc<dyn FileSystemStorageService>);

pub const FILE_SYSTEM_STORAGE_SERVICE_ID: ServiceIdentifier<FileSystemStorageServiceHandle> =
    ServiceIdentifier::new("fileSystemStorageService");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn storage_domain_preserves_retryability_and_actions() {
        ensure_storage_errors_registered();
        assert!(is_error_code(STORAGE_NOT_FOUND));
        assert!(!error_info(STORAGE_CORRUPTED).retryable);
        assert!(error_info(STORAGE_IO_FAILED).retryable);
        assert!(error_info(STORAGE_LOCKED).retryable);
        assert_eq!(
            error_info(STORAGE_DECODE_FAILED).action.as_deref(),
            Some("Inspect the stored document; it is not valid for its declared format.")
        );
    }

    #[test]
    fn io_translation_passes_storage_errors_and_wraps_other_causes() {
        let existing = StorageError::new(STORAGE_LOCKED, "locked");
        let passed = to_storage_io_error(Box::new(existing), "/tmp/db", "append");
        assert_eq!(passed.code(), STORAGE_LOCKED);

        let wrapped = to_storage_io_error(
            Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
            "/tmp/db",
            "write",
        );
        assert_eq!(wrapped.code(), STORAGE_IO_FAILED);
        assert_eq!(wrapped.to_string(), "storage write failed");
        assert_eq!(wrapped.error().details.as_ref().unwrap()["path"], "/tmp/db");
        assert!(wrapped.source().is_some());
        assert!(is_storage_error(&wrapped, STORAGE_IO_FAILED));
    }

    #[test]
    fn service_identifier_preserves_original_name() {
        assert_eq!(
            FILE_SYSTEM_STORAGE_SERVICE_ID.to_string(),
            "fileSystemStorageService"
        );
    }
}
