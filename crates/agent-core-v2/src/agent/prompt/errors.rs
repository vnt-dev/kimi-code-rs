//! Prompt domain error codes.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const REQUEST_INVALID: &str = "request.invalid";
pub const PROMPT_NOT_FOUND: &str = "prompt.not_found";
pub const SESSION_UNDO_UNAVAILABLE: &str = "session.undo_unavailable";

pub static PROMPT_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("REQUEST_INVALID", REQUEST_INVALID),
        ("PROMPT_NOT_FOUND", PROMPT_NOT_FOUND),
        ("SESSION_UNDO_UNAVAILABLE", SESSION_UNDO_UNAVAILABLE),
    ],
    retryable: &[],
    info: &[],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&PROMPT_ERRORS).expect("prompt error codes are unique");
});

pub fn ensure_prompt_errors_registered() {
    LazyLock::force(&REGISTERED);
}
