//! Stable session-filesystem error codes.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const FS_PATH_NOT_FOUND: &str = "fs.path_not_found";
pub const FS_PERMISSION_DENIED: &str = "fs.permission_denied";
pub const FS_PATH_ESCAPES: &str = "fs.path_escapes";
pub const FS_IS_DIRECTORY: &str = "fs.is_directory";
pub const FS_IS_BINARY: &str = "fs.is_binary";
pub const FS_TOO_LARGE: &str = "fs.too_large";
pub const FS_ALREADY_EXISTS: &str = "fs.already_exists";
pub const FS_TOO_MANY_RESULTS: &str = "fs.too_many_results";
pub const FS_GREP_TIMEOUT: &str = "fs.grep_timeout";
pub const FS_GIT_UNAVAILABLE: &str = "fs.git_unavailable";

pub static FS_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("FS_PATH_NOT_FOUND", FS_PATH_NOT_FOUND),
        ("FS_PERMISSION_DENIED", FS_PERMISSION_DENIED),
        ("FS_PATH_ESCAPES", FS_PATH_ESCAPES),
        ("FS_IS_DIRECTORY", FS_IS_DIRECTORY),
        ("FS_IS_BINARY", FS_IS_BINARY),
        ("FS_TOO_LARGE", FS_TOO_LARGE),
        ("FS_ALREADY_EXISTS", FS_ALREADY_EXISTS),
        ("FS_TOO_MANY_RESULTS", FS_TOO_MANY_RESULTS),
        ("FS_GREP_TIMEOUT", FS_GREP_TIMEOUT),
        ("FS_GIT_UNAVAILABLE", FS_GIT_UNAVAILABLE),
    ],
    retryable: &[],
    info: &[],
};

static FS_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&FS_ERRORS).expect("session filesystem error codes are unique");
});

// Rust modules have no import-time side effects. Composition roots and error
// constructors call this explicit counterpart to the source registration.
pub fn ensure_fs_errors_registered() {
    LazyLock::force(&FS_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::{error_info, is_error_code};

    use super::*;

    #[test]
    fn registers_every_original_code_without_retry_metadata() {
        ensure_fs_errors_registered();

        for (_, code) in FS_ERRORS.codes {
            assert!(is_error_code(code), "missing error code {code}");
            assert!(!error_info(code).retryable);
            assert_eq!(error_info(code).title, *code);
        }
        assert_eq!(FS_ERRORS.codes.len(), 10);
    }
}
