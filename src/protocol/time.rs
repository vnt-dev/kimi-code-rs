use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use std::error::Error;
use std::fmt;
use std::sync::LazyLock;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct IsoDateTime(String);

impl IsoDateTime {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for IsoDateTime {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for IsoDateTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl PartialEq<&str> for IsoDateTime {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl<'de> Deserialize<'de> for IsoDateTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_iso_date_time(&value).map_err(serde::de::Error::custom)
    }
}

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
    Ok(IsoDateTime(
        parsed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    ))
}

// Original: time.ts, nowIsoDateTime()
pub fn now_iso_date_time() -> IsoDateTime {
    IsoDateTime(
        DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}
