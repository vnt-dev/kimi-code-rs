//! Structured wire-domain error codes and base error type.
//!
//! Original: `packages/agent-core-v2/src/wire/errors.ts`.

use std::{error::Error, fmt, sync::LazyLock};

use crate::_base::errors::{
    codes::{ErrorDomain, ErrorInfo, register_error_domain},
    errors::{Error2, Error2Options},
};

pub const WIRE_DUPLICATE_OP: &str = "wire.duplicate_op";
pub const WIRE_CYCLE: &str = "wire.cycle";
pub const WIRE_UNKNOWN_RECORD: &str = "wire.unknown_record";
pub const RECORDS_WRITE_FAILED: &str = "records.write_failed";

pub static WIRE_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("WIRE_DUPLICATE_OP", WIRE_DUPLICATE_OP),
        ("WIRE_CYCLE", WIRE_CYCLE),
        ("WIRE_UNKNOWN_RECORD", WIRE_UNKNOWN_RECORD),
        ("RECORDS_WRITE_FAILED", RECORDS_WRITE_FAILED),
    ],
    retryable: &[],
    info: &[
        (
            WIRE_DUPLICATE_OP,
            ErrorInfo {
                title: "Duplicate wire op type",
                retryable: false,
                public: true,
                action: Some(
                    "Two ops registered the same type; rename one. This is a build-time bug.",
                ),
            },
        ),
        (
            WIRE_CYCLE,
            ErrorInfo {
                title: "Wire dispatch cycle",
                retryable: false,
                public: true,
                action: Some("An onChange handler re-dispatches endlessly; break the op cycle."),
            },
        ),
        (
            WIRE_UNKNOWN_RECORD,
            ErrorInfo {
                title: "Unknown wire record",
                retryable: false,
                public: true,
                action: Some("The record was written by a newer version; upgrade or drop it."),
            },
        ),
        (
            RECORDS_WRITE_FAILED,
            ErrorInfo {
                title: "Wire journal write failed",
                retryable: false,
                public: true,
                action: None,
            },
        ),
    ],
};

static WIRE_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&WIRE_ERRORS).expect("wire error codes are unique");
});

pub fn ensure_wire_errors_registered() {
    LazyLock::force(&WIRE_ERRORS_REGISTERED);
}

#[derive(Debug)]
pub struct WireError {
    inner: Error2,
}

impl WireError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_options(code, message, Error2Options::default())
    }

    pub fn with_options(
        code: &'static str,
        message: impl Into<String>,
        mut options: Error2Options,
    ) -> Self {
        ensure_wire_errors_registered();
        options.name.get_or_insert_with(|| "WireError".into());
        Self {
            inner: Error2::with_options(code, message, options),
        }
    }

    pub fn error(&self) -> &Error2 {
        &self.inner
    }

    pub fn code(&self) -> &str {
        &self.inner.code
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for WireError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn registers_complete_wire_error_domain() {
        ensure_wire_errors_registered();
        for code in [
            WIRE_DUPLICATE_OP,
            WIRE_CYCLE,
            WIRE_UNKNOWN_RECORD,
            RECORDS_WRITE_FAILED,
        ] {
            assert!(is_error_code(code), "{code}");
            assert!(!error_info(code).retryable);
        }
        assert_eq!(
            error_info(WIRE_UNKNOWN_RECORD).action.as_deref(),
            Some("The record was written by a newer version; upgrade or drop it.")
        );
    }

    #[test]
    fn wire_error_preserves_code_message_name_and_details() {
        let error = WireError::with_options(
            WIRE_UNKNOWN_RECORD,
            "Unknown wire record: future.op",
            Error2Options {
                details: Some(Map::from_iter([(
                    "type".into(),
                    Value::String("future.op".into()),
                )])),
                ..Error2Options::default()
            },
        );
        assert_eq!(error.code(), WIRE_UNKNOWN_RECORD);
        assert_eq!(error.to_string(), "Unknown wire record: future.op");
        assert_eq!(error.error().name, "WireError");
        assert_eq!(error.error().details.as_ref().unwrap()["type"], "future.op");
    }
}
