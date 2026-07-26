//! `/init` error code registration.
//!
//! Original: `ErrorCodes.SESSION_INIT_FAILED`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const SESSION_INIT_FAILED: &str = "session.init_failed";

pub static SESSION_INIT_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[("SESSION_INIT_FAILED", SESSION_INIT_FAILED)],
    retryable: &[],
    info: &[],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&SESSION_INIT_ERRORS).expect("session init error codes are unique");
});

pub fn ensure_session_init_errors_registered() {
    LazyLock::force(&REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::is_error_code;

    use super::*;

    #[test]
    fn registers_the_source_error_code() {
        ensure_session_init_errors_registered();
        assert!(is_error_code(SESSION_INIT_FAILED));
    }
}
