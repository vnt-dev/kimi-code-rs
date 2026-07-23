//! Plugin domain error codes.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const PLUGIN_NOT_FOUND: &str = "plugin.not_found";
pub const PLUGIN_LOAD_FAILED: &str = "plugin.load_failed";

pub static PLUGIN_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("PLUGIN_NOT_FOUND", PLUGIN_NOT_FOUND),
        ("PLUGIN_LOAD_FAILED", PLUGIN_LOAD_FAILED),
    ],
    retryable: &[],
    info: &[],
};

static PLUGIN_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&PLUGIN_ERRORS).expect("plugin error codes are unique");
});

pub fn ensure_plugin_errors_registered() {
    LazyLock::force(&PLUGIN_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::{error_info, is_error_code};

    use super::*;

    #[test]
    fn registers_original_non_retryable_codes() {
        ensure_plugin_errors_registered();
        assert!(is_error_code(PLUGIN_NOT_FOUND));
        assert!(is_error_code(PLUGIN_LOAD_FAILED));
        assert!(!error_info(PLUGIN_NOT_FOUND).retryable);
        assert!(!error_info(PLUGIN_LOAD_FAILED).retryable);
    }
}
