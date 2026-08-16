// Original: packages/agent-core-v2/src/_base/utils/isoDateTime.ts.
// The wire primitive is shared with the already migrated protocol crate.
pub use kimi_code_protocol::time::{
    IsoDateTime, IsoDateTimeError, now_iso_date_time, parse_iso_date_time,
};

use chrono::{DateTime, SecondsFormat, Utc};

/// Formats a Unix timestamp in milliseconds as an RFC 3339 string with
/// millisecond precision, falling back to the Unix epoch for invalid inputs.
pub fn format_millis_rfc3339(timestamp_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Current Unix timestamp in milliseconds.
pub fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_millis_with_millisecond_precision() {
        assert_eq!(
            format_millis_rfc3339(1_700_000_000_123),
            "2023-11-14T22:13:20.123Z"
        );
        assert_eq!(format_millis_rfc3339(0), "1970-01-01T00:00:00.000Z");
        // Invalid timestamps fall back to the Unix epoch.
        assert_eq!(format_millis_rfc3339(i64::MAX), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn now_millis_is_positive_and_advancing() {
        let first = now_millis();
        let second = now_millis();
        assert!(second >= first);
        assert!(first > 1_600_000_000_000);
    }
}
