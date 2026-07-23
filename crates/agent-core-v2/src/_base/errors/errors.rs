use serde_json::{Map, Value};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use super::codes::CORE_NOT_IMPLEMENTED;

#[derive(Debug)]
pub struct ExpectedError {
    message: String,
}

impl ExpectedError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn is_expected(&self) -> bool {
        true
    }
}

impl fmt::Display for ExpectedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ExpectedError {}

#[derive(Debug)]
pub struct ErrorNoTelemetry {
    message: String,
}

impl ErrorNoTelemetry {
    pub const NAME: &'static str = "CodeExpectedError";

    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn from_error(error: &(dyn Error + 'static)) -> Self {
        Self::new(error.to_string())
    }

    pub fn is_error_no_telemetry(error: &(dyn Error + 'static)) -> bool {
        error.downcast_ref::<Self>().is_some()
    }
}

impl fmt::Display for ErrorNoTelemetry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ErrorNoTelemetry {}

#[derive(Debug)]
pub struct BugIndicatingError {
    message: String,
}

impl BugIndicatingError {
    pub fn new(message: Option<&str>) -> Self {
        Self {
            message: message.unwrap_or("An unexpected bug occurred.").to_owned(),
        }
    }
}

impl fmt::Display for BugIndicatingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for BugIndicatingError {}

#[derive(Clone)]
pub enum ErrorCause {
    Error(Arc<dyn Error + Send + Sync>),
    Value(Value),
}

impl fmt::Debug for ErrorCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(error) => formatter.debug_tuple("Error").field(error).finish(),
            Self::Value(value) => formatter.debug_tuple("Value").field(value).finish(),
        }
    }
}

impl From<Arc<dyn Error + Send + Sync>> for ErrorCause {
    fn from(error: Arc<dyn Error + Send + Sync>) -> Self {
        Self::Error(error)
    }
}

impl From<Value> for ErrorCause {
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}

#[derive(Default)]
pub struct Error2Options {
    pub details: Option<Map<String, Value>>,
    pub cause: Option<ErrorCause>,
    pub name: Option<String>,
}

#[derive(Clone)]
pub struct Error2 {
    pub name: String,
    pub code: String,
    pub message: String,
    pub details: Option<Map<String, Value>>,
    cause: Option<ErrorCause>,
}

impl Error2 {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::with_options(code, message, Error2Options::default())
    }

    pub fn with_options(
        code: impl Into<String>,
        message: impl Into<String>,
        options: Error2Options,
    ) -> Self {
        Self {
            name: options.name.unwrap_or_else(|| "Error2".to_owned()),
            code: code.into(),
            message: message.into(),
            details: options.details,
            cause: options.cause,
        }
    }

    pub fn cause(&self) -> Option<&ErrorCause> {
        self.cause.as_ref()
    }
}

impl fmt::Debug for Error2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct(&self.name)
            .field("code", &self.code)
            .field("message", &self.message)
            .field("details", &self.details)
            .field("cause", &self.cause)
            .finish()
    }
}

impl fmt::Display for Error2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for Error2 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.cause.as_ref() {
            Some(ErrorCause::Error(cause)) => Some(cause.as_ref()),
            Some(ErrorCause::Value(_)) | None => None,
        }
    }
}

pub fn is_error2(error: &(dyn Error + 'static)) -> bool {
    error.downcast_ref::<Error2>().is_some()
}

#[derive(Debug, Clone, Copy)]
pub enum ErrorCauseRef<'a> {
    Error(&'a (dyn Error + 'static)),
    Value(&'a Value),
}

// Original: errors.ts, unwrapErrorCause()
pub fn unwrap_error_cause<'a>(error: &'a (dyn Error + 'static)) -> ErrorCauseRef<'a> {
    let mut current = error;
    loop {
        let Some(error2) = current.downcast_ref::<Error2>() else {
            return ErrorCauseRef::Error(current);
        };
        match error2.cause.as_ref() {
            Some(ErrorCause::Error(cause)) => current = cause.as_ref(),
            Some(ErrorCause::Value(value)) => return ErrorCauseRef::Value(value),
            None => return ErrorCauseRef::Error(current),
        }
    }
}

#[derive(Debug)]
pub struct NotImplementedError {
    inner: Error2,
}

impl NotImplementedError {
    pub fn new(feature: Option<&str>) -> Self {
        let message = feature.map_or_else(
            || "Not implemented".to_owned(),
            |feature| format!("Not implemented: {feature}"),
        );
        let mut inner = Error2::new(CORE_NOT_IMPLEMENTED, message);
        inner.name = "NotImplementedError".to_owned();
        Self { inner }
    }

    pub fn error(&self) -> &Error2 {
        &self.inner
    }
}

impl fmt::Display for NotImplementedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl Error for NotImplementedError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn error2_preserves_code_name_details_and_cause_chain() {
        let root: Arc<dyn Error + Send + Sync> = Arc::new(io::Error::other("root"));
        let inner: Arc<dyn Error + Send + Sync> = Arc::new(Error2::with_options(
            "inner",
            "wrapped",
            Error2Options {
                cause: Some(ErrorCause::Error(Arc::clone(&root))),
                ..Error2Options::default()
            },
        ));
        let outer = Error2::with_options(
            "outer",
            "top",
            Error2Options {
                name: Some("CustomError".to_owned()),
                details: Some(Map::from_iter([(
                    "statusCode".to_owned(),
                    Value::from(503),
                )])),
                cause: Some(ErrorCause::Error(inner)),
            },
        );
        assert_eq!(outer.name, "CustomError");
        assert_eq!(outer.code, "outer");
        assert_eq!(outer.details.as_ref().unwrap()["statusCode"], 503);
        let ErrorCauseRef::Error(root) = unwrap_error_cause(&outer) else {
            panic!("expected error cause")
        };
        assert_eq!(root.to_string(), "root");
        assert!(is_error2(&outer));
    }

    #[test]
    fn error2_preserves_non_error_causes() {
        let error = Error2::with_options(
            "internal",
            "value cause",
            Error2Options {
                cause: Some(ErrorCause::Value(Value::String("boom".to_owned()))),
                ..Error2Options::default()
            },
        );
        let ErrorCauseRef::Value(value) = unwrap_error_cause(&error) else {
            panic!("expected value cause")
        };
        assert_eq!(value, "boom");
        assert!(error.source().is_none());
    }

    #[test]
    fn no_telemetry_wrapper_and_expected_error_keep_control_flow_markers() {
        let source = io::Error::other("quiet");
        let wrapped = ErrorNoTelemetry::from_error(&source);
        assert_eq!(wrapped.to_string(), "quiet");
        assert!(ErrorNoTelemetry::is_error_no_telemetry(&wrapped));
        assert!(ExpectedError::new("expected").is_expected());
    }

    #[test]
    fn bug_and_not_implemented_defaults_match_source_messages() {
        assert_eq!(
            BugIndicatingError::new(None).to_string(),
            "An unexpected bug occurred."
        );
        let missing = NotImplementedError::new(None);
        assert_eq!(missing.to_string(), "Not implemented");
        assert_eq!(missing.error().code, CORE_NOT_IMPLEMENTED);
        assert_eq!(
            NotImplementedError::new(Some("video")).to_string(),
            "Not implemented: video"
        );
    }
}
