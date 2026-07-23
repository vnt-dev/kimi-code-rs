//! Uploaded-file protocol, service contract, and domain errors.
//!
//! Original: `packages/agent-core-v2/src/app/file/fileService.ts`.

use std::{
    error::Error,
    fmt,
    ops::Deref,
    pin::Pin,
    sync::{Arc, LazyLock},
};

use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::_base::{
    di::instantiation::ServiceIdentifier,
    errors::{
        codes::{ErrorDomain, ErrorInfo, register_error_domain},
        errors::{Error2, Error2Options},
    },
};

pub const DEFAULT_MAX_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;

pub const FILE_NOT_FOUND: &str = "file.not_found";
pub const FILE_TOO_LARGE: &str = "file.too_large";

pub static FILE_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("FILE_NOT_FOUND", FILE_NOT_FOUND),
        ("FILE_TOO_LARGE", FILE_TOO_LARGE),
    ],
    retryable: &[],
    info: &[
        (
            FILE_NOT_FOUND,
            ErrorInfo {
                title: "File not found",
                retryable: false,
                public: true,
                action: Some("Check the file_id or upload the file again."),
            },
        ),
        (
            FILE_TOO_LARGE,
            ErrorInfo {
                title: "Upload too large",
                retryable: false,
                public: true,
                action: Some("Upload a smaller file (limit is 50 MiB)."),
            },
        ),
    ],
};

static FILE_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&FILE_ERRORS).expect("file error codes are unique");
});

pub fn ensure_file_errors_registered() {
    LazyLock::force(&FILE_ERRORS_REGISTERED);
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileMeta {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveOptions {
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub expires_in_sec: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileReadRange {
    pub start: u64,
    pub end: u64,
}

pub type FileServiceError = Box<dyn Error + Send + Sync>;
pub type FileServiceResult<T> = Result<T, FileServiceError>;
pub type FileByteStream =
    Pin<Box<dyn Stream<Item = Result<Vec<u8>, FileServiceError>> + Send + 'static>>;
pub type FileReadStreamFactory = Arc<dyn Fn(Option<FileReadRange>) -> FileByteStream + Send + Sync>;

#[derive(Clone)]
pub struct GetResult {
    pub meta: FileMeta,
    pub stream: FileReadStreamFactory,
}

impl fmt::Debug for GetResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GetResult")
            .field("meta", &self.meta)
            .field("stream", &"<file stream factory>")
            .finish()
    }
}

#[async_trait]
pub trait FileServiceContract: Send + Sync {
    async fn save(
        &self,
        source: FileByteStream,
        filename: &str,
        options: Option<SaveOptions>,
    ) -> FileServiceResult<FileMeta>;

    async fn get(&self, file_id: &str) -> FileServiceResult<GetResult>;
    async fn delete(&self, file_id: &str) -> FileServiceResult<()>;
}

#[derive(Clone)]
pub struct FileServiceHandle(pub Arc<dyn FileServiceContract>);

impl Deref for FileServiceHandle {
    type Target = dyn FileServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const FILE_SERVICE_ID: ServiceIdentifier<FileServiceHandle> =
    ServiceIdentifier::new("fileService");

#[derive(Clone, Debug)]
pub struct FileError {
    inner: Box<Error2>,
}

impl FileError {
    pub fn new(
        code: &'static str,
        message: impl Into<String>,
        details: Option<Map<String, Value>>,
    ) -> Self {
        ensure_file_errors_registered();
        Self {
            inner: Box::new(Error2::with_options(
                code,
                message,
                Error2Options {
                    details,
                    name: Some("FileError".into()),
                    ..Error2Options::default()
                },
            )),
        }
    }

    pub fn code(&self) -> &str {
        &self.inner.code
    }

    pub fn error(&self) -> &Error2 {
        &self.inner
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for FileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}

// Original: fileNotFoundError().
pub fn file_not_found_error(file_id: &str) -> FileError {
    FileError::new(
        FILE_NOT_FOUND,
        format!("file not found: {file_id}"),
        Some(Map::from_iter([(
            "fileId".into(),
            Value::String(file_id.into()),
        )])),
    )
}

// Original: fileTooLargeError().
pub fn file_too_large_error(seen: u64, limit: u64) -> FileError {
    FileError::new(
        FILE_TOO_LARGE,
        format!("upload size {seen} bytes exceeds limit {limit} bytes"),
        Some(Map::from_iter([
            ("seen".into(), Value::from(seen)),
            ("limit".into(), Value::from(limit)),
        ])),
    )
}

// Original: isFileError(). Rust FileError wraps Error2 instead of inheriting
// from it, so both the domain wrapper and a boundary Error2 are recognized.
pub fn is_file_error(error: &(dyn Error + 'static), code: &str) -> bool {
    error
        .downcast_ref::<FileError>()
        .is_some_and(|error| error.code() == code)
        || error
            .downcast_ref::<Error2>()
            .is_some_and(|error| error.code == code)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::_base::errors::codes::error_info;

    use super::*;

    #[test]
    fn metadata_preserves_wire_names_and_optional_expiry() {
        let meta = FileMeta {
            id: "f_id".into(),
            name: "hello.txt".into(),
            media_type: "text/plain".into(),
            size: 5,
            created_at: "1970-01-01T00:00:00.000Z".into(),
            expires_at: None,
        };
        assert_eq!(
            serde_json::to_value(meta).unwrap(),
            json!({
                "id": "f_id",
                "name": "hello.txt",
                "media_type": "text/plain",
                "size": 5,
                "created_at": "1970-01-01T00:00:00.000Z"
            })
        );
        assert_eq!(FILE_SERVICE_ID.to_string(), "fileService");
        assert_eq!(DEFAULT_MAX_UPLOAD_BYTES, 52_428_800);
    }

    #[test]
    fn error_helpers_preserve_codes_messages_details_and_metadata() {
        let missing = file_not_found_error("f_missing");
        assert_eq!(missing.code(), FILE_NOT_FOUND);
        assert_eq!(missing.to_string(), "file not found: f_missing");
        assert_eq!(missing.error().name, "FileError");
        assert_eq!(
            missing.error().details.as_ref().unwrap()["fileId"],
            "f_missing"
        );
        assert!(is_file_error(&missing, FILE_NOT_FOUND));

        let large = file_too_large_error(11, 10);
        assert_eq!(large.code(), FILE_TOO_LARGE);
        assert_eq!(
            large.to_string(),
            "upload size 11 bytes exceeds limit 10 bytes"
        );
        assert_eq!(large.error().details.as_ref().unwrap()["seen"], 11);

        assert_eq!(
            error_info(FILE_NOT_FOUND).action.as_deref(),
            Some("Check the file_id or upload the file again.")
        );
        assert_eq!(
            error_info(FILE_TOO_LARGE).action.as_deref(),
            Some("Upload a smaller file (limit is 50 MiB).")
        );
    }
}
