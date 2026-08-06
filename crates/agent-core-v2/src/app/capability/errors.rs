//! Capability domain error codes.
//!
//! Original: `packages/agent-core-v2/src/app/capability/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const CAPABILITY_NOT_FOUND: &str = "capability.not_found";
pub const CAPABILITY_UNSUPPORTED: &str = "capability.unsupported";
pub const CAPABILITY_INSTALL_IN_PROGRESS: &str = "capability.install_in_progress";

pub static CAPABILITY_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("CAPABILITY_NOT_FOUND", CAPABILITY_NOT_FOUND),
        ("CAPABILITY_UNSUPPORTED", CAPABILITY_UNSUPPORTED),
        (
            "CAPABILITY_INSTALL_IN_PROGRESS",
            CAPABILITY_INSTALL_IN_PROGRESS,
        ),
    ],
    retryable: &[],
    info: &[],
};

static CAPABILITY_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&CAPABILITY_ERRORS).expect("capability error codes are unique");
});

pub fn ensure_capability_errors_registered() {
    LazyLock::force(&CAPABILITY_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::{error_info, is_error_code};

    use super::*;

    #[test]
    fn registers_original_non_retryable_codes() {
        ensure_capability_errors_registered();
        assert!(is_error_code(CAPABILITY_NOT_FOUND));
        assert!(is_error_code(CAPABILITY_UNSUPPORTED));
        assert!(is_error_code(CAPABILITY_INSTALL_IN_PROGRESS));
        assert!(!error_info(CAPABILITY_NOT_FOUND).retryable);
        assert!(!error_info(CAPABILITY_UNSUPPORTED).retryable);
        assert!(!error_info(CAPABILITY_INSTALL_IN_PROGRESS).retryable);
    }
}
