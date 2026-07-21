fn is_abort_message(message: &str) -> bool {
    message == "Aborted" || message.ends_with(": Aborted")
}

/// Rust callers extract the JavaScript-compatible error name and display
/// message at the I/O boundary. Errors without a distinct name pass `None`.
///
/// Original:
///   apps/kimi-code/src/tui/utils/errors.ts
///   isAbortError()
pub fn is_abort_error(name: Option<&str>, message: &str) -> bool {
    name == Some("AbortError") || is_abort_message(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_abort_error_name_regardless_of_message() {
        assert!(is_abort_error(Some("AbortError"), "request cancelled"));
        assert!(!is_abort_error(Some("TypeError"), "request cancelled"));
    }

    #[test]
    fn recognizes_exact_and_prefixed_abort_messages() {
        assert!(is_abort_error(None, "Aborted"));
        assert!(is_abort_error(None, "fetch: Aborted"));
        assert!(!is_abort_error(None, "aborted"));
        assert!(!is_abort_error(None, "Aborted request"));
        assert!(!is_abort_error(None, "prefix: Aborted suffix"));
    }
}
