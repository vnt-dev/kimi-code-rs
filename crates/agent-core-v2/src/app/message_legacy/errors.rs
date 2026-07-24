//! Original: `packages/agent-core-v2/src/app/messageLegacy/errors.ts`.
use crate::_base::errors::codes::{ErrorDomain, register_error_domain};
use std::sync::LazyLock;
pub const MESSAGE_NOT_FOUND: &str = "message.not_found";
pub static MESSAGE_LEGACY_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[("MESSAGE_NOT_FOUND", MESSAGE_NOT_FOUND)],
    retryable: &[],
    info: &[],
};
static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&MESSAGE_LEGACY_ERRORS).expect("message legacy errors must be unique")
});
pub fn ensure_message_legacy_errors_registered() {
    LazyLock::force(&REGISTERED);
}
