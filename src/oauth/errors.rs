use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthErrorKind {
    General,
    Unauthorized,
    Connection,
    DeviceCodeExpired,
    DeviceCodeTimeout,
    RetryableRefresh,
}

#[derive(Debug)]
pub struct OAuthError {
    kind: OAuthErrorKind,
    message: String,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl OAuthError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::with_kind(OAuthErrorKind::General, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::with_kind(OAuthErrorKind::Unauthorized, message)
    }

    pub fn connection(
        message: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind: OAuthErrorKind::Connection,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    pub fn device_code_expired() -> Self {
        Self::with_kind(OAuthErrorKind::DeviceCodeExpired, "Device code expired.")
    }

    pub fn device_code_timeout() -> Self {
        Self::with_kind(
            OAuthErrorKind::DeviceCodeTimeout,
            "Device authorization timed out locally.",
        )
    }

    pub fn device_code_timeout_with_message(message: impl Into<String>) -> Self {
        Self::with_kind(OAuthErrorKind::DeviceCodeTimeout, message)
    }

    pub fn retryable_refresh(message: impl Into<String>) -> Self {
        Self::with_kind(OAuthErrorKind::RetryableRefresh, message)
    }

    pub fn kind(&self) -> OAuthErrorKind {
        self.kind
    }

    fn with_kind(kind: OAuthErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for OAuthError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|error| error as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_distinct_error_kinds_and_original_default_messages() {
        assert_eq!(
            OAuthError::device_code_expired().to_string(),
            "Device code expired."
        );
        assert_eq!(
            OAuthError::device_code_timeout().to_string(),
            "Device authorization timed out locally."
        );
        assert_eq!(
            OAuthError::unauthorized("bad token").kind(),
            OAuthErrorKind::Unauthorized
        );
        assert_eq!(
            OAuthError::retryable_refresh("busy").kind(),
            OAuthErrorKind::RetryableRefresh
        );
    }

    #[test]
    fn connection_error_preserves_its_transport_source() {
        let error = OAuthError::connection("OAuth request failed", std::io::Error::other("down"));
        assert_eq!(error.kind(), OAuthErrorKind::Connection);
        assert_eq!(error.to_string(), "OAuth request failed");
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("down")
        );
    }
}
