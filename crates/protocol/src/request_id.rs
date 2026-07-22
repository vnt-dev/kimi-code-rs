use std::sync::LazyLock;

use regex::Regex;
use ulid::Ulid;

pub static ULID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-7][0-9A-HJKMNP-TV-Z]{25}$").expect("static ULID regex must compile")
});

// Original: request-id.ts, isUlid()
pub fn is_ulid(value: &str) -> bool {
    ULID_REGEX.is_match(value) && Ulid::from_string(value).is_ok()
}

// Original: request-id.ts, parseOrGenerateRequestId()
pub fn parse_or_generate_request_id(header_value: Option<&str>) -> String {
    header_value
        .filter(|value| is_ulid(value))
        .map(str::to_owned)
        .unwrap_or_else(|| Ulid::new().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CursorQuery, ERROR_CODE_REASON, ErrorCode, err_envelope, ok_envelope,
        pagination::PaginationValidationError, time::parse_iso_date_time,
    };

    #[test]
    fn core_protocol_wire_contracts_match_source_package() {
        assert_eq!(
            serde_json::to_string(&ok_envelope(serde_json::json!({"id":"sess_1"}), "req_y"))
                .unwrap(),
            r#"{"code":0,"msg":"success","data":{"id":"sess_1"},"request_id":"req_y"}"#
        );
        assert_eq!(
            serde_json::to_string(&err_envelope(
                ErrorCode::SessionNotFound,
                "missing",
                "req_z",
                None,
            ))
            .unwrap(),
            r#"{"code":40401,"msg":"missing","data":null,"request_id":"req_z"}"#
        );
        assert_eq!(
            ERROR_CODE_REASON[&ErrorCode::GoalUnsupportedAgent],
            "goal.unsupported_agent"
        );

        let error: PaginationValidationError = CursorQuery {
            before_id: Some("a".to_owned()),
            after_id: Some("b".to_owned()),
            page_size: None,
        }
        .validate()
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ValidationFailed);
        assert_eq!(error.path, "before_id");
        assert_eq!(
            parse_iso_date_time("2026-06-04T18:30:00+08:00").unwrap(),
            "2026-06-04T10:30:00.000Z"
        );
        let normalized: crate::IsoDateTime =
            serde_json::from_str("\"2026-06-04T18:30:00+08:00\"").unwrap();
        assert_eq!(normalized, "2026-06-04T10:30:00.000Z");
        assert!(
            serde_json::from_str::<CursorQuery>(r#"{"before_id":"a","after_id":"b"}"#).is_err()
        );

        let generated = parse_or_generate_request_id(None);
        assert!(is_ulid(&generated));
        assert_eq!(parse_or_generate_request_id(Some(&generated)), generated);
    }
}
