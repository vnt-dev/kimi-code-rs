use regex::Regex;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::sync::LazyLock;

use super::provider::FinishReason;

const ABORT_MESSAGE: &str = "The operation was aborted.";
const THINKING_EFFORT_CONFIG_DOCS_URL: &str =
    "https://moonshotai.github.io/kimi-code/en/configuration/config-files.html#thinking";

#[derive(Debug, Clone, PartialEq)]
pub struct ApiStatusData {
    pub status_code: i32,
    pub request_id: Option<String>,
    pub retry_after_ms: Option<f64>,
    pub trace_id: Option<String>,
}

impl ApiStatusData {
    pub fn new(
        status_code: i32,
        request_id: Option<String>,
        retry_after_ms: Option<f64>,
        trace_id: Option<String>,
    ) -> Self {
        Self {
            status_code,
            request_id,
            retry_after_ms,
            trace_id,
        }
    }
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/errors.ts
//   ChatProviderError and the API*Error class family.
//
// Rust adaptation:
//   The inheritance hierarchy becomes an exhaustive enum. Status subclasses
//   retain common ApiStatusData, so status, request, retry, and trace metadata
//   remain available without string downcasts.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatProviderError {
    Abort,
    ApiConnection {
        message: String,
    },
    ApiTimeout {
        message: String,
    },
    ApiStatus {
        message: String,
        data: ApiStatusData,
    },
    ApiContextOverflow {
        message: String,
        data: ApiStatusData,
    },
    ApiRequestTooLarge {
        message: String,
        data: ApiStatusData,
    },
    ApiProviderRateLimit {
        message: String,
        data: ApiStatusData,
    },
    ApiProviderOverloaded {
        message: String,
        data: ApiStatusData,
    },
    ApiEmptyResponse {
        message: String,
        finish_reason: Option<FinishReason>,
        raw_finish_reason: Option<String>,
    },
    ChatProvider {
        message: String,
    },
    External {
        name: String,
        message: String,
        status_code: Option<i32>,
    },
    Other {
        message: String,
    },
}

impl ChatProviderError {
    pub fn message(&self) -> &str {
        match self {
            Self::Abort => ABORT_MESSAGE,
            Self::ApiConnection { message }
            | Self::ApiTimeout { message }
            | Self::ApiStatus { message, .. }
            | Self::ApiContextOverflow { message, .. }
            | Self::ApiRequestTooLarge { message, .. }
            | Self::ApiProviderRateLimit { message, .. }
            | Self::ApiProviderOverloaded { message, .. }
            | Self::ApiEmptyResponse { message, .. }
            | Self::ChatProvider { message }
            | Self::External { message, .. }
            | Self::Other { message } => message,
        }
    }

    pub fn status_data(&self) -> Option<&ApiStatusData> {
        match self {
            Self::ApiStatus { data, .. }
            | Self::ApiContextOverflow { data, .. }
            | Self::ApiRequestTooLarge { data, .. }
            | Self::ApiProviderRateLimit { data, .. }
            | Self::ApiProviderOverloaded { data, .. } => Some(data),
            _ => None,
        }
    }

    pub fn status_code(&self) -> Option<i32> {
        self.status_data()
            .map(|data| data.status_code)
            .or(match self {
                Self::External { status_code, .. } => *status_code,
                _ => None,
            })
    }

    pub fn connection(message: impl Into<String>) -> Self {
        Self::ApiConnection {
            message: message.into(),
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::ApiTimeout {
            message: message.into(),
        }
    }

    pub fn empty_response(
        message: impl Into<String>,
        finish_reason: Option<FinishReason>,
        raw_finish_reason: Option<String>,
    ) -> Self {
        Self::ApiEmptyResponse {
            message: message.into(),
            finish_reason,
            raw_finish_reason,
        }
    }
}

impl fmt::Display for ChatProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl Error for ChatProviderError {}

// Original: errors.ts, createAbortError()
pub fn create_abort_error() -> ChatProviderError {
    ChatProviderError::Abort
}

// Original: errors.ts, isAbortError()
pub fn is_abort_error(error: &ChatProviderError) -> bool {
    matches!(error, ChatProviderError::Abort)
        || matches!(
            error,
            ChatProviderError::External { name, .. }
                if name == "AbortError" || name == "APIUserAbortError"
        )
}

// Original: errors.ts, throwIfAbortError()
pub fn throw_if_abort_error(error: &ChatProviderError) -> Result<(), ChatProviderError> {
    if is_abort_error(error) {
        Err(create_abort_error())
    } else {
        Ok(())
    }
}

fn compile_patterns(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .map(|pattern| Regex::new(pattern).expect("static error regex must compile"))
        .collect()
}

fn matches_any(patterns: &[Regex], message: &str) -> bool {
    patterns.iter().any(|pattern| pattern.is_match(message))
}

static IMAGE_FORMAT_PROVIDER_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    compile_patterns(&[
        "unsupported media type for base64 image",
        "invalid data url for image",
    ])
});
static IMAGE_FORMAT_STATUS_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    compile_patterns(&[
        r"unsupported image (?:url|format|type)",
        "does not represent a valid image",
        r"could not (?:process|decode) (?:the |input )?image",
        r"unable to process (?:the |input )?image",
        r"failed to decode (?:the )?image",
        r"invalid image(?: data| type| format)?",
    ])
});
static MEDIA_TYPE_FIELD_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:media|mime)_?type").unwrap());

// Original: errors.ts, isImageFormatError()
pub fn is_image_format_error(error: &ChatProviderError) -> bool {
    if let Some(data) = error.status_data() {
        if matches!(
            error,
            ChatProviderError::ApiContextOverflow { .. }
                | ChatProviderError::ApiRequestTooLarge { .. }
        ) || data.status_code != 400
        {
            return false;
        }
        let message = error.message().to_lowercase();
        return matches_any(&IMAGE_FORMAT_STATUS_PATTERNS, &message)
            || (MEDIA_TYPE_FIELD_PATTERN.is_match(&message) && message.contains("image"));
    }
    if matches!(
        error,
        ChatProviderError::ApiConnection { .. }
            | ChatProviderError::ApiTimeout { .. }
            | ChatProviderError::ApiEmptyResponse { .. }
            | ChatProviderError::ChatProvider { .. }
    ) {
        return matches_any(
            &IMAGE_FORMAT_PROVIDER_PATTERNS,
            &error.message().to_lowercase(),
        );
    }
    false
}

// Original: errors.ts, isRetryableGenerateError()
pub fn is_retryable_generate_error(error: &ChatProviderError) -> bool {
    match error {
        ChatProviderError::ApiConnection { .. }
        | ChatProviderError::ApiTimeout { .. }
        | ChatProviderError::ApiEmptyResponse { .. }
        | ChatProviderError::ApiProviderOverloaded { .. } => true,
        error if error.status_data().is_some() => {
            matches!(
                error.status_code(),
                Some(408 | 409 | 429 | 500 | 502 | 503 | 504 | 529)
            )
        }
        ChatProviderError::ChatProvider { .. } => !is_image_format_error(error),
        _ => false,
    }
}

static NETWORK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)network|connection|connect|disconnect|terminated").unwrap());
static TIMEOUT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)timed?\s*out|timeout|deadline").unwrap());

// Original: errors.ts, classifyBaseApiError()
pub fn classify_base_api_error(message: &str) -> ChatProviderError {
    if TIMEOUT_PATTERN.is_match(message) {
        return ChatProviderError::timeout(message);
    }
    if NETWORK_PATTERN.is_match(message) {
        return ChatProviderError::connection(message);
    }
    ChatProviderError::ChatProvider {
        message: format!("Error: {message}"),
    }
}

static CONTEXT_OVERFLOW_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    compile_patterns(&[
        r"context[ _-]?length",
        r"(?:context[ _-]?window.*exceed|exceed.*context[ _-]?window)",
        "maximum context",
        r"exceed(?:ed|s|ing)?\s+(?:the\s+)?max(?:imum)?\s+tokens?",
        r"(?:too many tokens.*(?:prompt|input|context)|(?:prompt|input|context).*too many tokens)",
        "prompt is too long.*maximum",
        "input token count.*exceeds?.*maximum number of tokens",
        r"request.*exceed(?:ed|s|ing)?.*model token limit",
    ])
});
static PROVIDER_RATE_LIMIT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    compile_patterns(&[
        r"(?:apistatuserror.*429|429.*apistatuserror)",
        "429.*too many requests",
        "too many requests",
        r"provider\.rate_limit",
        "reached .*max rpm",
        r"rate[ _-]?limit(?:ed)?",
        "rate-limited",
    ])
});
static PROVIDER_OVERLOAD_PATTERNS: LazyLock<Vec<Regex>> =
    LazyLock::new(|| compile_patterns(&["overload"]));
static REQUEST_TOO_LARGE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    compile_patterns(&[
        "request exceeds the maximum size",
        "request entity too large",
        "request_too_large",
        "exceeds? the maximum allowed number of bytes",
        "payload too large",
        "content too large",
        r"request (?:body )?too large",
    ])
});
static THINKING_EFFORT_STATUS_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    compile_patterns(&[
        r"reasoning[_ .-]?effort",
        r"thinking[_ .-]?effort",
        r"output_config[\s\S]*effort",
        r"unsupported[\s\S]*effort",
        r"invalid[\s\S]*effort",
    ])
});

fn append_thinking_effort_config_hint(status_code: i32, message: &str) -> String {
    if status_code != 400 && status_code != 422 {
        return message.to_owned();
    }
    let lower_message = message.to_lowercase();
    if !matches_any(&THINKING_EFFORT_STATUS_PATTERNS, &lower_message)
        || message.contains(THINKING_EFFORT_CONFIG_DOCS_URL)
    {
        return message.to_owned();
    }
    format!(
        "{message}\n\nThe provider rejected the configured thinking effort. Non-Kimi providers receive effort strings without client-side mapping; choose an effort supported by the selected model. For Kimi models, check support_efforts and default_effort. See {THINKING_EFFORT_CONFIG_DOCS_URL}"
    )
}

pub fn is_context_overflow_error_code(code: Option<&str>) -> bool {
    code == Some("context_length_exceeded")
}

// Original: errors.ts, normalizeAPIStatusError()
pub fn normalize_api_status_error(
    status_code: i32,
    message: &str,
    request_id: Option<String>,
    retry_after_ms: Option<f64>,
    trace_id: Option<String>,
) -> ChatProviderError {
    let data = ApiStatusData::new(status_code, request_id, retry_after_ms, trace_id);
    if status_code == 429 {
        return ChatProviderError::ApiProviderRateLimit {
            message: message.to_owned(),
            data,
        };
    }
    if is_context_overflow_status_error(status_code, message) {
        return ChatProviderError::ApiContextOverflow {
            message: message.to_owned(),
            data,
        };
    }
    if is_request_too_large_status_error(status_code, message) {
        return ChatProviderError::ApiRequestTooLarge {
            message: message.to_owned(),
            data,
        };
    }
    if is_provider_overload_status_error(status_code, message) {
        return ChatProviderError::ApiProviderOverloaded {
            message: message.to_owned(),
            data,
        };
    }
    ChatProviderError::ApiStatus {
        message: append_thinking_effort_config_hint(status_code, message),
        data,
    }
}

// Original: errors.ts, parseRetryAfterMs()
pub fn parse_retry_after_ms(headers: Option<&HeaderMap>) -> Option<f64> {
    let raw = headers?.get("retry-after")?.to_str().ok()?;
    static PARSE_INT_PREFIX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*([+-]?\d+)").unwrap());
    let digits = PARSE_INT_PREFIX.captures(raw)?.get(1)?.as_str();
    let seconds = digits.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(seconds * 1_000.0)
}

// Original: errors.ts, parseTraceId()
pub fn parse_trace_id(headers: Option<&HeaderMap>) -> Option<String> {
    let raw = headers?.get("x-trace-id")?.to_str().ok()?;
    (!raw.is_empty()).then(|| raw.to_owned())
}

pub fn is_context_overflow_status_error(status_code: i32, message: &str) -> bool {
    matches!(status_code, 400 | 413 | 422)
        && matches_any(&CONTEXT_OVERFLOW_PATTERNS, &message.to_lowercase())
}

pub fn is_provider_overload_status_error(status_code: i32, message: &str) -> bool {
    status_code == 529
        || (matches!(status_code, 500 | 503)
            && matches_any(&PROVIDER_OVERLOAD_PATTERNS, &message.to_lowercase()))
}

pub fn is_request_too_large_status_error(status_code: i32, message: &str) -> bool {
    status_code == 413 && matches_any(&REQUEST_TOO_LARGE_PATTERNS, &message.to_lowercase())
}

static TOOL_EXCHANGE_ADJACENCY_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    compile_patterns(&[
        r"tool_use[\s\S]*tool_result",
        r"tool_result[\s\S]*tool_use",
        r"unexpected\s+`?tool_result",
        r"tool_call_id[\s\S]*not found",
        r#"role\s+['"`]?tool['"`]?\s+must be a response to a preceding message"#,
        r#"assistant message with\s+['"`]?tool_calls['"`]?\s+must be followed by tool messages"#,
        "tool_call_ids? did not have response messages",
        "insufficient tool messages following",
    ])
});
static STRUCTURAL_REQUEST_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    compile_patterns(&[
        "text content blocks must be non-empty",
        "text content blocks must contain non-whitespace",
        "first message must use the .*user.* role",
        "roles must alternate",
        "multiple .*(?:user|assistant).* roles in a row",
        r"tool_use[\s\S]*ids must be unique",
        r#"message at position \d+ with role ['"`]?[a-z]+['"`]? must not be empty"#,
    ])
});

pub fn is_tool_exchange_adjacency_error(error: &ChatProviderError) -> bool {
    let Some(status) = error.status_code() else {
        return false;
    };
    !matches!(error, ChatProviderError::ApiContextOverflow { .. })
        && matches!(status, 400 | 422)
        && matches_any(
            &TOOL_EXCHANGE_ADJACENCY_PATTERNS,
            &error.message().to_lowercase(),
        )
}

pub fn is_recoverable_request_structure_error(error: &ChatProviderError) -> bool {
    if is_tool_exchange_adjacency_error(error) {
        return true;
    }
    let Some(status) = error.status_code() else {
        return false;
    };
    !matches!(error, ChatProviderError::ApiContextOverflow { .. })
        && matches!(status, 400 | 422)
        && matches_any(
            &STRUCTURAL_REQUEST_PATTERNS,
            &error.message().to_lowercase(),
        )
}

pub fn is_provider_rate_limit_error(error: &ChatProviderError) -> bool {
    if matches!(error, ChatProviderError::ApiProviderRateLimit { .. }) {
        return true;
    }
    if let Some(status_code) = error.status_code() {
        return status_code == 429;
    }
    matches_any(
        &PROVIDER_RATE_LIMIT_PATTERNS,
        &error.message().to_lowercase(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ApiErrorKind {
    #[serde(rename = "context_overflow")]
    ContextOverflow,
    #[serde(rename = "overloaded")]
    Overloaded,
    #[serde(rename = "rate_limit")]
    RateLimit,
    #[serde(rename = "auth")]
    Auth,
    #[serde(rename = "5xx_server")]
    Server5xx,
    #[serde(rename = "4xx_client")]
    Client4xx,
    #[serde(rename = "network")]
    Network,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "empty_response")]
    EmptyResponse,
    #[serde(rename = "other")]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorClassification {
    pub kind: ApiErrorKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<i32>,
}

// Original: errors.ts, classifyApiError()
pub fn classify_api_error(error: &ChatProviderError) -> ApiErrorClassification {
    let status_code = error.status_code();
    let kind = match error {
        ChatProviderError::ApiContextOverflow { .. } => ApiErrorKind::ContextOverflow,
        ChatProviderError::ApiProviderOverloaded { .. } => ApiErrorKind::Overloaded,
        error if error.status_data().is_some() => {
            let status = status_code.expect("status error has status code");
            if is_context_overflow_status_error(status, error.message()) {
                ApiErrorKind::ContextOverflow
            } else if status == 429 {
                ApiErrorKind::RateLimit
            } else if status == 529 {
                ApiErrorKind::Overloaded
            } else if matches!(status, 401 | 403) {
                ApiErrorKind::Auth
            } else if status >= 500 {
                ApiErrorKind::Server5xx
            } else if status >= 400 {
                ApiErrorKind::Client4xx
            } else {
                ApiErrorKind::Other
            }
        }
        ChatProviderError::ApiConnection { .. } => ApiErrorKind::Network,
        ChatProviderError::ApiTimeout { .. } => ApiErrorKind::Timeout,
        ChatProviderError::ApiEmptyResponse { .. } => ApiErrorKind::EmptyResponse,
        _ => ApiErrorKind::Other,
    };
    ApiErrorClassification { kind, status_code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    fn status_error(status_code: i32, message: &str) -> ChatProviderError {
        ChatProviderError::ApiStatus {
            message: message.to_owned(),
            data: ApiStatusData::new(status_code, None, None, None),
        }
    }

    #[test]
    fn abort_guard_recognizes_and_standardizes_all_abort_shapes() {
        for abort in [
            create_abort_error(),
            ChatProviderError::External {
                name: "AbortError".to_owned(),
                message: "aborted".to_owned(),
                status_code: None,
            },
            ChatProviderError::External {
                name: "APIUserAbortError".to_owned(),
                message: "connection aborted by user".to_owned(),
                status_code: None,
            },
        ] {
            assert!(is_abort_error(&abort));
            assert_eq!(
                throw_if_abort_error(&abort).unwrap_err(),
                create_abort_error()
            );
            assert!(!is_retryable_generate_error(&abort));
        }
        let connection = ChatProviderError::connection("Connection error.");
        assert!(!is_abort_error(&connection));
        assert_eq!(throw_if_abort_error(&connection), Ok(()));
        assert_eq!(create_abort_error().to_string(), ABORT_MESSAGE);
    }

    #[test]
    fn retry_verdict_matches_transient_and_deterministic_failures() {
        let retryable = [
            ChatProviderError::connection("Connection error."),
            ChatProviderError::timeout("Request timed out."),
            normalize_api_status_error(429, "Too many requests", None, None, None),
            normalize_api_status_error(529, "Overloaded", None, None, None),
            status_error(503, "Service unavailable"),
            ChatProviderError::empty_response("empty", None, None),
        ];
        assert!(retryable.iter().all(is_retryable_generate_error));
        assert!(!is_retryable_generate_error(&status_error(
            400,
            "Bad request"
        )));
        assert!(!is_retryable_generate_error(&status_error(
            401,
            "Unauthorized"
        )));
    }

    #[test]
    fn image_format_errors_exclude_overflow_and_request_size() {
        assert!(is_image_format_error(&status_error(
            400,
            "Unsupported image format"
        )));
        assert!(is_image_format_error(&ChatProviderError::ChatProvider {
            message: "invalid data url for image".to_owned(),
        }));
        assert!(!is_image_format_error(&normalize_api_status_error(
            400,
            "context length exceeded while reading image",
            None,
            None,
            None,
        )));
        assert!(!is_image_format_error(&normalize_api_status_error(
            413,
            "request entity too large for image",
            None,
            None,
            None,
        )));
    }

    #[test]
    fn base_classifier_preserves_timeout_network_and_generic_messages() {
        assert!(matches!(
            classify_base_api_error("request timed out"),
            ChatProviderError::ApiTimeout { .. }
        ));
        assert!(matches!(
            classify_base_api_error("connection terminated"),
            ChatProviderError::ApiConnection { .. }
        ));
        assert_eq!(classify_base_api_error("boom").message(), "Error: boom");
    }

    #[test]
    fn status_normalizer_preserves_branch_order_and_metadata() {
        let rate = normalize_api_status_error(
            429,
            "context length exceeded",
            Some("request-1".to_owned()),
            Some(2000.0),
            Some("trace-1".to_owned()),
        );
        assert!(matches!(
            rate,
            ChatProviderError::ApiProviderRateLimit { .. }
        ));
        let data = rate.status_data().unwrap();
        assert_eq!(data.status_code, 429);
        assert_eq!(data.request_id.as_deref(), Some("request-1"));
        assert_eq!(data.retry_after_ms, Some(2000.0));
        assert_eq!(data.trace_id.as_deref(), Some("trace-1"));

        assert!(matches!(
            normalize_api_status_error(413, "context length exceeded", None, None, None),
            ChatProviderError::ApiContextOverflow { .. }
        ));
        assert!(matches!(
            normalize_api_status_error(413, "request entity too large", None, None, None),
            ChatProviderError::ApiRequestTooLarge { .. }
        ));
        assert!(matches!(
            normalize_api_status_error(503, "provider overloaded", None, None, None),
            ChatProviderError::ApiProviderOverloaded { .. }
        ));
    }

    #[test]
    fn thinking_effort_hint_is_scoped_and_idempotent() {
        let error =
            normalize_api_status_error(422, "invalid reasoning_effort value", None, None, None);
        assert!(error.message().contains(THINKING_EFFORT_CONFIG_DOCS_URL));
        let repeated = normalize_api_status_error(422, error.message(), None, None, None);
        assert_eq!(repeated.message(), error.message());
        assert_eq!(
            normalize_api_status_error(500, "invalid effort", None, None, None).message(),
            "invalid effort"
        );
    }

    #[test]
    fn retry_after_parser_matches_javascript_parse_int_prefix_rules() {
        for (raw, expected) in [
            ("2", Some(2000.0)),
            ("  +3seconds", Some(3000.0)),
            ("1.5", Some(1000.0)),
            ("-1", None),
            ("date", None),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("retry-after", HeaderValue::from_str(raw).unwrap());
            assert_eq!(parse_retry_after_ms(Some(&headers)), expected);
        }
        assert_eq!(parse_retry_after_ms(None), None);
    }

    #[test]
    fn trace_id_parser_preserves_nonempty_header_verbatim() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-trace-id"),
            HeaderValue::from_static("trace-123"),
        );
        assert_eq!(parse_trace_id(Some(&headers)).as_deref(), Some("trace-123"));
        headers.insert("x-trace-id", HeaderValue::from_static(""));
        assert_eq!(parse_trace_id(Some(&headers)), None);
        assert_eq!(parse_trace_id(None), None);
    }

    #[test]
    fn status_message_predicates_keep_their_code_gates() {
        assert!(is_context_overflow_status_error(
            400,
            "maximum context exceeded"
        ));
        assert!(!is_context_overflow_status_error(
            500,
            "maximum context exceeded"
        ));
        assert!(is_provider_overload_status_error(529, "anything"));
        assert!(is_provider_overload_status_error(503, "OVERLOADED"));
        assert!(!is_provider_overload_status_error(400, "overloaded"));
        assert!(is_request_too_large_status_error(413, "payload too large"));
        assert!(!is_request_too_large_status_error(400, "payload too large"));
        assert!(is_context_overflow_error_code(Some(
            "context_length_exceeded"
        )));
    }

    #[test]
    fn structural_request_classifiers_preserve_subcategories() {
        let adjacency = status_error(400, "tool_call_id abc not found");
        assert!(is_tool_exchange_adjacency_error(&adjacency));
        assert!(is_recoverable_request_structure_error(&adjacency));

        let structural = status_error(422, "roles must alternate");
        assert!(!is_tool_exchange_adjacency_error(&structural));
        assert!(is_recoverable_request_structure_error(&structural));

        let overflow = normalize_api_status_error(
            400,
            "context length and roles must alternate",
            None,
            None,
            None,
        );
        assert!(!is_recoverable_request_structure_error(&overflow));
    }

    #[test]
    fn rate_limit_classifier_uses_typed_status_then_message_fallback() {
        assert!(is_provider_rate_limit_error(&normalize_api_status_error(
            429, "anything", None, None, None,
        )));
        assert!(is_provider_rate_limit_error(&ChatProviderError::External {
            name: "Error".to_owned(),
            message: "provider.rate_limit".to_owned(),
            status_code: None,
        }));
        assert!(!is_provider_rate_limit_error(
            &ChatProviderError::External {
                name: "Error".to_owned(),
                message: "too many requests".to_owned(),
                status_code: Some(400),
            }
        ));
    }

    #[test]
    fn telemetry_classification_matches_typed_and_status_errors() {
        let cases = [
            (
                normalize_api_status_error(400, "context length exceeded", None, None, None),
                ApiErrorKind::ContextOverflow,
            ),
            (
                normalize_api_status_error(429, "Too many requests", None, None, None),
                ApiErrorKind::RateLimit,
            ),
            (
                normalize_api_status_error(529, "Overloaded", None, None, None),
                ApiErrorKind::Overloaded,
            ),
            (status_error(401, "Unauthorized"), ApiErrorKind::Auth),
            (status_error(500, "Internal"), ApiErrorKind::Server5xx),
            (status_error(422, "Nope"), ApiErrorKind::Client4xx),
            (
                ChatProviderError::connection("network"),
                ApiErrorKind::Network,
            ),
            (ChatProviderError::timeout("timeout"), ApiErrorKind::Timeout),
            (
                ChatProviderError::empty_response("empty", None, None),
                ApiErrorKind::EmptyResponse,
            ),
            (
                ChatProviderError::Other {
                    message: "boom".to_owned(),
                },
                ApiErrorKind::Other,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(classify_api_error(&error).kind, expected);
        }
        assert_eq!(
            classify_api_error(&status_error(413, "Request exceeds the maximum size")).kind,
            ApiErrorKind::Client4xx
        );
        assert_eq!(
            classify_api_error(&normalize_api_status_error(
                413,
                "context length exceeded",
                None,
                None,
                None,
            ))
            .kind,
            ApiErrorKind::ContextOverflow
        );
    }

    #[test]
    fn classification_serialization_preserves_telemetry_names() {
        assert_eq!(
            serde_json::to_value(ApiErrorClassification {
                kind: ApiErrorKind::Server5xx,
                status_code: Some(503),
            })
            .unwrap(),
            serde_json::json!({"kind":"5xx_server","statusCode":503})
        );
    }
}
