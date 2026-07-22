use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use std::error::Error;
use std::fmt;
use std::sync::LazyLock;
use std::time::SystemTime;

pub type IsoDateTime = String;

static ISO_8601_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}(?::?\d{2})?)$")
        .expect("static ISO-8601 regex must compile")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoDateTimeError {
    message: &'static str,
}

impl fmt::Display for IsoDateTimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for IsoDateTimeError {}

fn normalize_offset(value: &str) -> String {
    let Some(sign_at) = value.rfind(['+', '-']).filter(|at| *at > 10) else {
        return value.to_owned();
    };
    let suffix = &value[sign_at..];
    match suffix.len() {
        3 => format!("{value}:00"),
        5 => format!("{}{}:{}", &value[..sign_at], &suffix[..3], &suffix[3..]),
        _ => value.to_owned(),
    }
}

// Original: time.ts, isoDateTimeSchema transform.
pub fn parse_iso_date_time(value: &str) -> Result<IsoDateTime, IsoDateTimeError> {
    if !ISO_8601_RE.is_match(value) {
        return Err(IsoDateTimeError {
            message: "must be an ISO 8601 datetime string",
        });
    }
    let normalized = normalize_offset(value);
    let parsed = DateTime::parse_from_rfc3339(&normalized).map_err(|_| IsoDateTimeError {
        message: "invalid ISO 8601 datetime",
    })?;
    Ok(parsed
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

// Original: time.ts, nowIsoDateTime()
pub fn now_iso_date_time() -> IsoDateTime {
    DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Millis, true)
}
