pub const FEEDBACK_ISSUE_URL: &str = "https://github.com/MoonshotAI/kimi-code/issues";
pub const FEEDBACK_TELEMETRY_EVENT: &str = "feedback_submitted";
pub const FEEDBACK_VERSION_PREFIX: &str = "kimi-code-";

pub const FEEDBACK_STATUS_SUBMITTING: &str = "Submitting feedback…";
pub const FEEDBACK_STATUS_UPLOADING: &str = "Uploading attachments, this could take a few minutes…";
pub const FEEDBACK_STATUS_SUCCESS: &str = "Feedback submitted, thank you!";
pub const FEEDBACK_STATUS_CANCELLED: &str = "Feedback cancelled.";
pub const FEEDBACK_STATUS_NETWORK_ERROR: &str = "Network error, failed to submit feedback.";
pub const FEEDBACK_STATUS_FALLBACK: &str = "Opening GitHub Issues as fallback…";
pub const FEEDBACK_STATUS_NOT_SIGNED_IN: &str =
    "You're not signed in. Opening GitHub Issues for feedback…";
pub const FEEDBACK_STATUS_UPLOAD_FAILED: &str =
    "Feedback sent; attachment upload failed — see feedback-upload.log.";

// Original: `src/tui/constant/feedback.ts`, `feedbackHttpErrorMessage()`.
pub fn feedback_http_error_message(status: u16) -> String {
    format!("Failed to submit feedback (HTTP {status}).")
}

// Original: `feedbackSessionLine()`.
pub fn feedback_session_line(session_id: &str) -> String {
    format!("Session: {session_id}")
}

// Original: `feedbackIdLine()`.
pub fn feedback_id_line(feedback_id: f64) -> String {
    format!("Feedback ID: {feedback_id}")
}

// Original: `errorReportHintLine()`.
pub const fn error_report_hint_line() -> &'static str {
    "If this persists, run `/export-debug-zip` and share the file with us for diagnosis. Please don't share it publicly."
}

// Original: `withFeedbackVersionPrefix()`.
pub fn with_feedback_version_prefix(version: &str) -> String {
    format!("{FEEDBACK_VERSION_PREFIX}{version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_feedback_identifiers_errors_and_version() {
        assert_eq!(
            feedback_http_error_message(429),
            "Failed to submit feedback (HTTP 429)."
        );
        assert_eq!(feedback_session_line("ses-1"), "Session: ses-1");
        assert_eq!(feedback_id_line(3.0), "Feedback ID: 3");
        assert_eq!(with_feedback_version_prefix("1.2.3"), "kimi-code-1.2.3");
    }

    #[test]
    fn preserves_external_urls_telemetry_and_diagnostic_hint() {
        assert_eq!(
            FEEDBACK_ISSUE_URL,
            "https://github.com/MoonshotAI/kimi-code/issues"
        );
        assert_eq!(FEEDBACK_TELEMETRY_EVENT, "feedback_submitted");
        assert!(error_report_hint_line().contains("`/export-debug-zip`"));
    }
}
