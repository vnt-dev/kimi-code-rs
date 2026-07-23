use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const COMPACTION_FAILED: &str = "compaction.failed";
pub const COMPACTION_UNABLE: &str = "compaction.unable";

pub static FULL_COMPACTION_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("COMPACTION_FAILED", COMPACTION_FAILED),
        ("COMPACTION_UNABLE", COMPACTION_UNABLE),
    ],
    retryable: &[],
    info: &[],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&FULL_COMPACTION_ERRORS).expect("full compaction error codes are unique");
});

// Original: fullCompaction/errors.ts, registerErrorDomain().
pub fn ensure_full_compaction_errors_registered() {
    LazyLock::force(&REGISTERED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn registers_complete_non_retryable_error_domain() {
        ensure_full_compaction_errors_registered();
        for code in [COMPACTION_FAILED, COMPACTION_UNABLE] {
            assert!(is_error_code(code));
            assert!(!error_info(code).retryable);
        }
    }
}
