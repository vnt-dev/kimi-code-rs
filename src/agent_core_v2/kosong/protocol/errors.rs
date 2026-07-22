use regex::Regex;
use serde_json::{Map, Value};
use std::error::Error;
use std::sync::{Arc, LazyLock};

use crate::agent_core_v2::_base::errors::codes::{
    CORE_INTERNAL, ErrorDomain, ErrorInfo, register_error_domain,
};
use crate::agent_core_v2::_base::errors::errors::{Error2, Error2Options, ErrorCause};
use crate::agent_core_v2::kosong::contract::errors::ChatProviderError;
use crate::agent_core_v2::kosong::contract::provider::FinishReason;

pub const PROVIDER_API_ERROR: &str = "provider.api_error";
pub const PROVIDER_FILTERED: &str = "provider.filtered";
pub const PROVIDER_RATE_LIMIT: &str = "provider.rate_limit";
pub const PROVIDER_AUTH_ERROR: &str = "provider.auth_error";
pub const PROVIDER_CONNECTION_ERROR: &str = "provider.connection_error";
pub const PROVIDER_OVERLOADED: &str = "provider.overloaded";
pub const CONTEXT_OVERFLOW: &str = "context.overflow";

pub static PROTOCOL_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("PROVIDER_API_ERROR", PROVIDER_API_ERROR),
        ("PROVIDER_FILTERED", PROVIDER_FILTERED),
        ("PROVIDER_RATE_LIMIT", PROVIDER_RATE_LIMIT),
        ("PROVIDER_AUTH_ERROR", PROVIDER_AUTH_ERROR),
        ("PROVIDER_CONNECTION_ERROR", PROVIDER_CONNECTION_ERROR),
        ("PROVIDER_OVERLOADED", PROVIDER_OVERLOADED),
        ("CONTEXT_OVERFLOW", CONTEXT_OVERFLOW),
    ],
    retryable: &[
        PROVIDER_RATE_LIMIT,
        PROVIDER_CONNECTION_ERROR,
        PROVIDER_OVERLOADED,
        CONTEXT_OVERFLOW,
    ],
    info: &[
        (
            PROVIDER_RATE_LIMIT,
            ErrorInfo {
                title: "Provider rate limit",
                retryable: true,
                public: true,
                action: Some("Retry after the provider rate limit resets."),
            },
        ),
        (
            PROVIDER_FILTERED,
            ErrorInfo {
                title: "Provider filtered response",
                retryable: false,
                public: true,
                action: Some(
                    "Revise the prompt or model configuration to avoid provider safety filtering.",
                ),
            },
        ),
        (
            PROVIDER_AUTH_ERROR,
            ErrorInfo {
                title: "Provider authentication failed",
                retryable: false,
                public: true,
                action: Some("Check provider credentials and authentication configuration."),
            },
        ),
        (
            PROVIDER_OVERLOADED,
            ErrorInfo {
                title: "Provider overloaded",
                retryable: true,
                public: true,
                action: Some("Retry after the provider recovers from overload."),
            },
        ),
        (
            CONTEXT_OVERFLOW,
            ErrorInfo {
                title: "Context overflow",
                retryable: true,
                public: true,
                action: Some("Compact the conversation or retry with fewer tokens."),
            },
        ),
    ],
};

static PROTOCOL_ERRORS_REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&PROTOCOL_ERRORS).expect("protocol error codes are unique");
});

pub fn ensure_protocol_errors_registered() {
    LazyLock::force(&PROTOCOL_ERRORS_REGISTERED);
}

pub enum ProviderBoundaryError {
    Coded(Error2),
    Provider(ChatProviderError),
    Error {
        name: String,
        message: String,
        cause: Arc<dyn Error + Send + Sync>,
    },
    Value {
        message: String,
        cause: Value,
    },
}

// Original:
//   packages/agent-core-v2/src/kosong/protocol/errors.ts
//   translateProviderError()
//
// Rust adaptation:
//   Throwing the standardized DOMException becomes Err(Abort); every other
//   input returns a coded Error2. Taking the boundary value by ownership lets
//   Error2 retain the exact typed cause without cloning it.
pub fn translate_provider_error(error: ProviderBoundaryError) -> Result<Error2, ChatProviderError> {
    ensure_protocol_errors_registered();
    match error {
        ProviderBoundaryError::Coded(error) => Ok(error),
        ProviderBoundaryError::Provider(error)
            if crate::agent_core_v2::kosong::contract::errors::is_abort_error(&error) =>
        {
            Err(crate::agent_core_v2::kosong::contract::errors::create_abort_error())
        }
        ProviderBoundaryError::Provider(error) => translate_typed_provider_error(error),
        ProviderBoundaryError::Error {
            name,
            message,
            cause,
        } => Ok(Error2::with_options(
            CORE_INTERNAL,
            message,
            Error2Options {
                name: Some(name),
                cause: Some(ErrorCause::Error(cause)),
                ..Error2Options::default()
            },
        )),
        ProviderBoundaryError::Value { message, cause } => Ok(Error2::with_options(
            CORE_INTERNAL,
            message,
            Error2Options {
                cause: Some(ErrorCause::Value(cause)),
                ..Error2Options::default()
            },
        )),
    }
}

fn translate_typed_provider_error(error: ChatProviderError) -> Result<Error2, ChatProviderError> {
    if let Some(status) = error.status_data() {
        let code = if matches!(error, ChatProviderError::ApiContextOverflow { .. }) {
            CONTEXT_OVERFLOW
        } else if matches!(error, ChatProviderError::ApiProviderOverloaded { .. })
            || status.status_code == 529
        {
            PROVIDER_OVERLOADED
        } else if status.status_code == 429 {
            PROVIDER_RATE_LIMIT
        } else if matches!(status.status_code, 401 | 403) {
            PROVIDER_AUTH_ERROR
        } else {
            PROVIDER_API_ERROR
        };
        let mut details = Map::new();
        details.insert("statusCode".to_owned(), Value::from(status.status_code));
        details.insert(
            "requestId".to_owned(),
            status
                .request_id
                .as_ref()
                .map_or(Value::Null, |value| Value::String(value.clone())),
        );
        details.insert(
            "traceId".to_owned(),
            status
                .trace_id
                .as_ref()
                .map_or(Value::Null, |value| Value::String(value.clone())),
        );
        let name = error.name().to_owned();
        let message = sanitize_status_error_message(error.message());
        let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
        return Ok(Error2::with_options(
            code,
            message,
            Error2Options {
                name: Some(name),
                details: Some(details),
                cause: Some(ErrorCause::Error(cause)),
            },
        ));
    }

    match error {
        ChatProviderError::ApiConnection { .. } | ChatProviderError::ApiTimeout { .. } => {
            let name = error.name().to_owned();
            let message = error.message().to_owned();
            let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
            Ok(Error2::with_options(
                PROVIDER_CONNECTION_ERROR,
                message,
                Error2Options {
                    name: Some(name),
                    cause: Some(ErrorCause::Error(cause)),
                    ..Error2Options::default()
                },
            ))
        }
        ChatProviderError::ApiEmptyResponse {
            message,
            finish_reason,
            raw_finish_reason,
        } => {
            let code = if finish_reason == Some(FinishReason::Filtered) {
                PROVIDER_FILTERED
            } else {
                PROVIDER_API_ERROR
            };
            let mut details = Map::new();
            details.insert(
                "finishReason".to_owned(),
                finish_reason.map_or(Value::Null, |reason| {
                    Value::String(finish_reason_name(reason).to_owned())
                }),
            );
            details.insert(
                "rawFinishReason".to_owned(),
                raw_finish_reason
                    .as_ref()
                    .map_or(Value::Null, |value| Value::String(value.clone())),
            );
            let cause_error = ChatProviderError::ApiEmptyResponse {
                message: message.clone(),
                finish_reason,
                raw_finish_reason,
            };
            let cause: Arc<dyn Error + Send + Sync> = Arc::new(cause_error);
            Ok(Error2::with_options(
                code,
                message,
                Error2Options {
                    name: Some("APIEmptyResponseError".to_owned()),
                    details: Some(details),
                    cause: Some(ErrorCause::Error(cause)),
                },
            ))
        }
        ChatProviderError::ChatProvider { .. } => {
            let name = error.name().to_owned();
            let message = error.message().to_owned();
            let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
            Ok(Error2::with_options(
                PROVIDER_API_ERROR,
                message,
                Error2Options {
                    name: Some(name),
                    cause: Some(ErrorCause::Error(cause)),
                    ..Error2Options::default()
                },
            ))
        }
        ChatProviderError::External { .. } | ChatProviderError::Other { .. } => {
            let name = error.name().to_owned();
            let message = error.message().to_owned();
            let cause: Arc<dyn Error + Send + Sync> = Arc::new(error);
            Ok(Error2::with_options(
                CORE_INTERNAL,
                message,
                Error2Options {
                    name: Some(name),
                    cause: Some(ErrorCause::Error(cause)),
                    ..Error2Options::default()
                },
            ))
        }
        ChatProviderError::Abort => Err(ChatProviderError::Abort),
        _ => unreachable!("all status errors were handled before variant matching"),
    }
}

static TITLE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());

// Original: protocol/errors.ts, sanitizeStatusErrorMessage()
pub fn sanitize_status_error_message(message: &str) -> String {
    let extracted = TITLE_PATTERN
        .captures(message)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().trim())
        .filter(|value| !value.is_empty());
    extracted.unwrap_or(message).replace('\r', "")
}

fn finish_reason_name(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Completed => "completed",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::Truncated => "truncated",
        FinishReason::Filtered => "filtered",
        FinishReason::Paused => "paused",
        FinishReason::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::_base::errors::codes::{error_info, is_error_code};
    use crate::agent_core_v2::_base::errors::errors::ErrorCause;
    use crate::agent_core_v2::kosong::contract::errors::{
        ApiStatusData, create_abort_error, normalize_api_status_error,
    };
    use std::io;

    #[test]
    fn protocol_domain_registration_exposes_codes_and_metadata() {
        ensure_protocol_errors_registered();
        for (_, code) in PROTOCOL_ERRORS.codes {
            assert!(is_error_code(code));
        }
        assert!(error_info(PROVIDER_RATE_LIMIT).retryable);
        assert_eq!(
            error_info(PROVIDER_FILTERED).title,
            "Provider filtered response"
        );
    }

    #[test]
    fn abort_guard_runs_before_every_mapping() {
        for abort in [
            create_abort_error(),
            ChatProviderError::External {
                name: "AbortError".to_owned(),
                message: "aborted".to_owned(),
                status_code: None,
            },
            ChatProviderError::External {
                name: "APIUserAbortError".to_owned(),
                message: "aborted".to_owned(),
                status_code: None,
            },
        ] {
            assert_eq!(
                translate_provider_error(ProviderBoundaryError::Provider(abort)).unwrap_err(),
                create_abort_error()
            );
        }
    }

    #[test]
    fn coded_error_passes_through_unchanged() {
        let error = Error2::new(PROVIDER_RATE_LIMIT, "slow down");
        let translated = translate_provider_error(ProviderBoundaryError::Coded(error)).unwrap();
        assert_eq!(translated.code, PROVIDER_RATE_LIMIT);
        assert_eq!(translated.message, "slow down");
    }

    #[test]
    fn status_errors_map_codes_and_preserve_wire_details_and_cause() {
        let cases = [
            (
                normalize_api_status_error(429, "too many requests", None, None, None),
                PROVIDER_RATE_LIMIT,
            ),
            (
                normalize_api_status_error(529, "overloaded", None, None, None),
                PROVIDER_OVERLOADED,
            ),
            (
                normalize_api_status_error(503, "overloaded", None, None, None),
                PROVIDER_OVERLOADED,
            ),
            (
                normalize_api_status_error(400, "context length exceeded", None, None, None),
                CONTEXT_OVERFLOW,
            ),
            (
                normalize_api_status_error(401, "bad key", None, None, None),
                PROVIDER_AUTH_ERROR,
            ),
            (
                normalize_api_status_error(403, "forbidden", None, None, None),
                PROVIDER_AUTH_ERROR,
            ),
            (
                normalize_api_status_error(500, "boom", None, None, None),
                PROVIDER_API_ERROR,
            ),
        ];
        for (error, code) in cases {
            let translated =
                translate_provider_error(ProviderBoundaryError::Provider(error)).unwrap();
            assert_eq!(translated.code, code);
            assert!(matches!(translated.cause(), Some(ErrorCause::Error(_))));
            assert!(
                translated
                    .details
                    .as_ref()
                    .unwrap()
                    .contains_key("statusCode")
            );
        }

        let error = ChatProviderError::ApiStatus {
            message: "too many requests".to_owned(),
            data: ApiStatusData::new(
                429,
                Some("req-1".to_owned()),
                None,
                Some("trace-1".to_owned()),
            ),
        };
        let translated = translate_provider_error(ProviderBoundaryError::Provider(error)).unwrap();
        assert_eq!(translated.details.as_ref().unwrap()["requestId"], "req-1");
        assert_eq!(translated.details.as_ref().unwrap()["traceId"], "trace-1");
    }

    #[test]
    fn connection_timeout_empty_and_plain_provider_errors_map_correctly() {
        for error in [
            ChatProviderError::connection("down"),
            ChatProviderError::timeout("slow"),
        ] {
            assert_eq!(
                translate_provider_error(ProviderBoundaryError::Provider(error))
                    .unwrap()
                    .code,
                PROVIDER_CONNECTION_ERROR
            );
        }
        let filtered = ChatProviderError::empty_response(
            "empty",
            Some(FinishReason::Filtered),
            Some("content_filter".to_owned()),
        );
        let translated =
            translate_provider_error(ProviderBoundaryError::Provider(filtered)).unwrap();
        assert_eq!(translated.code, PROVIDER_FILTERED);
        assert_eq!(
            translated.details.as_ref().unwrap()["finishReason"],
            "filtered"
        );
        assert_eq!(
            translated.details.as_ref().unwrap()["rawFinishReason"],
            "content_filter"
        );

        let plain = ChatProviderError::ChatProvider {
            message: "weird".to_owned(),
        };
        assert_eq!(
            translate_provider_error(ProviderBoundaryError::Provider(plain))
                .unwrap()
                .code,
            PROVIDER_API_ERROR
        );
    }

    #[test]
    fn unknown_errors_and_values_map_to_internal_with_cause() {
        let cause: Arc<dyn Error + Send + Sync> = Arc::new(io::Error::other("boom"));
        let translated = translate_provider_error(ProviderBoundaryError::Error {
            name: "Error".to_owned(),
            message: "boom".to_owned(),
            cause,
        })
        .unwrap();
        assert_eq!(translated.code, CORE_INTERNAL);
        assert!(matches!(translated.cause(), Some(ErrorCause::Error(_))));

        let translated = translate_provider_error(ProviderBoundaryError::Value {
            message: "boom".to_owned(),
            cause: Value::String("boom".to_owned()),
        })
        .unwrap();
        assert_eq!(translated.code, CORE_INTERNAL);
        assert_eq!(translated.message, "boom");
        assert!(matches!(translated.cause(), Some(ErrorCause::Value(_))));
    }

    #[test]
    fn status_message_sanitizer_extracts_html_title_and_strips_carriage_returns() {
        let html = "<html>\r\n<head><title>429 Too Many Requests</title></head>\r\n<body>...</body></html>";
        assert_eq!(sanitize_status_error_message(html), "429 Too Many Requests");
        assert_eq!(
            sanitize_status_error_message("line one\r\nline two\r"),
            "line one\nline two"
        );
        assert_eq!(
            sanitize_status_error_message("<title>   </title>fallback"),
            "<title>   </title>fallback"
        );
        assert_eq!(sanitize_status_error_message("plain"), "plain");
    }
}
