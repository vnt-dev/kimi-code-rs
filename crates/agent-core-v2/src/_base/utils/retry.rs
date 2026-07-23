use std::{error::Error, sync::Arc, time::Duration};

use serde_json::Value;
use uuid::Uuid;

use super::abort::{AbortError, AbortSignal, abortable};
use crate::_base::errors::errors::Error2;

pub const DEFAULT_MAX_RETRY_ATTEMPTS: usize = 10;
const BASE_DELAY_MS: f64 = 500.0;
const MAX_DELAY_MS: f64 = 32_000.0;
const RETRY_FACTOR: f64 = 2.0;
const JITTER_FACTOR: f64 = 0.25;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryErrorFields {
    pub error_name: String,
    pub error_message: String,
    pub status_code: Option<u16>,
}

// Original: packages/agent-core-v2/src/_base/utils/retry.ts, retryBackoffDelays().
pub fn retry_backoff_delays(max_attempts: usize) -> Vec<f64> {
    retry_backoff_delays_with(max_attempts, random_unit)
}

pub fn retry_backoff_delays_with(max_attempts: usize, mut random: impl FnMut() -> f64) -> Vec<f64> {
    (0..max_attempts.saturating_sub(1))
        .map(|attempt| {
            let base = (BASE_DELAY_MS * RETRY_FACTOR.powi(attempt as i32)).min(MAX_DELAY_MS);
            base + random().clamp(0.0, 1.0) * JITTER_FACTOR * base
        })
        .collect()
}

fn random_unit() -> f64 {
    let bytes = Uuid::new_v4();
    let raw = u64::from_le_bytes(bytes.as_bytes()[..8].try_into().expect("UUID has 16 bytes"));
    ((raw >> 11) as f64) / ((1_u64 << 53) as f64)
}

pub fn read_retry_after_ms(error: &(dyn Error + 'static)) -> Option<f64> {
    let value = error
        .downcast_ref::<Error2>()?
        .details
        .as_ref()?
        .get("retryAfterMs")?
        .as_f64()?;
    (value > 0.0).then_some(value)
}

pub async fn sleep_for_retry(
    delay_ms: f64,
    signal: Option<&AbortSignal>,
) -> Result<(), Arc<AbortError>> {
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    let seconds = if delay_ms.is_finite() {
        delay_ms.max(0.0) / 1_000.0
    } else {
        0.0
    };
    let sleep = tokio::time::sleep(Duration::from_secs_f64(seconds));
    match signal {
        Some(signal) => abortable(sleep, signal).await,
        None => {
            sleep.await;
            Ok(())
        }
    }
}

pub fn retry_error_fields(error: &(dyn Error + 'static)) -> RetryErrorFields {
    let (error_name, status_code) = if let Some(error) = error.downcast_ref::<Error2>() {
        (
            error.name.clone(),
            error.details.as_ref().and_then(status_from_details),
        )
    } else if let Some(error) = error.downcast_ref::<AbortError>() {
        (error.name().to_owned(), None)
    } else if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        (
            "Error".to_owned(),
            error.status().map(|status| status.as_u16()),
        )
    } else {
        ("Error".to_owned(), None)
    };
    RetryErrorFields {
        error_name,
        error_message: error.to_string(),
        status_code,
    }
}

fn status_from_details(details: &serde_json::Map<String, Value>) -> Option<u16> {
    let value = details.get("statusCode")?.as_u64()?;
    u16::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::_base::{
        errors::errors::{Error2, Error2Options},
        utils::abort::AbortController,
    };

    #[test]
    fn backoff_count_cap_and_jitter_match_source_policy() {
        assert!(retry_backoff_delays_with(0, || 0.0).is_empty());
        assert_eq!(
            retry_backoff_delays_with(5, || 0.0),
            vec![500.0, 1_000.0, 2_000.0, 4_000.0]
        );
        assert_eq!(retry_backoff_delays_with(9, || 1.0)[7], MAX_DELAY_MS * 1.25);
    }

    #[test]
    fn extracts_retry_metadata_from_structured_errors() {
        let error = Error2::with_options(
            "provider.rate_limit",
            "slow down",
            Error2Options {
                name: Some("ProviderError".into()),
                details: Some(Map::from_iter([
                    ("statusCode".into(), Value::from(429)),
                    ("retryAfterMs".into(), Value::from(2500)),
                ])),
                ..Error2Options::default()
            },
        );
        assert_eq!(read_retry_after_ms(&error), Some(2500.0));
        assert_eq!(
            retry_error_fields(&error),
            RetryErrorFields {
                error_name: "ProviderError".into(),
                error_message: "slow down".into(),
                status_code: Some(429),
            }
        );
    }

    #[tokio::test]
    async fn retry_sleep_honors_existing_abort() {
        let controller = AbortController::new();
        controller.abort(None);
        assert!(
            sleep_for_retry(10_000.0, Some(&controller.signal()))
                .await
                .is_err()
        );
    }
}
