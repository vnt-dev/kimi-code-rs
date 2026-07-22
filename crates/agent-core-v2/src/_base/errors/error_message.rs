use std::error::Error;

use super::errors::{Error2, ErrorCause};

// Original: packages/agent-core-v2/src/_base/errors/errorMessage.ts, toErrorMessage().
pub fn to_error_message(error: &(dyn Error + 'static), verbose: bool) -> String {
    if let Some(error) = error.downcast_ref::<Error2>() {
        let mut message = format!("[{}] {}", error.code, error.message);
        if verbose && let Some(details) = &error.details {
            message.push(' ');
            message.push_str(&serde_json::Value::Object(details.clone()).to_string());
        }
        return message;
    }
    let message = error.to_string();
    if !verbose {
        return message;
    }
    let cause = error
        .downcast_ref::<Error2>()
        .and_then(Error2::cause)
        .and_then(|cause| match cause {
            ErrorCause::Error(error) => Some(error.as_ref() as &(dyn Error + 'static)),
            ErrorCause::Value(_) => None,
        })
        .or_else(|| error.source());
    match cause {
        Some(cause) => format!("{message} (caused by: {})", to_error_message(cause, false)),
        None => message,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Map, Value};

    use super::*;
    use crate::_base::errors::errors::{Error2Options, ErrorCause};

    #[test]
    fn formats_coded_details_and_plain_causes() {
        let coded = Error2::with_options(
            "provider.rate_limit",
            "slow",
            Error2Options {
                details: Some(Map::from_iter([("statusCode".into(), Value::from(429))])),
                ..Default::default()
            },
        );
        assert_eq!(
            to_error_message(&coded, false),
            "[provider.rate_limit] slow"
        );
        assert_eq!(
            to_error_message(&coded, true),
            "[provider.rate_limit] slow {\"statusCode\":429}"
        );

        let outer = Error2::with_options(
            "internal",
            "outer",
            Error2Options {
                cause: Some(ErrorCause::Error(Arc::new(std::io::Error::other("inner")))),
                ..Default::default()
            },
        );
        assert_eq!(to_error_message(&outer, false), "[internal] outer");
    }
}
