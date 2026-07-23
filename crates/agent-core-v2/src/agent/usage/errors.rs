use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const USAGE_TURN_ID_CONFLICT: &str = "usage.turn_id_conflict";

pub static USAGE_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[("TURN_ID_CONFLICT", USAGE_TURN_ID_CONFLICT)],
    retryable: &[],
    info: &[],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&USAGE_ERRORS).expect("usage error codes are unique");
});

// Original:
//   packages/agent-core-v2/src/agent/usage/errors.ts
//   registerErrorDomain(UsageErrors)
pub fn ensure_usage_errors_registered() {
    LazyLock::force(&REGISTERED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn registers_non_retryable_turn_conflict_code() {
        ensure_usage_errors_registered();
        assert!(is_error_code(USAGE_TURN_ID_CONFLICT));
        assert!(!error_info(USAGE_TURN_ID_CONFLICT).retryable);
    }
}
