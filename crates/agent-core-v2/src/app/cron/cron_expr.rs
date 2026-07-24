//! Five-field local-time cron parser and next-run calculation.
//!
//! Original: `packages/agent-core-v2/src/app/cron/cron-expr.ts`.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, Duration, Local, Timelike, Utc};

const MS_PER_MINUTE: f64 = 60_000.0;
const HARD_ITERATION_CAP: usize = 10_000_000;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct CronExpressionError {
    message: String,
}
impl CronExpressionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedCronExpression {
    pub raw: String,
    pub minutes: BTreeSet<u32>,
    pub hours: BTreeSet<u32>,
    pub days_of_month: BTreeSet<u32>,
    pub months: BTreeSet<u32>,
    pub days_of_week: BTreeSet<u32>,
    pub days_of_month_wildcard: bool,
    pub days_of_week_wildcard: bool,
}

pub fn parse_cron_expression(expr: &str) -> Result<ParsedCronExpression, CronExpressionError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(CronExpressionError::new("cron expression is empty"));
    }
    let fields = trimmed.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        return Err(CronExpressionError::new(format!(
            "cron expression must have exactly 5 fields (minute hour day-of-month month day-of-week); got {}",
            fields.len()
        )));
    }
    let minutes = parse_field(fields[0], 0, 59, "minute")?;
    let hours = parse_field(fields[1], 0, 23, "hour")?;
    let days_of_month = parse_field(fields[2], 1, 31, "day-of-month")?;
    let months = parse_field(fields[3], 1, 12, "month")?;
    let mut days_of_week = BTreeSet::new();
    for value in parse_field(fields[4], 0, 7, "day-of-week")? {
        days_of_week.insert(if value == 7 { 0 } else { value });
    }
    Ok(ParsedCronExpression {
        raw: trimmed.into(),
        minutes,
        hours,
        days_of_month,
        months,
        days_of_week,
        days_of_month_wildcard: fields[2] == "*",
        days_of_week_wildcard: fields[4] == "*",
    })
}

fn parse_field(
    field: &str,
    min: u32,
    max: u32,
    name: &str,
) -> Result<BTreeSet<u32>, CronExpressionError> {
    if field.is_empty() {
        return Err(CronExpressionError::new(format!(
            "cron {name} field is empty"
        )));
    }
    let mut result = BTreeSet::new();
    for term in field.split(',') {
        if term.is_empty() {
            return Err(CronExpressionError::new(format!(
                "cron {name} field has empty term in list"
            )));
        }
        add_term(&mut result, term, min, max, name)?;
    }
    if result.is_empty() {
        return Err(CronExpressionError::new(format!(
            "cron {name} field matches no values"
        )));
    }
    Ok(result)
}
fn cron_int(raw: &str, name: &str, role: &str) -> Result<u32, CronExpressionError> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CronExpressionError::new(format!(
            "cron {name} {role} must be a non-negative integer with digits only (got {:?})",
            raw
        )));
    }
    raw.parse().map_err(|_| {
        CronExpressionError::new(format!(
            "cron {name} {role} must be a non-negative integer with digits only (got {:?})",
            raw
        ))
    })
}
fn add_term(
    out: &mut BTreeSet<u32>,
    term: &str,
    min: u32,
    max: u32,
    name: &str,
) -> Result<(), CronExpressionError> {
    let (range, step) = match term.split_once('/') {
        Some((range, step)) => {
            if step.is_empty() {
                return Err(CronExpressionError::new(format!(
                    "cron {name} step is empty in \"{term}\""
                )));
            }
            let step = cron_int(step, name, "step")?;
            if step == 0 {
                return Err(CronExpressionError::new(format!(
                    "cron {name} step must be a positive integer (got \"{}\")",
                    term.split_once('/').unwrap().1
                )));
            }
            if range.is_empty() {
                return Err(CronExpressionError::new(format!(
                    "cron {name} step needs a range or \"*\" before \"/\" in \"{term}\""
                )));
            }
            (range, step)
        }
        None => (term, 1),
    };
    let (lo, hi, stepped) = if range == "*" {
        (min, max, term.contains('/'))
    } else if let Some((lo, hi)) = range.split_once('-') {
        let lo = cron_int(lo, name, "range lower bound")?;
        let hi = cron_int(hi, name, "range upper bound")?;
        if lo < min || hi > max || lo > hi {
            return Err(CronExpressionError::new(format!(
                "cron {name} range {lo}-{hi} out of bounds (must be {min}..{max}, ascending)"
            )));
        }
        (lo, hi, true)
    } else {
        let value = cron_int(range, name, "value")?;
        if value < min || value > max {
            return Err(CronExpressionError::new(format!(
                "cron {name} value {value} out of range {min}..{max}"
            )));
        }
        if !term.contains('/') {
            out.insert(value);
            return Ok(());
        }
        (value, max, true)
    };
    let _ = stepped;
    for value in (lo..=hi).step_by(step as usize) {
        out.insert(value);
    }
    Ok(())
}

pub fn compute_next_cron_run(expr: &ParsedCronExpression, from_ms: f64) -> Option<f64> {
    next_run_within_minutes(expr, from_ms, 5 * 366 * 24 * 60)
}
pub fn has_fire_within_years(expr: &ParsedCronExpression, years: f64, from_ms: f64) -> bool {
    next_run_within_minutes(
        expr,
        from_ms,
        (years * 366.0 * 24.0 * 60.0).floor().max(1.0) as usize,
    )
    .is_some()
}
fn next_run_within_minutes(expr: &ParsedCronExpression, from_ms: f64, cap: usize) -> Option<f64> {
    let seconds = (from_ms / 1000.0).floor() as i64;
    let mut date = DateTime::<Utc>::from_timestamp(seconds, 0)?.with_timezone(&Local);
    date = date
        - Duration::seconds(date.second() as i64)
        - Duration::nanoseconds(date.nanosecond() as i64)
        + Duration::minutes(1);
    let deadline = from_ms + cap as f64 * MS_PER_MINUTE;
    for _ in 0..cap.min(HARD_ITERATION_CAP) {
        if date.timestamp_millis() as f64 > deadline {
            return None;
        }
        if matches(expr, &date) {
            return Some(date.timestamp_millis() as f64);
        }
        date += Duration::minutes(1);
    }
    None
}
fn matches(expr: &ParsedCronExpression, date: &DateTime<Local>) -> bool {
    if !expr.months.contains(&date.month())
        || !expr.hours.contains(&date.hour())
        || !expr.minutes.contains(&date.minute())
    {
        return false;
    }
    let dom = expr.days_of_month.contains(&date.day());
    let dow = expr
        .days_of_week
        .contains(&date.weekday().num_days_from_sunday());
    if expr.days_of_month_wildcard && expr.days_of_week_wildcard {
        return true;
    }
    if expr.days_of_month_wildcard {
        return dow;
    }
    if expr.days_of_week_wildcard {
        return dom;
    }
    dom || dow
}

pub fn cron_to_human(expr: &ParsedCronExpression) -> String {
    let all_min = full(&expr.minutes, 0, 59);
    let all_hour = full(&expr.hours, 0, 23);
    let all_month = full(&expr.months, 1, 12);
    if all_hour && expr.days_of_month_wildcard && all_month && expr.days_of_week_wildcard {
        if let Some(step) = detect_step(&expr.minutes, 0, 59).filter(|step| *step > 1) {
            return format!("every {step} minutes");
        }
        if all_min {
            return "every minute".into();
        }
        if expr.minutes.len() == 1 {
            return format!("at minute {} of every hour", expr.minutes.first().unwrap());
        }
    }
    if expr.minutes.len() == 1
        && expr.days_of_month_wildcard
        && all_month
        && expr.days_of_week_wildcard
        && let Some(step) = detect_step(&expr.hours, 0, 23).filter(|step| *step > 1)
    {
        return format!(
            "every {step} hours at minute {:02}",
            expr.minutes.first().unwrap()
        );
    }
    if expr.minutes.len() == 1 && expr.hours.len() == 1 && expr.days_of_month_wildcard && all_month
    {
        let h = expr.hours.first().unwrap();
        let m = expr.minutes.first().unwrap();
        if expr.days_of_week_wildcard {
            return format!("at {h:02}:{m:02} every day");
        }
        if let Some(days) = format_dows(&expr.days_of_week) {
            return format!("at {h:02}:{m:02} on {days}");
        }
    }
    if expr.minutes.len() == 1
        && expr.hours.len() == 1
        && expr.days_of_month.len() == 1
        && !expr.days_of_month_wildcard
        && expr.months.len() == 1
        && expr.days_of_week_wildcard
    {
        const MONTHS: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        return format!(
            "at {:02}:{:02} on day {} of {}",
            expr.hours.first().unwrap(),
            expr.minutes.first().unwrap(),
            expr.days_of_month.first().unwrap(),
            MONTHS[(expr.months.first().unwrap() - 1) as usize]
        );
    }
    expr.raw.clone()
}
fn full(set: &BTreeSet<u32>, min: u32, max: u32) -> bool {
    set.len() == (max - min + 1) as usize && (min..=max).all(|v| set.contains(&v))
}
fn detect_step(set: &BTreeSet<u32>, min: u32, max: u32) -> Option<u32> {
    let values = set.iter().copied().collect::<Vec<_>>();
    if values.len() < 2 || values[0] != min {
        return None;
    }
    let step = values[1] - values[0];
    if step == 0 {
        return None;
    }
    if values
        .iter()
        .enumerate()
        .any(|(i, v)| *v != min + i as u32 * step)
        || *values.last().unwrap() > max
    {
        return None;
    }
    Some(step)
}
fn format_dows(set: &BTreeSet<u32>) -> Option<String> {
    let values = set.iter().copied().collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    if values == [1, 2, 3, 4, 5] {
        return Some("weekdays".into());
    }
    if values == [0, 6] {
        return Some("weekends".into());
    }
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    Some(
        values
            .into_iter()
            .map(|v| DAYS[v as usize])
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_and_human_forms_match_source_rules() {
        let every = parse_cron_expression("*/5 * * * *").unwrap();
        assert_eq!(cron_to_human(&every), "every 5 minutes");
        let weekday = parse_cron_expression("0 9 * * 1-5").unwrap();
        assert_eq!(cron_to_human(&weekday), "at 09:00 on weekdays");
        assert_eq!(
            parse_cron_expression("0 0 31 2 *").unwrap().raw,
            "0 0 31 2 *"
        );
        assert!(parse_cron_expression("*/0 * * * *").is_err());
    }
    #[test]
    fn dom_and_dow_are_or_when_restricted() {
        let expr = parse_cron_expression("0 0 1 * 1").unwrap();
        assert!(has_fire_within_years(&expr, 1.0, 0.0));
    }
}
