//! Authentication domain error codes.
//!
//! Original: `packages/agent-core-v2/src/app/auth/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, ErrorInfo, register_error_domain};

pub const AUTH_LOGIN_REQUIRED: &str = "auth.login_required";
pub const AUTH_PROVISIONING_REQUIRED: &str = "auth.provisioning_required";
pub const AUTH_TOKEN_MISSING: &str = "auth.token_missing";
pub const AUTH_TOKEN_UNAUTHORIZED: &str = "auth.token_unauthorized";
pub const AUTH_MODEL_NOT_RESOLVED: &str = "auth.model_not_resolved";

pub static AUTH_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("AUTH_LOGIN_REQUIRED", AUTH_LOGIN_REQUIRED),
        ("AUTH_PROVISIONING_REQUIRED", AUTH_PROVISIONING_REQUIRED),
        ("AUTH_TOKEN_MISSING", AUTH_TOKEN_MISSING),
        ("AUTH_TOKEN_UNAUTHORIZED", AUTH_TOKEN_UNAUTHORIZED),
        ("AUTH_MODEL_NOT_RESOLVED", AUTH_MODEL_NOT_RESOLVED),
    ],
    retryable: &[],
    info: &[
        (
            AUTH_LOGIN_REQUIRED,
            ErrorInfo {
                title: "Login required",
                retryable: false,
                public: true,
                action: Some("Run /login to authenticate with the OAuth provider."),
            },
        ),
        (
            AUTH_PROVISIONING_REQUIRED,
            ErrorInfo {
                title: "Provider provisioning required",
                retryable: false,
                public: true,
                action: Some("Configure a provider via /login or the providers endpoint."),
            },
        ),
        (
            AUTH_TOKEN_MISSING,
            ErrorInfo {
                title: "Provider credential missing",
                retryable: false,
                public: true,
                action: Some("Configure an API key or complete OAuth login for the provider."),
            },
        ),
        (
            AUTH_TOKEN_UNAUTHORIZED,
            ErrorInfo {
                title: "Provider credential unauthorized",
                retryable: false,
                public: true,
                action: Some("Re-authenticate with the OAuth provider."),
            },
        ),
        (
            AUTH_MODEL_NOT_RESOLVED,
            ErrorInfo {
                title: "Model not resolved",
                retryable: false,
                public: true,
                action: Some("Set a default model or configure the requested model alias."),
            },
        ),
    ],
};

static AUTH_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&AUTH_ERRORS).expect("auth error codes must remain unique");
});

pub fn ensure_auth_errors_registered() {
    LazyLock::force(&AUTH_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::{error_info, is_error_code};

    use super::*;

    #[test]
    fn registers_all_public_non_retryable_auth_errors_and_actions() {
        ensure_auth_errors_registered();
        for code in [
            AUTH_LOGIN_REQUIRED,
            AUTH_PROVISIONING_REQUIRED,
            AUTH_TOKEN_MISSING,
            AUTH_TOKEN_UNAUTHORIZED,
            AUTH_MODEL_NOT_RESOLVED,
        ] {
            assert!(is_error_code(code), "{code}");
            let info = error_info(code);
            assert!(!info.retryable, "{code}");
            assert!(info.public, "{code}");
            assert!(info.action.is_some(), "{code}");
        }
        assert_eq!(error_info(AUTH_LOGIN_REQUIRED).title, "Login required");
    }
}
