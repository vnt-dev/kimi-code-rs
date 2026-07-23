//! Configuration domain error codes.
//!
//! Original: `packages/agent-core-v2/src/app/config/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const CONFIG_INVALID: &str = "config.invalid";

pub static CONFIG_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[("CONFIG_INVALID", CONFIG_INVALID)],
    retryable: &[],
    info: &[],
};

static CONFIG_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&CONFIG_ERRORS).expect("config error codes must remain unique");
});

pub fn ensure_config_errors_registered() {
    LazyLock::force(&CONFIG_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::{error_info, is_error_code};

    use super::*;

    #[test]
    fn registers_non_retryable_invalid_config_code() {
        ensure_config_errors_registered();
        assert!(is_error_code(CONFIG_INVALID));
        assert!(!error_info(CONFIG_INVALID).retryable);
    }
}
