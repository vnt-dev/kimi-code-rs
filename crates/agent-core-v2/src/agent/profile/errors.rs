//! Agent-profile configuration error codes.
//!
//! Original: `packages/agent-core-v2/src/agent/profile/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, register_error_domain};

pub const MODEL_NOT_CONFIGURED: &str = "model.not_configured";
pub const MODEL_CONFIG_INVALID: &str = "model.config_invalid";
pub const THINKING_ALIAS_CONFLICT: &str = "profile.thinking_alias_conflict";
pub const PROFILE_UNKNOWN: &str = "profile.unknown";
pub const PROFILE_ALREADY_BOUND: &str = "profile.already_bound";
pub const PROFILE_NOT_BOUND: &str = "profile.not_bound";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileErrorCode {
    ModelNotConfigured,
    ModelConfigInvalid,
    ThinkingAliasConflict,
    ProfileUnknown,
    ProfileAlreadyBound,
    ProfileNotBound,
}

impl ProfileErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModelNotConfigured => MODEL_NOT_CONFIGURED,
            Self::ModelConfigInvalid => MODEL_CONFIG_INVALID,
            Self::ThinkingAliasConflict => THINKING_ALIAS_CONFLICT,
            Self::ProfileUnknown => PROFILE_UNKNOWN,
            Self::ProfileAlreadyBound => PROFILE_ALREADY_BOUND,
            Self::ProfileNotBound => PROFILE_NOT_BOUND,
        }
    }
}

pub static PROFILE_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("MODEL_NOT_CONFIGURED", MODEL_NOT_CONFIGURED),
        ("MODEL_CONFIG_INVALID", MODEL_CONFIG_INVALID),
        ("THINKING_ALIAS_CONFLICT", THINKING_ALIAS_CONFLICT),
        ("PROFILE_UNKNOWN", PROFILE_UNKNOWN),
        ("PROFILE_ALREADY_BOUND", PROFILE_ALREADY_BOUND),
        ("PROFILE_NOT_BOUND", PROFILE_NOT_BOUND),
    ],
    retryable: &[],
    info: &[],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&PROFILE_ERRORS).expect("agent-profile error codes are unique");
});

// Original: errors.ts, registerErrorDomain(ProfileErrors).
// Rust uses explicit lazy initialization rather than module-load side effects.
pub fn ensure_profile_errors_registered() {
    LazyLock::force(&REGISTERED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn registers_every_non_retryable_profile_error_code() {
        ensure_profile_errors_registered();
        for code in [
            MODEL_NOT_CONFIGURED,
            MODEL_CONFIG_INVALID,
            THINKING_ALIAS_CONFLICT,
            PROFILE_UNKNOWN,
            PROFILE_ALREADY_BOUND,
            PROFILE_NOT_BOUND,
        ] {
            assert!(is_error_code(code), "{code}");
            assert!(!error_info(code).retryable, "{code}");
        }
    }
}
