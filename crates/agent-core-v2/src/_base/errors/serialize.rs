use std::{error::Error, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    codes::{CORE_INTERNAL, error_info},
    errors::{Error2, Error2Options, ErrorCause},
};

const MAX_CAUSE_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Map<String, Value>>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<Box<ErrorPayload>>,
}

pub type KimiErrorPayload = ErrorPayload;

pub fn make_error_payload(
    code: impl Into<String>,
    message: impl Into<String>,
    details: Option<Map<String, Value>>,
    name: Option<String>,
) -> ErrorPayload {
    let code = code.into();
    ErrorPayload {
        retryable: error_info(&code).retryable,
        code,
        message: message.into(),
        name,
        details,
        cause: None,
    }
}

// Original: packages/agent-core-v2/src/_base/errors/serialize.ts, toErrorPayload().
pub fn to_error_payload(error: &(dyn Error + 'static)) -> ErrorPayload {
    to_error_payload_at_depth(error, 0)
}

fn to_error_payload_at_depth(error: &(dyn Error + 'static), depth: usize) -> ErrorPayload {
    let mut payload = if let Some(error) = error.downcast_ref::<Error2>() {
        make_error_payload(
            error.code.clone(),
            error.message.clone(),
            error.details.clone(),
            Some(error.name.clone()),
        )
    } else {
        make_error_payload(
            CORE_INTERNAL,
            error.to_string(),
            None,
            Some(std::any::type_name_of_val(error).to_owned()),
        )
    };
    if depth >= MAX_CAUSE_DEPTH {
        return payload;
    }
    let cause = error
        .downcast_ref::<Error2>()
        .and_then(Error2::cause)
        .map(|cause| match cause {
            ErrorCause::Error(error) => to_error_payload_at_depth(error.as_ref(), depth + 1),
            ErrorCause::Value(value) => to_error_payload_value_at_depth(value, depth + 1),
        })
        .or_else(|| {
            error
                .source()
                .map(|source| to_error_payload_at_depth(source, depth + 1))
        });
    payload.cause = cause.map(Box::new);
    payload
}

pub fn to_error_payload_value(value: &Value) -> ErrorPayload {
    to_error_payload_value_at_depth(value, 0)
}

fn to_error_payload_value_at_depth(value: &Value, _depth: usize) -> ErrorPayload {
    let message = match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".into(),
        value => value.to_string(),
    };
    make_error_payload(CORE_INTERNAL, message, None, None)
}

// Original: fromErrorPayload().
pub fn from_error_payload(payload: &ErrorPayload) -> Error2 {
    let cause = payload.cause.as_deref().map(|cause| {
        ErrorCause::Error(Arc::new(from_error_payload(cause)) as Arc<dyn Error + Send + Sync>)
    });
    Error2::with_options(
        payload.code.clone(),
        payload.message.clone(),
        Error2Options {
            name: payload.name.clone(),
            details: payload.details.clone(),
            cause,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coded_errors_preserve_details_retryability_and_cause() {
        static DOMAIN: super::super::codes::ErrorDomain = super::super::codes::ErrorDomain {
            codes: &[("RATE", "test.serialize.rate")],
            retryable: &["test.serialize.rate"],
            info: &[],
        };
        super::super::codes::register_error_domain(&DOMAIN).unwrap();
        let inner: Arc<dyn Error + Send + Sync> = Arc::new(std::io::Error::other("socket"));
        let error = Error2::with_options(
            "test.serialize.rate",
            "slow down",
            Error2Options {
                name: Some("APIStatusError".into()),
                details: Some(Map::from_iter([("statusCode".into(), Value::from(429))])),
                cause: Some(ErrorCause::Error(inner)),
            },
        );

        let payload = to_error_payload(&error);
        assert!(payload.retryable);
        assert_eq!(payload.details.as_ref().unwrap()["statusCode"], 429);
        assert_eq!(payload.cause.as_ref().unwrap().message, "socket");
        let revived = from_error_payload(&payload);
        assert_eq!(revived.code, "test.serialize.rate");
        assert_eq!(revived.source().unwrap().to_string(), "socket");
    }

    #[test]
    fn cause_depth_is_capped_at_eight() {
        let mut error: Arc<dyn Error + Send + Sync> = Arc::new(Error2::new("internal", "root"));
        for index in 0..20 {
            error = Arc::new(Error2::with_options(
                "internal",
                format!("layer {index}"),
                Error2Options {
                    cause: Some(ErrorCause::Error(error)),
                    ..Default::default()
                },
            ));
        }
        let payload = to_error_payload(error.as_ref());
        let mut depth = 0;
        let mut current = &payload;
        while let Some(cause) = current.cause.as_deref() {
            depth += 1;
            current = cause;
        }
        assert_eq!(depth, 8);
    }
}
