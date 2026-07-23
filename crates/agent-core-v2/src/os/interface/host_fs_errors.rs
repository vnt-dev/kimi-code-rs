//! Stable host-filesystem error domain and boundary translation.
//!
//! Original: `packages/agent-core-v2/src/os/interface/hostFsErrors.ts`.

use std::{
    error::Error,
    fmt, io,
    sync::{Arc, LazyLock},
};

use serde_json::{Map, Value};

use crate::_base::errors::{
    codes::{ErrorDomain, ErrorInfo, register_error_domain},
    errors::{Error2, Error2Options, ErrorCause},
};

pub const OS_FS_NOT_FOUND: &str = "os.fs.not_found";
pub const OS_FS_IS_DIRECTORY: &str = "os.fs.is_directory";
pub const OS_FS_NOT_DIRECTORY: &str = "os.fs.not_directory";
pub const OS_FS_ALREADY_EXISTS: &str = "os.fs.already_exists";
pub const OS_FS_PERMISSION_DENIED: &str = "os.fs.permission_denied";
pub const OS_FS_NOT_EMPTY: &str = "os.fs.not_empty";
pub const OS_FS_UNAVAILABLE: &str = "os.fs.unavailable";
pub const OS_FS_UNKNOWN: &str = "os.fs.unknown";

pub static OS_FS_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("OS_FS_NOT_FOUND", OS_FS_NOT_FOUND),
        ("OS_FS_IS_DIRECTORY", OS_FS_IS_DIRECTORY),
        ("OS_FS_NOT_DIRECTORY", OS_FS_NOT_DIRECTORY),
        ("OS_FS_ALREADY_EXISTS", OS_FS_ALREADY_EXISTS),
        ("OS_FS_PERMISSION_DENIED", OS_FS_PERMISSION_DENIED),
        ("OS_FS_NOT_EMPTY", OS_FS_NOT_EMPTY),
        ("OS_FS_UNAVAILABLE", OS_FS_UNAVAILABLE),
        ("OS_FS_UNKNOWN", OS_FS_UNKNOWN),
    ],
    retryable: &[OS_FS_UNAVAILABLE, OS_FS_UNKNOWN],
    info: &[
        (
            OS_FS_NOT_FOUND,
            ErrorInfo {
                title: "Path not found",
                retryable: false,
                public: true,
                action: None,
            },
        ),
        (
            OS_FS_IS_DIRECTORY,
            ErrorInfo {
                title: "Path is a directory",
                retryable: false,
                public: true,
                action: None,
            },
        ),
        (
            OS_FS_NOT_DIRECTORY,
            ErrorInfo {
                title: "Path is not a directory",
                retryable: false,
                public: true,
                action: None,
            },
        ),
        (
            OS_FS_ALREADY_EXISTS,
            ErrorInfo {
                title: "Path already exists",
                retryable: false,
                public: true,
                action: None,
            },
        ),
        (
            OS_FS_PERMISSION_DENIED,
            ErrorInfo {
                title: "Permission denied",
                retryable: false,
                public: true,
                action: Some("Check the file permissions of the target path."),
            },
        ),
        (
            OS_FS_NOT_EMPTY,
            ErrorInfo {
                title: "Directory not empty",
                retryable: false,
                public: true,
                action: None,
            },
        ),
        (
            OS_FS_UNAVAILABLE,
            ErrorInfo {
                title: "Filesystem unavailable",
                retryable: true,
                public: true,
                action: None,
            },
        ),
        (
            OS_FS_UNKNOWN,
            ErrorInfo {
                title: "Filesystem error",
                retryable: true,
                public: true,
                action: None,
            },
        ),
    ],
};

static OS_FS_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&OS_FS_ERRORS).expect("host filesystem error codes are unique");
});

pub fn ensure_os_fs_errors_registered() {
    LazyLock::force(&OS_FS_ERRORS_REGISTERED);
}

#[derive(Debug)]
pub struct HostFsError {
    inner: Box<Error2>,
}

impl HostFsError {
    pub fn with_options(
        code: &'static str,
        message: impl Into<String>,
        mut options: Error2Options,
    ) -> Self {
        ensure_os_fs_errors_registered();
        options.name.get_or_insert_with(|| "HostFsError".into());
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

impl fmt::Display for HostFsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for HostFsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}

/// Portable representation of the Node `ErrnoException` fields used by the
/// original pure translator. Backends may use this when an API exposes a
/// symbolic errno and syscall directly.
#[derive(Debug)]
pub struct HostFsRawError {
    pub message: String,
    pub errno: Option<String>,
    pub syscall: Option<String>,
}

impl fmt::Display for HostFsRawError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for HostFsRawError {}

// Original: toHostFsError(). An existing HostFsError is returned without
// changing its code, message, details, or cause.
pub fn to_host_fs_error(
    error: Box<dyn Error + Send + Sync>,
    path: &str,
    operation: &str,
) -> HostFsError {
    let error = match error.downcast::<HostFsError>() {
        Ok(error) => return *error,
        Err(error) => error,
    };
    let (errno, syscall, code) = classify_error(error.as_ref());
    let reason = match code {
        OS_FS_NOT_FOUND => "path does not exist",
        OS_FS_IS_DIRECTORY => "path is a directory",
        OS_FS_NOT_DIRECTORY => "a path component is not a directory",
        OS_FS_ALREADY_EXISTS => "path already exists",
        OS_FS_PERMISSION_DENIED => "permission denied",
        OS_FS_NOT_EMPTY => "directory is not empty",
        OS_FS_UNAVAILABLE => "filesystem resource unavailable",
        _ => "unrecognized filesystem error",
    };
    let details = Map::from_iter([
        ("path".into(), Value::String(path.into())),
        ("op".into(), Value::String(operation.into())),
        ("errno".into(), errno.map_or(Value::Null, Value::String)),
        ("syscall".into(), syscall.map_or(Value::Null, Value::String)),
    ]);
    let cause: Arc<dyn Error + Send + Sync> = Arc::from(error);
    HostFsError::with_options(
        code,
        format!("{operation} failed: {reason}"),
        Error2Options {
            details: Some(details),
            cause: Some(ErrorCause::Error(cause)),
            ..Error2Options::default()
        },
    )
}

fn classify_error(error: &(dyn Error + 'static)) -> (Option<String>, Option<String>, &'static str) {
    if let Some(raw) = error.downcast_ref::<HostFsRawError>() {
        return (
            raw.errno.clone(),
            raw.syscall.clone(),
            map_errno(raw.errno.as_deref()),
        );
    }
    if let Some(error) = error.downcast_ref::<io::Error>() {
        let errno = symbolic_io_kind(error.kind()).map(str::to_owned);
        return (errno.clone(), None, map_errno(errno.as_deref()));
    }
    (None, None, OS_FS_UNKNOWN)
}

fn symbolic_io_kind(kind: io::ErrorKind) -> Option<&'static str> {
    match kind {
        io::ErrorKind::NotFound => Some("ENOENT"),
        io::ErrorKind::IsADirectory => Some("EISDIR"),
        io::ErrorKind::NotADirectory => Some("ENOTDIR"),
        io::ErrorKind::AlreadyExists => Some("EEXIST"),
        io::ErrorKind::PermissionDenied => Some("EACCES"),
        io::ErrorKind::DirectoryNotEmpty => Some("ENOTEMPTY"),
        _ => None,
    }
}

fn map_errno(errno: Option<&str>) -> &'static str {
    match errno {
        Some("ENOENT") => OS_FS_NOT_FOUND,
        Some("EISDIR") => OS_FS_IS_DIRECTORY,
        Some("ENOTDIR") => OS_FS_NOT_DIRECTORY,
        Some("EEXIST") => OS_FS_ALREADY_EXISTS,
        Some("EACCES" | "EPERM") => OS_FS_PERMISSION_DENIED,
        Some("ENOTEMPTY") => OS_FS_NOT_EMPTY,
        _ => OS_FS_UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::error_info;

    fn raw(errno: Option<&str>, syscall: Option<&str>) -> Box<dyn Error + Send + Sync> {
        Box::new(HostFsRawError {
            message: "mock failure".into(),
            errno: errno.map(str::to_owned),
            syscall: syscall.map(str::to_owned),
        })
    }

    #[test]
    fn maps_every_original_errno_and_unknown_fallback() {
        for (errno, expected) in [
            ("ENOENT", OS_FS_NOT_FOUND),
            ("EISDIR", OS_FS_IS_DIRECTORY),
            ("ENOTDIR", OS_FS_NOT_DIRECTORY),
            ("EEXIST", OS_FS_ALREADY_EXISTS),
            ("EACCES", OS_FS_PERMISSION_DENIED),
            ("EPERM", OS_FS_PERMISSION_DENIED),
            ("ENOTEMPTY", OS_FS_NOT_EMPTY),
            ("EIO", OS_FS_UNKNOWN),
        ] {
            assert_eq!(
                to_host_fs_error(raw(Some(errno), Some("open")), "/x/y.txt", "read").code(),
                expected
            );
        }
        assert_eq!(
            to_host_fs_error(raw(None, None), "/x", "read").code(),
            OS_FS_UNKNOWN
        );
    }

    #[test]
    fn preserves_details_cause_message_redaction_and_idempotence() {
        let error = to_host_fs_error(raw(Some("EACCES"), Some("stat")), "/secret", "stat");
        assert_eq!(
            error.error().details.as_ref().unwrap(),
            &Map::from_iter([
                ("path".into(), Value::String("/secret".into())),
                ("op".into(), Value::String("stat".into())),
                ("errno".into(), Value::String("EACCES".into())),
                ("syscall".into(), Value::String("stat".into())),
            ])
        );
        assert!(!error.to_string().contains("/secret"));
        assert!(!error.to_string().contains("EACCES"));
        assert!(error.source().is_some());

        let translated = to_host_fs_error(Box::new(error), "/other", "write");
        assert_eq!(translated.code(), OS_FS_PERMISSION_DENIED);
        assert_eq!(
            translated.error().details.as_ref().unwrap()["path"],
            "/secret"
        );
    }

    #[test]
    fn registers_retryability_and_permission_action() {
        ensure_os_fs_errors_registered();
        assert!(error_info(OS_FS_UNAVAILABLE).retryable);
        assert!(error_info(OS_FS_UNKNOWN).retryable);
        assert_eq!(
            error_info(OS_FS_PERMISSION_DENIED).action.as_deref(),
            Some("Check the file permissions of the target path.")
        );
    }
}
