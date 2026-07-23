//! Agent loop error domain and max-step error construction.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/errors.ts` and the error
//! helpers in `loop/loop.ts`.

use std::{error::Error, sync::LazyLock};

use serde_json::{Map, Value};

use crate::_base::errors::{
    codes::{ErrorDomain, ErrorInfo, register_error_domain},
    errors::{Error2, Error2Options},
};

pub const LOOP_MAX_STEPS_EXCEEDED: &str = "loop.max_steps_exceeded";
pub const TURN_AGENT_BUSY: &str = "turn.agent_busy";

pub static LOOP_ERRORS: ErrorDomain = ErrorDomain {
    codes: &[
        ("LOOP_MAX_STEPS_EXCEEDED", LOOP_MAX_STEPS_EXCEEDED),
        ("TURN_AGENT_BUSY", TURN_AGENT_BUSY),
    ],
    retryable: &[TURN_AGENT_BUSY],
    info: &[(
        LOOP_MAX_STEPS_EXCEEDED,
        ErrorInfo {
            title: "Loop max steps exceeded",
            retryable: false,
            public: true,
            action: Some(
                "Raise loop_control.max_steps_per_turn in config.toml, or run \"/update-config\" then \"/reload\".",
            ),
        },
    )],
};

static REGISTERED: LazyLock<()> = LazyLock::new(|| {
    register_error_domain(&LOOP_ERRORS).expect("loop error codes are unique");
});

pub type LoopError = Error2;

pub fn ensure_loop_errors_registered() {
    LazyLock::force(&REGISTERED);
}

// Original: loop.ts, createMaxStepsExceededError().
pub fn create_max_steps_exceeded_error(max_steps: f64, message: Option<&str>) -> LoopError {
    ensure_loop_errors_registered();
    Error2::with_options(
        LOOP_MAX_STEPS_EXCEEDED,
        message.map_or_else(
            || {
                format!(
                    "Turn exceeded maxSteps={}. If max_steps_per_turn is too small, raise it in config.toml (loop_control.max_steps_per_turn), or run \"/update-config\" to update it, then \"/reload\".",
                    js_number_to_string(max_steps)
                )
            },
            str::to_owned,
        ),
        Error2Options {
            name: Some("LoopError".into()),
            details: Some(Map::from_iter([(
                "maxSteps".into(),
                serde_json::to_value(max_steps).unwrap_or(Value::Null),
            )])),
            cause: None,
        },
    )
}

// Original: loop.ts, isMaxStepsExceededError().
pub fn is_max_steps_exceeded_error(error: &(dyn Error + 'static)) -> bool {
    error
        .downcast_ref::<Error2>()
        .is_some_and(|error| error.code == LOOP_MAX_STEPS_EXCEEDED)
}

fn js_number_to_string(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value == f64::INFINITY {
        "Infinity".into()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".into()
    } else if value == 0.0 {
        "0".into()
    } else if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::{error_info, is_error_code};

    #[test]
    fn domain_preserves_retryability_and_public_max_step_guidance() {
        ensure_loop_errors_registered();
        assert!(is_error_code(LOOP_MAX_STEPS_EXCEEDED));
        assert!(is_error_code(TURN_AGENT_BUSY));
        let max_steps = error_info(LOOP_MAX_STEPS_EXCEEDED);
        assert_eq!(max_steps.title, "Loop max steps exceeded");
        assert!(!max_steps.retryable);
        assert!(max_steps.public);
        assert_eq!(
            max_steps.action.as_deref(),
            Some(
                "Raise loop_control.max_steps_per_turn in config.toml, or run \"/update-config\" then \"/reload\"."
            )
        );
        assert!(error_info(TURN_AGENT_BUSY).retryable);
    }

    #[test]
    fn max_step_error_keeps_name_code_details_message_and_guard() {
        let error = create_max_steps_exceeded_error(12.0, None);
        assert_eq!(error.name, "LoopError");
        assert_eq!(error.code, LOOP_MAX_STEPS_EXCEEDED);
        assert_eq!(error.details.as_ref().unwrap()["maxSteps"], 12.0);
        assert_eq!(
            error.message,
            "Turn exceeded maxSteps=12. If max_steps_per_turn is too small, raise it in config.toml (loop_control.max_steps_per_turn), or run \"/update-config\" to update it, then \"/reload\"."
        );
        assert!(is_max_steps_exceeded_error(&error));
        assert!(!is_max_steps_exceeded_error(&std::io::Error::other("no")));

        let custom = create_max_steps_exceeded_error(2.5, Some("custom"));
        assert_eq!(custom.message, "custom");
        assert_eq!(custom.details.as_ref().unwrap()["maxSteps"], 2.5);
    }

    #[test]
    fn default_message_formats_javascript_number_special_values() {
        for (value, rendered) in [
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (-0.0, "0"),
        ] {
            assert!(
                create_max_steps_exceeded_error(value, None)
                    .message
                    .starts_with(&format!("Turn exceeded maxSteps={rendered}."))
            );
        }
    }
}
