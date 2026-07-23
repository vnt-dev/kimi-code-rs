//! Workspace registry error codes.
//!
//! Original: `packages/agent-core-v2/src/app/workspaceRegistry/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const WORKSPACE_NOT_FOUND: &str = "workspace.not_found";

pub static WORKSPACE_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[("WORKSPACE_NOT_FOUND", WORKSPACE_NOT_FOUND)],
    retryable: &[],
    info: &[],
};

static WORKSPACE_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&WORKSPACE_ERRORS).expect("workspace error codes are unique");
});

pub fn ensure_workspace_errors_registered() {
    LazyLock::force(&WORKSPACE_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::{error_info, is_error_code};

    use super::*;

    #[test]
    fn registers_original_non_retryable_code() {
        ensure_workspace_errors_registered();
        assert!(is_error_code(WORKSPACE_NOT_FOUND));
        assert!(!error_info(WORKSPACE_NOT_FOUND).retryable);
    }
}
