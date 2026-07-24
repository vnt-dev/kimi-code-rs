//! Deterministic per-task cron jitter.
//!
//! Original: `packages/agent-core-v2/src/app/cron/jitter.ts`.

use std::cmp::Ordering;

use chrono::{DateTime, Local, Timelike, Utc};

use super::{ParsedCronExpression, compute_next_cron_run};

const MS_PER_DAY: f64 = 86_400_000.0;
const MS_PER_MINUTE: f64 = 60_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JitterConfig {
    pub recurring_max_fraction_of_period: f64,
    pub recurring_max_ms: f64,
    pub one_shot_max_ms: f64,
}
pub const DEFAULT_CRON_JITTER_CONFIG: JitterConfig = JitterConfig {
    recurring_max_fraction_of_period: 0.1,
    recurring_max_ms: 900_000.0,
    one_shot_max_ms: 90_000.0,
};

pub fn jittered_next_cron_run_ms(
    id: &str,
    parsed: &ParsedCronExpression,
    ideal_ms: f64,
    config: JitterConfig,
    no_jitter: Option<bool>,
) -> f64 {
    if no_jitter == Some(true) {
        return ideal_ms;
    }
    let next = compute_next_cron_run(parsed, ideal_ms);
    let period = next
        .filter(|next| *next > ideal_ms)
        .map_or(MS_PER_DAY, |next| next - ideal_ms);
    let cap = (period * config.recurring_max_fraction_of_period).min(config.recurring_max_ms);
    if cap.partial_cmp(&0.0) != Some(Ordering::Greater) {
        return ideal_ms;
    }
    ideal_ms + cap * fraction_from_id(id)
}

pub fn one_shot_jittered_next_cron_run_ms(
    id: &str,
    created_at: Option<f64>,
    ideal_ms: f64,
    config: JitterConfig,
    no_jitter: Option<bool>,
) -> f64 {
    if no_jitter == Some(true) || ideal_ms % MS_PER_MINUTE != 0.0 {
        return ideal_ms;
    }
    let Some(date) = DateTime::<Utc>::from_timestamp((ideal_ms / 1000.0).floor() as i64, 0)
        .map(|date| date.with_timezone(&Local))
    else {
        return ideal_ms;
    };
    if !matches!(date.minute(), 0 | 30)
        || config.one_shot_max_ms.partial_cmp(&0.0) != Some(Ordering::Greater)
    {
        return ideal_ms;
    }
    let shifted = ideal_ms - config.one_shot_max_ms * fraction_from_id(id);
    if created_at.is_some_and(|created| shifted < created) {
        ideal_ms
    } else {
        shifted
    }
}

fn fraction_from_id(id: &str) -> f64 {
    if id.len() == 8
        && id.bytes().all(|byte| byte.is_ascii_hexdigit())
        && let Ok(value) = u32::from_str_radix(id, 16)
    {
        return value as f64 / 4_294_967_296.0;
    }
    let mut hash: i32 = 5381;
    for code_unit in id.encode_utf16() {
        hash = hash
            .wrapping_shl(5)
            .wrapping_add(hash)
            .wrapping_add(code_unit as i32);
    }
    (hash as u32) as f64 / 4_294_967_296.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::cron::parse_cron_expression;
    #[test]
    fn jitter_is_deterministic_and_can_be_disabled() {
        let parsed = parse_cron_expression("*/5 * * * *").unwrap();
        let ideal = 0.0;
        let first =
            jittered_next_cron_run_ms("deadbeef", &parsed, ideal, DEFAULT_CRON_JITTER_CONFIG, None);
        assert_eq!(
            first,
            jittered_next_cron_run_ms("deadbeef", &parsed, ideal, DEFAULT_CRON_JITTER_CONFIG, None)
        );
        assert!(first >= ideal && first <= 30_000.0);
        assert_eq!(
            jittered_next_cron_run_ms(
                "deadbeef",
                &parsed,
                ideal,
                DEFAULT_CRON_JITTER_CONFIG,
                Some(true)
            ),
            ideal
        );
    }
    #[test]
    fn one_shot_only_moves_round_times_without_crossing_creation() {
        let shifted = one_shot_jittered_next_cron_run_ms(
            "deadbeef",
            None,
            0.0,
            DEFAULT_CRON_JITTER_CONFIG,
            None,
        );
        assert!(shifted <= 0.0);
        assert_eq!(
            one_shot_jittered_next_cron_run_ms(
                "deadbeef",
                0.0f64.into(),
                0.0,
                DEFAULT_CRON_JITTER_CONFIG,
                None
            ),
            0.0
        );
        assert_eq!(
            one_shot_jittered_next_cron_run_ms(
                "deadbeef",
                None,
                7.0 * MS_PER_MINUTE,
                DEFAULT_CRON_JITTER_CONFIG,
                None
            ),
            7.0 * MS_PER_MINUTE
        );
    }
}
