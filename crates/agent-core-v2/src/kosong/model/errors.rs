//! Model-catalog error codes.
//!
//! Original: `packages/agent-core-v2/src/kosong/model/errors.ts`.

use std::sync::LazyLock;

use crate::_base::errors::codes::{ErrorDomain, ErrorInfo, register_error_domain};

pub const PROVIDER_NOT_FOUND: &str = "provider.not_found";
pub const MODEL_NOT_FOUND: &str = "model.not_found";

pub static MODEL_CATALOG_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("PROVIDER_NOT_FOUND", PROVIDER_NOT_FOUND),
        ("MODEL_NOT_FOUND", MODEL_NOT_FOUND),
    ],
    retryable: &[],
    info: &[
        (
            PROVIDER_NOT_FOUND,
            ErrorInfo {
                title: "Provider not found",
                retryable: false,
                public: true,
                action: Some("Check the provider id or configure the provider first."),
            },
        ),
        (
            MODEL_NOT_FOUND,
            ErrorInfo {
                title: "Model not found",
                retryable: false,
                public: true,
                action: Some("Check the model alias or configure the model first."),
            },
        ),
    ],
};

static MODEL_CATALOG_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    // Rust has no module-load side effects. Callers that construct or expose a
    // model catalog call `ensure_model_catalog_errors_registered` first.
    register_error_domain(&MODEL_CATALOG_ERRORS)
        .expect("model catalog error codes must remain unique");
});

// Original: module-load `registerErrorDomain(ModelCatalogErrors)`.
pub fn ensure_model_catalog_errors_registered() {
    LazyLock::force(&MODEL_CATALOG_ERRORS_REGISTERED);
}

#[cfg(test)]
mod tests {
    use crate::_base::errors::codes::{error_info, is_error_code};

    use super::*;

    #[test]
    fn registers_the_public_non_retryable_model_catalog_errors() {
        ensure_model_catalog_errors_registered();

        for code in [PROVIDER_NOT_FOUND, MODEL_NOT_FOUND] {
            assert!(is_error_code(code), "{code}");
            let info = error_info(code);
            assert!(!info.retryable, "{code}");
            assert!(info.public, "{code}");
            assert!(info.action.is_some(), "{code}");
        }

        assert_eq!(error_info(PROVIDER_NOT_FOUND).title, "Provider not found");
        assert_eq!(error_info(MODEL_NOT_FOUND).title, "Model not found");
    }
}
