//! Skill catalog domain error codes.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const SKILL_NOT_FOUND: &str = "skill.not_found";
pub const SKILL_TYPE_UNSUPPORTED: &str = "skill.type_unsupported";
pub const SKILL_NAME_EMPTY: &str = "skill.name_empty";

pub static SKILL_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("SKILL_NOT_FOUND", SKILL_NOT_FOUND),
        ("SKILL_TYPE_UNSUPPORTED", SKILL_TYPE_UNSUPPORTED),
        ("SKILL_NAME_EMPTY", SKILL_NAME_EMPTY),
    ],
    retryable: &[],
    info: &[],
};

static SKILL_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&SKILL_ERRORS).expect("skill error codes are unique");
});

pub fn ensure_skill_errors_registered() {
    LazyLock::force(&SKILL_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn error_codes_register_without_retryable_overrides() {
        ensure_skill_errors_registered();
        assert!(is_error_code(SKILL_NOT_FOUND));
        assert!(is_error_code(SKILL_TYPE_UNSUPPORTED));
        assert!(is_error_code(SKILL_NAME_EMPTY));
        assert!(!error_info(SKILL_NOT_FOUND).retryable);
        assert!(!error_info(SKILL_TYPE_UNSUPPORTED).retryable);
        assert!(!error_info(SKILL_NAME_EMPTY).retryable);
    }
}
