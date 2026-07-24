//! Agent lifecycle error codes.
//!
//! Original: `session/agentLifecycle/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const AGENT_NOT_FOUND: &str = "agent.not_found";

pub static AGENT_LIFECYCLE_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[("AGENT_NOT_FOUND", AGENT_NOT_FOUND)],
    retryable: &[],
    info: &[],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&AGENT_LIFECYCLE_ERRORS).expect("agent lifecycle error codes are unique");
});

pub fn ensure_agent_lifecycle_errors_registered() {
    LazyLock::force(&REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::is_error_code;

    use super::*;

    #[test]
    fn registers_the_source_agent_not_found_code() {
        ensure_agent_lifecycle_errors_registered();
        assert!(is_error_code(AGENT_NOT_FOUND));
    }
}
