use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorInfo {
    pub title: &'static str,
    pub retryable: bool,
    pub public: bool,
    pub action: Option<&'static str>,
}

#[derive(Debug)]
pub struct ErrorDomain {
    pub codes: &'static [(&'static str, &'static str)],
    pub retryable: &'static [&'static str],
    pub info: &'static [(&'static str, ErrorInfo)],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedErrorInfo {
    pub title: String,
    pub retryable: bool,
    pub public: bool,
    pub action: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DomainOwner {
    pointer: usize,
    length: usize,
}

impl DomainOwner {
    fn of(domain: &'static ErrorDomain) -> Self {
        Self {
            pointer: domain.codes.as_ptr() as usize,
            length: domain.codes.len(),
        }
    }
}

#[derive(Default)]
struct ErrorRegistry {
    registered_codes: HashMap<&'static str, DomainOwner>,
    retryable_codes: HashSet<&'static str>,
    info_overrides: HashMap<&'static str, ErrorInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDomainRegistrationError {
    pub code: &'static str,
}

impl fmt::Display for ErrorDomainRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "error code '{}' is registered by two different domains",
            self.code
        )
    }
}

impl Error for ErrorDomainRegistrationError {}

pub const CORE_INTERNAL: &str = "internal";
pub const CORE_NOT_IMPLEMENTED: &str = "not_implemented";
pub const CORE_VALIDATION_FAILED: &str = "validation.failed";

pub static CORE_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("INTERNAL", CORE_INTERNAL),
        ("NOT_IMPLEMENTED", CORE_NOT_IMPLEMENTED),
        ("VALIDATION_FAILED", CORE_VALIDATION_FAILED),
    ],
    retryable: &[],
    info: &[
        (
            CORE_INTERNAL,
            ErrorInfo {
                title: "Internal error",
                retryable: false,
                public: true,
                action: Some("Inspect logs or report the issue with diagnostics."),
            },
        ),
        (
            CORE_NOT_IMPLEMENTED,
            ErrorInfo {
                title: "Not implemented",
                retryable: false,
                public: true,
                action: Some("This feature is not implemented yet."),
            },
        ),
    ],
};

static ERROR_REGISTRY: LazyLock<RwLock<ErrorRegistry>> = LazyLock::new(|| {
    let mut registry = ErrorRegistry::default();
    register_inner(&mut registry, &CORE_ERRORS).expect("core error codes are unique");
    RwLock::new(registry)
});

// Original:
//   packages/agent-core-v2/src/_base/errors/codes.ts
//   registerErrorDomain()
pub fn register_error_domain(
    domain: &'static ErrorDomain,
) -> Result<(), ErrorDomainRegistrationError> {
    register_inner(&mut ERROR_REGISTRY.write(), domain)
}

fn register_inner(
    registry: &mut ErrorRegistry,
    domain: &'static ErrorDomain,
) -> Result<(), ErrorDomainRegistrationError> {
    let owner = DomainOwner::of(domain);
    for (_, code) in domain.codes {
        if let Some(existing_owner) = registry.registered_codes.get(code)
            && *existing_owner != owner
        {
            return Err(ErrorDomainRegistrationError { code });
        }
    }
    for (_, code) in domain.codes {
        registry.registered_codes.insert(code, owner);
    }
    registry
        .retryable_codes
        .extend(domain.retryable.iter().copied());
    registry.info_overrides.extend(domain.info.iter().copied());
    Ok(())
}

// Original: codes.ts, isErrorCode()
pub fn is_error_code(code: &str) -> bool {
    ERROR_REGISTRY.read().registered_codes.contains_key(code)
}

// Original: codes.ts, errorInfo()
pub fn error_info(code: &str) -> ResolvedErrorInfo {
    let registry = ERROR_REGISTRY.read();
    if let Some(info) = registry.info_overrides.get(code) {
        return ResolvedErrorInfo {
            title: info.title.to_owned(),
            retryable: info.retryable,
            public: info.public,
            action: info.action.map(str::to_owned),
        };
    }
    ResolvedErrorInfo {
        title: code.to_owned(),
        retryable: registry.retryable_codes.contains(code),
        public: true,
        action: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_DOMAIN: ErrorDomain = ErrorDomain {
        codes: &[("RETRY", "test.codes.retry"), ("PLAIN", "test.codes.plain")],
        retryable: &["test.codes.retry"],
        info: &[(
            "test.codes.plain",
            ErrorInfo {
                title: "Plain test failure",
                retryable: false,
                public: false,
                action: Some("Fix the fixture."),
            },
        )],
    };
    static CONFLICTING_DOMAIN: ErrorDomain = ErrorDomain {
        codes: &[("CONFLICT", "test.codes.plain")],
        retryable: &[],
        info: &[],
    };

    #[test]
    fn core_domain_is_registered_on_first_access() {
        assert!(is_error_code(CORE_INTERNAL));
        assert!(is_error_code(CORE_NOT_IMPLEMENTED));
        assert!(is_error_code(CORE_VALIDATION_FAILED));
        assert_eq!(error_info(CORE_INTERNAL).title, "Internal error");
    }

    #[test]
    fn domain_registration_is_idempotent_for_the_same_codes_object() {
        register_error_domain(&TEST_DOMAIN).unwrap();
        register_error_domain(&TEST_DOMAIN).unwrap();
        assert!(is_error_code("test.codes.retry"));
        assert!(error_info("test.codes.retry").retryable);
        assert_eq!(error_info("test.codes.plain").title, "Plain test failure");
        assert!(!error_info("test.codes.plain").public);
    }

    #[test]
    fn different_domains_cannot_claim_the_same_code() {
        register_error_domain(&TEST_DOMAIN).unwrap();
        let error = register_error_domain(&CONFLICTING_DOMAIN).unwrap_err();
        assert_eq!(
            error.to_string(),
            "error code 'test.codes.plain' is registered by two different domains"
        );
    }

    #[test]
    fn unknown_code_uses_public_nonretryable_fallback() {
        let info = error_info("test.codes.unknown");
        assert_eq!(info.title, "test.codes.unknown");
        assert!(!info.retryable);
        assert!(info.public);
        assert_eq!(info.action, None);
    }
}
